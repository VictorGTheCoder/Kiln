//! Resume after interruption, driven through the CLI in a temporary repository.
//! Interruptions are injected with `KILN_FAULT_INJECT=<point>@<ticket>`, which ends the
//! process abruptly (no destructors, no final state save) at that point.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const FAULT_EXIT: i32 = 86;

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    fn new(concurrency: Option<u64>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let repo = Self { _temp: temp, path };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        fs::write(
            repo.path.join("one.md"),
            "# One\n## Acceptance criteria\n- Works\n",
        )
        .unwrap();
        fs::write(
            repo.path.join("two.md"),
            "# Two\n## Acceptance criteria\n- Also works\n",
        )
        .unwrap();
        fs::write(repo.path.join(".gitignore"), ".kiln/\n").unwrap();
        let mut config = json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}});
        if let Some(limit) = concurrency {
            config["implementation_concurrency"] = json!(limit);
        }
        fs::write(repo.path.join("kiln.json"), config.to_string()).unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn cli_with(&self, args: &[&str], fault: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln"));
        command.args(args).current_dir(&self.path);
        match fault {
            Some(fault) => command.env("KILN_FAULT_INJECT", fault),
            None => command.env_remove("KILN_FAULT_INJECT"),
        };
        command.output().unwrap()
    }
    fn cli(&self, args: &[&str]) -> Output {
        self.cli_with(args, None)
    }
    fn planned(&self, tickets: &[(&str, &str, &[&str])]) -> String {
        let prepared = self.cli(&[
            "prepare",
            "--config",
            "kiln.json",
            "--spec",
            "one.md",
            "--spec",
            "two.md",
        ]);
        assert!(
            prepared.status.success(),
            "{}",
            String::from_utf8_lossy(&prepared.stderr)
        );
        let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
        let id = prepared["id"].as_str().unwrap().to_owned();
        let plan_tickets: Vec<Value> = tickets
            .iter()
            .map(|(name, spec, blockers)| json!({"id":name,"title":name,"description":"Deliver","acceptance_criteria":["works"],"covers":[format!("{spec}#ac-1")],"blocked_by":blockers}))
            .collect();
        fs::write(
            self.path.join("plan.json"),
            json!({"tickets":plan_tickets,"verification":{"outcome":"verified","findings":[]}})
                .to_string(),
        )
        .unwrap();
        let planned = self.cli(&["plan", &id, "--fixture", "plan.json"]);
        assert!(
            planned.status.success(),
            "{}",
            String::from_utf8_lossy(&planned.stderr)
        );
        id
    }
    fn scenario(&self, scenario: Value) {
        fs::write(self.path.join("scenario.json"), scenario.to_string()).unwrap();
    }
    /// Run until the injected fault interrupts the process.
    fn interrupted_run(&self, id: &str, fault: &str) {
        let out = self.cli_with(&["run", id, "--fixture", "scenario.json"], Some(fault));
        assert_eq!(
            out.status.code(),
            Some(FAULT_EXIT),
            "run was not interrupted at {fault}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn resume(&self, id: &str) -> (Output, Value) {
        let out = self.cli(&["resume", id, "--fixture", "scenario.json"]);
        let run = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("no run state: {}", String::from_utf8_lossy(&out.stderr)));
        (out, run)
    }
    fn inspect(&self, id: &str) -> Value {
        serde_json::from_slice(&self.cli(&["inspect", id]).stdout).unwrap()
    }
}
fn approved() -> Value {
    json!({"outcome":"approved","findings":[],"evidence":"Observed concrete change"})
}
fn works(file: &str, content: &str) -> Value {
    json!({"implementation":{"files":{file:content},"outcome":"completed"},"review":{"standards":approved(),"spec":approved()}})
}
fn ticket<'a>(run: &'a Value, id: &str) -> &'a Value {
    run["scheduler"]["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}
fn sessions_for<'a>(run: &'a Value, id: &str) -> Vec<&'a Value> {
    run["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["ticket_id"] == id)
        .collect()
}
/// Decisions of the latest recovery that concern `ticket`.
fn decisions<'a>(run: &'a Value, ticket: &str) -> Vec<&'a Value> {
    run["recoveries"]
        .as_array()
        .expect("recoveries are recorded")
        .last()
        .expect("a recovery was recorded")["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["ticket_id"] == ticket)
        .collect()
}
fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn interrupted_implementation_is_restarted_in_a_fresh_session_with_a_reason() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &[])]);
    repo.scenario(json!({"tickets":{"a":works("a.txt","works"),"b":works("b.txt","works")}}));
    repo.interrupted_run(&id, "implementation.after_agent@a");

    let (out, run) = repo.resume(&id);
    assert_success(&out);
    assert_eq!(run["status"], "awaiting_validation");
    let sessions = sessions_for(&run, "a");
    assert_eq!(
        sessions.len(),
        2,
        "interrupted session kept, fresh one started"
    );
    assert_eq!(sessions[0]["status"], "interrupted");
    assert_eq!(sessions[1]["status"], "integrated");
    let restart = decisions(&run, "a");
    assert!(
        restart
            .iter()
            .any(|d| d["action"] == "restarted" && d["subject"] == sessions[0]["id"]),
        "{restart:?}"
    );
    assert!(restart
        .iter()
        .all(|d| !d["reason"].as_str().unwrap().is_empty()));
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    assert_eq!(repo.git(&["show", &format!("{tip}:a.txt")]), "works");
    assert_eq!(ticket(&run, "a")["state"], "integrated");
    assert_eq!(repo.inspect(&id)["status"], "awaiting_validation");
}

