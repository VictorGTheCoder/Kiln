//! Run limits through the CLI boundary: per-ticket correction exhaustion,
//! run-wide duration/usage/cost exhaustion, and inspection of preserved state.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{Duration, Instant},
};

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    /// One approved spec; `extra` keys are merged into the project configuration.
    fn new(extra: Value) -> Self {
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
        fs::write(repo.path.join(".gitignore"), ".kiln/\n").unwrap();
        let mut config = json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}});
        for (k, v) in extra.as_object().unwrap() {
            config[k] = v.clone();
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
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
    /// `tickets` are (id, blocked_by).
    fn planned(&self, tickets: &[(&str, &[&str])]) -> String {
        let prepared = self.cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
        assert!(
            prepared.status.success(),
            "{}",
            String::from_utf8_lossy(&prepared.stderr)
        );
        let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
        let id = prepared["id"].as_str().unwrap().to_owned();
        let plan_tickets: Vec<Value> = tickets
            .iter()
            .map(|(name, blockers)| json!({"id":name,"title":name,"description":"Deliver","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":blockers}))
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
    fn run(&self, id: &str, scenario: Value) -> (Output, Value) {
        fs::write(self.path.join("scenario.json"), scenario.to_string()).unwrap();
        let out = self.cli(&["run", id, "--fixture", "scenario.json"]);
        let run = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("no run state: {}", String::from_utf8_lossy(&out.stderr)));
        (out, run)
    }
    fn inspect(&self, id: &str) -> Value {
        let out = self.cli(&["inspect", id]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
fn approved() -> Value {
    json!({"outcome":"approved","findings":[],"evidence":"Observed concrete change"})
}
fn rejected() -> Value {
    json!({"outcome":"rejected","findings":[{"code":"wrong","message":"Still incorrect","evidence":"x","required":true}],"evidence":"Observed change"})
}
/// A ticket that writes `file` and passes review; `log` is the provider observation.
fn works(file: &str, log: Value) -> Value {
    json!({"implementation":{"files":{file:"works"},"outcome":"completed","log":log.to_string()},"review":{"standards":approved(),"spec":approved()}})
}
fn usage(tokens: u64, cost: Value) -> Value {
    json!({"usage":{"input_tokens":tokens,"output_tokens":0},"cost":cost})
}
/// Rejected implementation whose every correction changes content but stays rejected.
fn never_corrected(cycles: usize) -> Value {
    let corrections: Vec<Value> = (0..cycles)
        .map(|n| json!({"files":{"bad.txt":format!("attempt {n}")},"outcome":"completed"}))
        .collect();
    let reviews: Vec<Value> = (0..cycles)
        .map(|_| json!({"standards":approved(),"spec":rejected()}))
        .collect();
    json!({"implementation":{"files":{"bad.txt":"wrong"},"outcome":"completed"},
           "review":{"standards":approved(),"spec":rejected()},
           "corrections":{"corrections":corrections,"reviews":reviews}})
}
fn ticket<'a>(run: &'a Value, id: &str) -> &'a Value {
    run["scheduler"]["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}
fn count(run: &Value, field: &str, ticket: &str) -> usize {
    run[field]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["ticket_id"] == ticket)
        .count()
}

#[test]
fn ticket_correction_exhaustion_blocks_only_that_ticket_with_default_three_cycles() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[("a", &[]), ("b", &[])]);
    let (out, run) = repo.run(
        &id,
        json!({"tickets":{"a":never_corrected(5),"b":works("b.txt", json!(null))}}),
    );
    assert!(
        !out.status.success(),
        "an exhausted ticket must not report success"
    );
    assert_eq!(run["status"], "blocked");
    // Defaults: three correction cycles per ticket, three concurrent implementations,
    // and no invented monetary ceiling.
    let limits = &run["scheduler"]["limits"];
    assert_eq!(limits["correction_cycles"], 3);
    assert_eq!(run["scheduler"]["implementation_concurrency"], 3);
    assert!(limits["cost_limit_usd"].is_null());
    assert_eq!(count(&run, "corrections", "a"), 3);
    assert_eq!(ticket(&run, "a")["state"], "blocked");
    assert_eq!(ticket(&run, "a")["exhaustion"], "correction_cycles");
    // Run-wide resources were not exhausted, so later replanning may still apply.
    assert!(limits["exhausted"].is_null());
    assert_eq!(ticket(&run, "b")["state"], "integrated");
}