#[test]
fn commit_created_before_its_record_is_adopted_and_reverified_not_reimplemented() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("z", "two.md", &["a"])]);
    repo.scenario(json!({"tickets":{"a":works("a.txt","first"),"z":works("z.txt","works")}}));
    repo.interrupted_run(&id, "implementation.after_commit@a");
    let recorded = repo.inspect(&id);
    let session = sessions_for(&recorded, "a")[0].clone();
    assert_eq!(session["status"], "running");
    assert!(
        session["commit"].is_null(),
        "commit effect was not recorded"
    );
    let committed = repo.git(&["rev-parse", session["branch"].as_str().unwrap()]);

    // A repeated implementation would produce different content.
    repo.scenario(json!({"tickets":{"a":works("a.txt","second"),"z":works("z.txt","works")}}));
    let (out, run) = repo.resume(&id);
    assert_success(&out);
    let sessions = sessions_for(&run, "a");
    assert_eq!(sessions.len(), 1, "no second implementation session");
    assert_eq!(sessions[0]["commit"], committed.as_str());
    assert_eq!(sessions[0]["status"], "integrated");
    assert!(
        sessions[0]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "test" && c["passed"] == true),
        "missing verification evidence is rerun on the adopted commit"
    );
    assert_eq!(
        repo.git(&[
            "rev-list",
            "--count",
            &format!("{}..{committed}", session["base_commit"].as_str().unwrap())
        ]),
        "1",
        "exactly one implementation commit"
    );
    let adopted = decisions(&run, "a");
    assert!(
        adopted.iter().any(|d| d["action"] == "adopted" && d["reason"].as_str().unwrap().contains(&committed)),
        "{adopted:?}"
    );
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    assert_eq!(repo.git(&["show", &format!("{tip}:a.txt")]), "first");
}

#[test]
fn interrupted_review_is_rerun_on_the_preserved_implementation() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("z", "two.md", &["a"])]);
    repo.scenario(json!({"tickets":{"a":works("a.txt","first"),"z":works("z.txt","works")}}));
    repo.interrupted_run(&id, "review.after_axis@a");
    let recorded = repo.inspect(&id);
    let implemented = sessions_for(&recorded, "a")[0].clone();
    assert_eq!(implemented["status"], "implemented");
    assert!(recorded["reviews"].as_array().unwrap().is_empty());

    repo.scenario(json!({"tickets":{"a":works("a.txt","second"),"z":works("z.txt","works")}}));
    let (out, run) = repo.resume(&id);
    assert_success(&out);
    let sessions = sessions_for(&run, "a");
    assert_eq!(
        sessions.len(),
        1,
        "completed implementation is not repeated"
    );
    assert_eq!(sessions[0]["commit"], implemented["commit"]);
    let reviews: Vec<_> = run["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["session_id"] == implemented["id"])
        .collect();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0]["passed"], true);
    let rerun = decisions(&run, "a");
    assert!(
        rerun.iter().any(|d| d["action"] == "rerun"
            && d["subject"] == implemented["id"]
            && d["reason"].as_str().unwrap().contains("review")),
        "{rerun:?}"
    );
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    assert_eq!(repo.git(&["show", &format!("{tip}:a.txt")]), "first");
}

fn integration_attempts<'a>(run: &'a Value, id: &str) -> Vec<&'a Value> {
    run["integrations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["ticket_id"] == id)
        .collect()
}

#[test]
fn resume_recovers_an_empty_legacy_integration_lock_marker() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &[])]);
    repo.scenario(
        json!({"tickets":{"a":works("a.txt", "works"),"b":works("b.txt", "also works")}}),
    );
    fs::create_dir_all(repo.path.join(".kiln")).unwrap();
    fs::write(repo.path.join(".kiln/integration.lock"), b"").unwrap();

    let out = repo.cli(&["run", &id, "--fixture", "scenario.json"]);
    let run: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("no run state: {}", String::from_utf8_lossy(&out.stderr)));

    assert_success(&out);
    assert_eq!(run["status"], "awaiting_validation");
    assert_eq!(integration_attempts(&run, "a").len(), 1);
    assert_eq!(integration_attempts(&run, "b").len(), 1);
}

#[test]
fn integration_ref_updated_before_its_record_is_adopted_not_merged_again() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("z", "two.md", &["a"])]);
    repo.scenario(json!({"tickets":{"a":works("a.txt","first"),"z":works("z.txt","works")}}));
    repo.interrupted_run(&id, "integration.after_update_ref@a");
    let branch = format!("kiln/{id}/integration");
    let published = repo.git(&["rev-parse", &branch]);
    let recorded = repo.inspect(&id);
    assert_eq!(
        integration_attempts(&recorded, "a")[0]["status"],
        "verified"
    );

    let (out, run) = repo.resume(&id);
    assert_success(&out);
    let attempts = integration_attempts(&run, "a");
    assert_eq!(attempts.len(), 1, "integration is not attempted again");
    assert_eq!(attempts[0]["status"], "integrated");
    assert_eq!(attempts[0]["integrated_commit"], published.as_str());
    assert_eq!(sessions_for(&run, "a")[0]["status"], "integrated");
    let tip = repo.git(&["rev-parse", &branch]);
    assert_eq!(
        repo.git(&["rev-parse", &format!("{tip}^1")]),
        published,
        "z builds directly on a's adopted integration"
    );
    assert_eq!(
        repo.git(&["rev-list", "--count", "--merges", &branch]),
        "2",
        "one merge per ticket"
    );
    let adopted = decisions(&run, "a");
    assert!(
        adopted
            .iter()
            .any(|d| d["action"] == "adopted" && d["subject"] == attempts[0]["id"]),
        "{adopted:?}"
    );
}

#[test]
fn verified_integration_interrupted_before_its_ref_update_is_completed_once() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("z", "two.md", &["a"])]);
    repo.scenario(json!({"tickets":{"a":works("a.txt","first"),"z":works("z.txt","works")}}));
    repo.interrupted_run(&id, "integration.before_update_ref@a");
    let branch = format!("kiln/{id}/integration");
    let recorded = repo.inspect(&id);
    let attempt = integration_attempts(&recorded, "a")[0].clone();
    assert_eq!(attempt["status"], "verified");
    assert_eq!(
        repo.git(&["rev-parse", &branch]),
        attempt["base_commit"].as_str().unwrap()
    );

    let (out, run) = repo.resume(&id);
    assert_success(&out);
    let attempts = integration_attempts(&run, "a");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["status"], "integrated");
    assert_eq!(
        attempts[0]["integrated_commit"],
        attempt["candidate_commit"]
    );
    assert_eq!(repo.git(&["rev-list", "--count", "--merges", &branch]), "2");
    let completed = decisions(&run, "a");
    assert!(
        completed
            .iter()
            .any(|d| d["action"] == "completed" && d["subject"] == attempt["id"]),
        "{completed:?}"
    );
}