#[test]
fn configured_correction_cycles_bound_each_ticket() {
    let repo = Repo::new(json!({"correction_cycles": 1}));
    let id = repo.planned(&[("a", &[])]);
    let (out, run) = repo.run(&id, json!({"tickets":{"a":never_corrected(5)}}));
    assert!(!out.status.success());
    assert_eq!(run["scheduler"]["limits"]["correction_cycles"], 1);
    assert_eq!(count(&run, "corrections", "a"), 1);
    assert_eq!(ticket(&run, "a")["exhaustion"], "correction_cycles");
}

#[test]
fn usage_limit_stops_further_work_and_preserves_resumable_state() {
    let repo = Repo::new(json!({"usage_token_limit": 500}));
    let id = repo.planned(&[("a", &[]), ("b", &["a"])]);
    let scenario = json!({"tickets":{"a":works("a.txt", usage(700, json!(null))),"b":works("b.txt", json!(null))}});
    let (out, run) = repo.run(&id, scenario.clone());
    assert!(
        !out.status.success(),
        "a stopped run must not report success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage_token_limit"), "{stderr}");
    assert_eq!(run["status"], "limit_exhausted");
    let exhausted = &run["scheduler"]["limits"]["exhausted"];
    assert_eq!(exhausted["limit"], "usage_tokens");
    assert_eq!(run["scheduler"]["limits"]["usage"]["tokens"], 700);
    // The settled session is preserved; no further work was started.
    assert_eq!(count(&run, "sessions", "a"), 1);
    assert_eq!(count(&run, "reviews", "a"), 0);
    assert_eq!(count(&run, "sessions", "b"), 0);
    assert_eq!(ticket(&run, "a")["state"], "stopped");
    assert!(ticket(&run, "a")["exhaustion"].is_null());
    assert_eq!(ticket(&run, "b")["state"], "waiting");
    assert!(
        ticket(&run, "b")["blocker"].is_null(),
        "b is resumable, not blocked"
    );

    let inspected = repo.inspect(&id);
    assert_eq!(inspected["status"], "limit_exhausted");
    assert_eq!(inspected["scheduler"]["limits"]["exhausted"], *exhausted);
    assert_eq!(inspected["sessions"], run["sessions"]);

    // Repeated invocation cannot reset the usage allowance.
    let (again, rerun) = repo.run(&id, scenario);
    assert!(!again.status.success());
    assert_eq!(rerun["status"], "limit_exhausted");
    assert_eq!(count(&rerun, "sessions", "b"), 0);
}

#[test]
fn duration_limit_under_stop_policy_cancels_the_active_session() {
    let repo = Repo::new(json!({"duration_limit_seconds": 1, "limit_policy": "stop"}));
    let id = repo.planned(&[("a", &[]), ("b", &["a"])]);
    // a's agent waits for a release marker that never appears (30s fixture timeout).
    let mut a = works("a.txt", json!(null));
    a["await_file"] = json!("never-released");
    let started = Instant::now();
    let (out, run) = repo.run(
        &id,
        json!({"tickets":{"a":a,"b":works("b.txt", json!(null))}}),
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the active session was not stopped"
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("duration"));
    assert_eq!(run["status"], "limit_exhausted");
    assert_eq!(run["scheduler"]["limits"]["exhausted"]["limit"], "duration");
    assert_eq!(run["scheduler"]["limits"]["limit_policy"], "stop");
    assert_eq!(ticket(&run, "a")["state"], "stopped");
    // The cancelled session's evidence is preserved and costs no correction cycle.
    assert_eq!(count(&run, "sessions", "a"), 1);
    assert_eq!(count(&run, "corrections", "a"), 0);
    assert_eq!(count(&run, "sessions", "b"), 0);
    assert_eq!(ticket(&run, "b")["state"], "waiting");
}