#[test]
fn integration_interrupted_before_verification_is_restarted_with_a_reason() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("z", "two.md", &["a"])]);
    repo.scenario(json!({"tickets":{"a":works("a.txt","first"),"z":works("z.txt","works")}}));
    repo.interrupted_run(&id, "integration.after_merge@a");
    let branch = format!("kiln/{id}/integration");
    let recorded = repo.inspect(&id);
    let interrupted = integration_attempts(&recorded, "a")[0].clone();
    assert_eq!(interrupted["status"], "running");

    let (out, run) = repo.resume(&id);
    assert_success(&out);
    let attempts = integration_attempts(&run, "a");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["status"], "interrupted");
    assert_eq!(attempts[1]["status"], "integrated");
    assert_eq!(
        sessions_for(&run, "a").len(),
        1,
        "reviewed implementation is preserved"
    );
    assert_eq!(repo.git(&["rev-list", "--count", "--merges", &branch]), "2");
    let restarted = decisions(&run, "a");
    assert!(
        restarted
            .iter()
            .any(|d| d["action"] == "restarted" && d["subject"] == interrupted["id"]),
        "{restarted:?}"
    );
}

#[test]
fn resume_preserves_specs_blockers_review_history_and_independent_progress() {
    let repo = Repo::new(Some(1));
    let id = repo.planned(&[
        ("a", "one.md", &[]),
        ("b", "two.md", &[]),
        ("c", "one.md", &["a"]),
    ]);
    let rejected = json!({"outcome":"rejected","findings":[{"code":"spec","message":"wrong","evidence":"Observed wrong output"}],"evidence":"Observed concrete change"});
    let b_rejected = json!({"implementation":{"files":{"b.txt":"wrong"},"outcome":"completed"},"review":{"standards":approved(),"spec":rejected}});
    repo.scenario(
        json!({"tickets":{"a":works("a.txt","first"),"b":b_rejected,"c":works("c.txt","works")}}),
    );
    repo.interrupted_run(&id, "integration.after_merge@c");
    let before = repo.inspect(&id);
    assert_eq!(ticket(&before, "a")["state"], "integrated");
    assert_eq!(ticket(&before, "b")["state"], "blocked");

    // Rerunning a or b would now produce different results.
    repo.scenario(json!({"tickets":{"a":works("a.txt","second"),"b":works("b.txt","fixed"),"c":works("c.txt","works")}}));
    let (out, run) = repo.resume(&id);
    assert!(
        !out.status.success(),
        "a preserved blocker keeps the run blocked"
    );
    assert_eq!(run["status"], "blocked");
    assert_eq!(run["specs"], before["specs"], "frozen specs are unchanged");
    assert_eq!(ticket(&run, "b")["state"], "blocked");
    assert_eq!(
        ticket(&run, "b")["blocker"],
        ticket(&before, "b")["blocker"]
    );
    assert_eq!(sessions_for(&run, "b").len(), 1);
    let review_ids = |r: &Value| -> Vec<Value> {
        r["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].clone())
            .collect()
    };
    for kept in review_ids(&before) {
        assert!(review_ids(&run).contains(&kept), "review {kept} preserved");
    }
    assert_eq!(sessions_for(&run, "a").len(), 1);
    assert_eq!(integration_attempts(&run, "a").len(), 1);
    assert_eq!(ticket(&run, "a")["state"], "integrated");
    assert_eq!(ticket(&run, "c")["state"], "integrated");
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    assert_eq!(repo.git(&["show", &format!("{tip}:a.txt")]), "first");
    let b_integrated = Command::new("git")
        .args(["cat-file", "-e", &format!("{tip}:b.txt")])
        .current_dir(&repo.path)
        .status()
        .unwrap();
    assert!(!b_integrated.success(), "blocked work is never integrated");
    for (t, action) in [("a", "preserved"), ("b", "preserved"), ("c", "restarted")] {
        assert!(
            decisions(&run, t).iter().any(|d| d["action"] == action),
            "{t}: {:?}",
            decisions(&run, t)
        );
    }
    assert!(decisions(&run, "b")
        .iter()
        .all(|d| d["action"] == "preserved"));
}

#[test]
fn integration_evidence_that_no_longer_matches_git_is_unable_to_verify_not_success() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &["a"])]);
    repo.scenario(json!({"tickets":{"a":works("a.txt","first"),"b":works("b.txt","works")}}));
    repo.interrupted_run(&id, "implementation.after_agent@b");
    let before = repo.inspect(&id);
    let attempt = integration_attempts(&before, "a")[0].clone();
    assert_eq!(attempt["status"], "integrated");
    // The integration branch is rewound outside Kiln while no process is running.
    let branch = format!("refs/heads/kiln/{id}/integration");
    repo.git(&[
        "update-ref",
        &branch,
        attempt["base_commit"].as_str().unwrap(),
    ]);

    let (out, run) = repo.resume(&id);
    assert!(!out.status.success());
    assert_eq!(run["status"], "blocked");
    assert_eq!(ticket(&run, "a")["state"], "blocked");
    assert!(ticket(&run, "a")["blocker"]
        .as_str()
        .unwrap()
        .contains("unable to verify"));
    assert_ne!(integration_attempts(&run, "a")[0]["status"], "integrated");
    assert!(sessions_for(&run, "b")
        .iter()
        .all(|s| s["status"] != "integrated"));
    assert!(
        decisions(&run, "a")
            .iter()
            .any(|d| d["action"] == "unable-to-verify"),
        "{:?}",
        decisions(&run, "a")
    );
}

#[test]
fn resume_refuses_a_run_whose_scheduler_process_is_still_alive() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("z", "two.md", &["a"])]);
    let mut a = works("a.txt", "first");
    a["await_file"] = json!(".kiln/release");
    repo.scenario(json!({"tickets":{"a":a,"z":works("z.txt","works")}}));
    let mut child = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(["run", &id, "--fixture", "scenario.json"])
        .current_dir(&repo.path)
        .env_remove("KILN_FAULT_INJECT")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        assert!(std::time::Instant::now() < deadline, "run never started a");
        let run = repo.inspect(&id);
        if run["scheduler"]["tickets"].is_array()
            && ticket(&run, "a")["state"] == "implementing"
            && sessions_for(&run, "a")
                .last()
                .is_some_and(|session| session["status"] == "running")
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = repo.cli(&["resume", &id, "--fixture", "scenario.json"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("active in another process"));
    let observed = repo.inspect(&id);
    assert_eq!(
        sessions_for(&observed, "a")[0]["status"],
        "running",
        "live work is not reclassified"
    );
    assert!(observed["recoveries"].as_array().unwrap().is_empty());
    fs::write(repo.path.join(".kiln/release"), "go").unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn limit_stopped_ticket_resumes_in_a_fresh_session_and_completes() {
    let repo = Repo::new(None);
    // Duration budget applies per invocation; a's first session is cancelled by it.
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(repo.path.join("kiln.json")).unwrap()).unwrap();
    config["duration_limit_seconds"] = json!(3);
    config["limit_policy"] = json!("stop");
    fs::write(repo.path.join("kiln.json"), config.to_string()).unwrap();
    repo.git(&["commit", "-qam", "limits"]);
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &["a"])]);
    let mut a = works("a.txt", "first");
    a["await_file"] = json!(".kiln/release");
    repo.scenario(json!({"tickets":{"a":a,"b":works("b.txt","works")}}));
    let out = repo.cli(&["run", &id, "--fixture", "scenario.json"]);
    assert!(!out.status.success());
    let stopped = repo.inspect(&id);
    assert_eq!(stopped["status"], "limit_exhausted");
    assert_eq!(ticket(&stopped, "a")["state"], "stopped");
    let cancelled = sessions_for(&stopped, "a")[0]["id"].clone();

    fs::write(repo.path.join(".kiln/release"), "go").unwrap();
    let (out, run) = repo.resume(&id);
    assert_success(&out);
    assert_eq!(run["status"], "awaiting_validation");
    let sessions = sessions_for(&run, "a");
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["status"], "interrupted");
    assert_eq!(sessions[1]["status"], "integrated");
    assert_eq!(
        run["corrections"].as_array().unwrap().len(),
        0,
        "a stop is not a failure to correct"
    );
    assert!(
        decisions(&run, "a")
            .iter()
            .any(|d| d["action"] == "restarted"
                && d["subject"] == cancelled
                && d["reason"].as_str().unwrap().contains("limit")),
        "{:?}",
        decisions(&run, "a")
    );
}