#[test]
fn duration_stop_cancels_project_checks_and_kills_their_descendants() {
    const SLOW_CHECK: [&str; 3] = [
        "sh",
        "-c",
        "mkdir -p .kiln; (sleep 3; touch .kiln/late-child) & wait",
    ];
    let repo = Repo::new(json!({
        "duration_limit_seconds": 1,
        "limit_policy": "stop",
        "build": SLOW_CHECK,
        "isolation": {
            "network": "none",
            "runtime": "system",
            "commands": [["git", "diff", "--check"], ["git", "--version"], SLOW_CHECK]
        }
    }));
    let id = repo.planned(&[("a", &[])]);
    let started = Instant::now();
    let (out, run) = repo.run(&id, json!({"tickets":{"a":works("a.txt", json!(null))}}));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "stop policy waited for project check"
    );
    assert!(!out.status.success());
    assert_eq!(run["status"], "limit_exhausted");
    assert_eq!(ticket(&run, "a")["state"], "stopped");
    assert_eq!(run["sessions"][0]["status"], "interrupted");
    let worktree = PathBuf::from(run["sessions"][0]["worktree"].as_str().unwrap());
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !worktree.join(".kiln/late-child").exists(),
        "a child process outlived the stopped check"
    );

    // Resume with the same durable run after the run-wide timeout has reset.
    let mut resumable = run;
    resumable["config"]["duration_limit_seconds"] = json!(10);
    resumable["config"]["build"] = json!(["git", "diff", "--check"]);
    fs::write(
        repo.path.join(".kiln/runs").join(format!("{id}.json")),
        serde_json::to_vec_pretty(&resumable).unwrap(),
    )
    .unwrap();
    let resumed = repo.cli(&["resume", &id, "--fixture", "scenario.json"]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["status"], "awaiting_validation");
    assert_eq!(count(&resumed, "sessions", "a"), 2);
    assert_eq!(resumed["sessions"][0]["status"], "interrupted");
}

#[test]
fn invalid_limit_configuration_is_rejected_before_launch() {
    for bad in [
        json!({"duration_limit_seconds": 0}),
        json!({"usage_token_limit": "many"}),
        json!({"cost_limit_usd": -1}),
        json!({"limit_policy": "abandon"}),
    ] {
        let repo = Repo::new(bad.clone());
        let out = repo.cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
        assert!(!out.status.success(), "{bad} accepted");
    }
}

#[test]
fn unavailable_or_estimated_cost_is_identified_and_never_enforced() {
    for (cost_field, data, ceiling) in [
        (
            json!({"cost": null}),
            "unavailable",
            "not_enforced_cost_unavailable",
        ),
        (
            json!({"cost": null, "cost_estimate": 5.0}),
            "estimated",
            "not_enforced_estimate_only",
        ),
    ] {
        let repo = Repo::new(json!({"cost_limit_usd": 0.01}));
        let id = repo.planned(&[("a", &[])]);
        let mut log = usage(100, json!(null));
        for (k, v) in cost_field.as_object().unwrap() {
            log[k] = v.clone();
        }
        let (out, run) = repo.run(&id, json!({"tickets":{"a":works("a.txt", log)}}));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(run["status"], "awaiting_validation");
        let limits = &repo.inspect(&id)["scheduler"]["limits"];
        assert_eq!(limits["usage"]["cost_data"], data);
        assert!(
            limits["usage"]["measured_cost_usd"].is_null(),
            "unavailable cost is not zero"
        );
        assert_eq!(limits["cost_ceiling"], ceiling);
        assert!(limits["exhausted"].is_null());
        if data == "estimated" {
            assert_eq!(limits["usage"]["estimated_cost_usd"], 5.0);
        }
    }
}

#[test]
fn measured_cost_reaching_the_ceiling_stops_the_run() {
    let repo = Repo::new(json!({"cost_limit_usd": 0.5}));
    let id = repo.planned(&[("a", &[]), ("b", &["a"])]);
    let (out, run) = repo.run(
        &id,
        json!({"tickets":{"a":works("a.txt", usage(10, json!(0.75))),"b":works("b.txt", json!(null))}}),
    );
    assert!(!out.status.success());
    assert_eq!(run["status"], "limit_exhausted");
    let limits = &run["scheduler"]["limits"];
    assert_eq!(limits["exhausted"]["limit"], "cost");
    assert_eq!(limits["usage"]["measured_cost_usd"], 0.75);
    assert_eq!(count(&run, "sessions", "b"), 0);
}
