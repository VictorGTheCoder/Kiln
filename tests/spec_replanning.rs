//! Explicit replanning when approved specs change, through the CLI boundary:
//! frozen inputs never follow external edits; `kiln replan` captures revised specs
//! as a new input version, invalidates only affected work and dependent evidence,
//! and keeps unaffected work with its provenance.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const ONE: &str = "# One\n## Acceptance criteria\n- Greets in English\n";
const ONE_REVISED: &str = "# One\n## Acceptance criteria\n- Greets in French\n";
const TWO: &str = "# Two\n## Acceptance criteria\n- Counts visitors\n";

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let repo = Self { _temp: temp, path };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        fs::write(repo.path.join("one.md"), ONE).unwrap();
        fs::write(repo.path.join("two.md"), TWO).unwrap();
        fs::write(repo.path.join(".gitignore"), ".kiln/\n").unwrap();
        let config = json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}});
        fs::write(repo.path.join("kiln.json"), config.to_string()).unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(&self.path).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.cli(args);
        assert!(out.status.success(), "kiln {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn write(&self, name: &str, value: Value) {
        fs::write(self.path.join(name), value.to_string()).unwrap();
    }
    /// a covers one.md; b and c cover two.md; c depends on a.
    fn planned(&self) -> String {
        let prepared = self.ok(&["prepare", "--config", "kiln.json", "--spec", "one.md", "--spec", "two.md"]);
        let id = prepared["id"].as_str().unwrap().to_owned();
        self.write("plan.json", json!({"tickets":[
            ticket("a", "Greet", &["one.md#ac-1"], &[]),
            ticket("b", "Count", &["two.md#ac-1"], &[]),
            ticket("c", "Count greetings", &["two.md#ac-1"], &["a"])],
            "verification":{"outcome":"verified","findings":[]}}));
        self.ok(&["plan", &id, "--fixture", "plan.json"]);
        id
    }
    fn run(&self, id: &str, scenario: Value) -> (Output, Value) {
        self.write("scenario.json", scenario);
        let out = self.cli(&["run", id, "--fixture", "scenario.json"]);
        let run = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("no run state: {}", String::from_utf8_lossy(&out.stderr)));
        (out, run)
    }
    /// Plan and integrate every ticket under the original specs.
    fn integrated(&self) -> (String, Value) {
        let id = self.planned();
        let (out, run) = self.run(&id, json!({"tickets":{
            "a":works("a.txt","hello"),"b":works("b.txt","count"),"c":works("c.txt","count greetings")}}));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(run["status"], "awaiting_validation");
        (id, run)
    }
    fn replan(&self, id: &str, specs: &[&str], fixture: Value) -> (Output, Value) {
        self.write("replan.json", fixture);
        let mut args = vec!["replan", id];
        for spec in specs {
            args.extend(["--spec", spec]);
        }
        args.extend(["--fixture", "replan.json"]);
        let out = self.cli(&args);
        let run = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, run)
    }
}
fn ticket(id: &str, title: &str, covers: &[&str], blocked_by: &[&str]) -> Value {
    json!({"id":id,"title":title,"description":"Deliver","acceptance_criteria":["works"],"covers":covers,"blocked_by":blocked_by})
}
fn approved() -> Value {
    json!({"outcome":"approved","findings":[],"evidence":"Observed concrete change"})
}
fn works(file: &str, content: &str) -> Value {
    json!({"implementation":{"files":{file:content},"outcome":"completed"},"review":{"standards":approved(),"spec":approved()}})
}
/// The revised ticket `a` for the French requirement, independently verified.
fn revise_a(verification: &str) -> Value {
    json!({"tickets":[{"id":"a","title":"Greet","description":"Greet in French","acceptance_criteria":["bonjour"],"covers":["one.md#ac-1"],"blocked_by":[]}],
           "verification":{"outcome":verification,"findings":[]}})
}
fn sessions<'a>(run: &'a Value, ticket: &str) -> Vec<&'a Value> {
    run["sessions"].as_array().unwrap().iter().filter(|s| s["ticket_id"] == ticket).collect()
}
fn strings(value: &Value) -> Vec<&str> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect()
}

#[test]
fn external_spec_edits_do_not_mutate_frozen_inputs() {
    let repo = Repo::new();
    let (id, before) = repo.integrated();
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    repo.git(&["commit", "-qam", "edit spec"]);
    let after = repo.ok(&["inspect", &id]);
    assert_eq!(after["specs"], before["specs"]);
    assert_eq!(after["specs"][0]["content"], ONE);
    assert!(after["spec_revisions"].as_array().unwrap().is_empty());
    assert_eq!(after["plan"], before["plan"]);
    // Running again neither picks up the edit nor redoes integrated work.
    let (out, rerun) = repo.run(&id, json!({"tickets":{}}));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(rerun["sessions"].as_array().unwrap().len(), 3);
    assert_eq!(rerun["specs"][0]["content"], ONE);
}

#[test]
fn replanning_captures_revised_specs_and_identifies_affected_work() {
    let repo = Repo::new();
    let (id, before) = repo.integrated();
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    let (out, run) = repo.replan(&id, &["one.md", "two.md"], revise_a("verified"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Frozen inputs stay; the revision is a new versioned input.
    assert_eq!(run["specs"][0]["content"], ONE);
    let revisions = run["spec_revisions"].as_array().unwrap();
    assert_eq!(revisions.len(), 1, "only changed specs are revised: {revisions:?}");
    assert_eq!(revisions[0]["version"], 1);
    assert_eq!(revisions[0]["path"], "one.md");
    assert_eq!(revisions[0]["content"], ONE_REVISED);
    assert_eq!(revisions[0]["status"], "verified");
    let replan = &run["spec_replans"][0];
    assert_eq!(replan["outcome"], "replanned");
    assert_eq!(replan["input_version"], 1);
    assert_eq!(replan["previous_input_version"], 0);
    assert_eq!(strings(&replan["changed_requirements"]), ["one.md#ac-1"]);
    assert_eq!(strings(&replan["affected_tickets"]), ["a"]);
    assert_eq!(strings(&replan["dependent_tickets"]), ["c"]);
    let a_session = sessions(&before, "a")[0]["id"].as_str().unwrap();
    assert_eq!(strings(&replan["invalidated_sessions"]), [a_session]);
    assert_eq!(replan["invalidated_integrations"].as_array().unwrap().len(), 1);
    // Old and new decisions remain inspectable; the revised plan was independently checked.
    assert_eq!(replan["previous_plan"], before["plan"]);
    assert_ne!(replan["context_id"], replan["verification"]["verification_context"]);
    assert_eq!(replan["verification"]["executable"], true);
    let a = run["plan"]["tickets"].as_array().unwrap().iter().find(|t| t["id"] == "a").unwrap();
    assert_eq!(a["description"], "Greet in French");
    assert_eq!(run["plan"]["requirements"][0]["criterion"], "Greets in French");
    assert_eq!(run["status"], "replanned");
    assert_eq!(repo.ok(&["inspect", &id])["spec_replans"], run["spec_replans"]);
}

#[test]
fn a_revised_plan_failing_independent_verification_changes_nothing() {
    let repo = Repo::new();
    let (id, before) = repo.integrated();
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    let (out, run) = repo.replan(&id, &["one.md"], revise_a("failed"));
    assert!(!out.status.success(), "a rejected replan must not report success");
    let replan = &run["spec_replans"][0];
    assert_eq!(replan["outcome"], "rejected");
    assert!(replan["findings"].as_array().unwrap().iter().any(|f| f["code"] == "verification_not_verified"));
    assert_eq!(replan["input_version"], 0);
    assert!(replan["invalidated_sessions"].as_array().unwrap().is_empty());
    assert_eq!(run["spec_revisions"][0]["status"], "rejected", "the rejected revision stays inspectable");
    assert_eq!(run["plan"], before["plan"]);
    assert_eq!(run["sessions"], before["sessions"]);
    assert_eq!(run["status"], "awaiting_validation");
}

#[test]
fn replanning_requires_a_changed_approved_input_of_the_run() {
    let repo = Repo::new();
    let (id, _) = repo.integrated();
    let (out, _) = repo.replan(&id, &["one.md"], revise_a("verified"));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no approved spec changed"));
    fs::write(repo.path.join("three.md"), "# Three\n## Acceptance criteria\n- New\n").unwrap();
    let (out, _) = repo.replan(&id, &["three.md"], revise_a("verified"));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an approved input"));
    assert!(repo.ok(&["inspect", &id])["spec_replans"].as_array().unwrap().is_empty());
}

#[test]
fn after_integration_a_changed_spec_reexecutes_only_affected_work_under_the_new_input_version() {
    let repo = Repo::new();
    let (id, before) = repo.integrated();
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    repo.git(&["commit", "-qam", "approve French greeting"]);
    let (out, _) = repo.replan(&id, &["one.md"], revise_a("verified"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Only the affected ticket has a response: anything else re-executing would fail.
    let (out, run) = repo.run(&id, json!({"tickets":{"a":works("a.txt","bonjour")}}));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(run["status"], "awaiting_validation");

    // Affected work: old evidence invalidated, new session under input version 1.
    let a = sessions(&run, "a");
    assert_eq!(a.len(), 2);
    assert_eq!(a[0]["status"], "superseded");
    assert_eq!(a[0]["input_version"], 0);
    assert_eq!(a[1]["status"], "integrated");
    assert_eq!(a[1]["input_version"], 1);
    let context = fs::read_to_string(repo.path.join(".kiln/contexts").join(format!("{}.json", a[1]["id"].as_str().unwrap()))).unwrap();
    assert!(context.contains("Greets in French") && !context.contains("Greets in English"), "{context}");
    let a_integrations: Vec<&Value> = run["integrations"].as_array().unwrap().iter().filter(|i| i["ticket_id"] == "a").collect();
    assert_eq!(a_integrations.iter().map(|i| i["status"].as_str().unwrap()).collect::<Vec<_>>(), ["superseded", "integrated"]);

    // Unaffected independent work is retained with its provenance.
    assert_eq!(sessions(&run, "b"), sessions(&before, "b"));
    let replan = &run["spec_replans"][0];
    let retained = replan["retained"].as_array().unwrap();
    let b = retained.iter().find(|r| r["ticket_id"] == "b").unwrap();
    assert_eq!(b["session_id"], sessions(&before, "b")[0]["id"]);
    assert_eq!(b["commit"], sessions(&before, "b")[0]["commit"]);
    assert_eq!(b["input_version"], 0);

    // Dependent work is kept, but its evidence is revalidated against the re-integrated prerequisite.
    assert_eq!(sessions(&run, "c"), sessions(&before, "c"), "dependent work is not re-implemented");
    let revalidation = replan["revalidations"].as_array().unwrap().iter().find(|r| r["ticket_id"] == "c").unwrap();
    assert_eq!(revalidation["outcome"], "revalidated");
    assert_eq!(revalidation["input_version"], 1);
    let tip = repo.git(&["rev-parse", run["integration_branch"].as_str().unwrap()]);
    assert_eq!(revalidation["verified_commit"], tip.as_str());
    let new_a = a_integrations[1]["integrated_commit"].as_str().unwrap();
    repo.git(&["merge-base", "--is-ancestor", new_a, &tip]);
    assert!(revalidation["checks"].as_array().unwrap().iter().all(|c| c["passed"] == true));
    let c = run["scheduler"]["tickets"].as_array().unwrap().iter().find(|t| t["id"] == "c").unwrap();
    assert_eq!(c["state"], "integrated");
}

#[test]
fn active_affected_sessions_are_superseded_so_stale_results_cannot_be_integrated() {
    let repo = Repo::new();
    let id = repo.planned();
    for (ticket, file) in [("a", "a.txt"), ("b", "b.txt")] {
        repo.write("implement.json", json!({"files":{file:"old"},"outcome":"completed"}));
        repo.ok(&["implement", &id, ticket, "--fixture", "implement.json"]);
        repo.write("review.json", json!({"standards":approved(),"spec":approved()}));
        repo.ok(&["review", &id, ticket, "--fixture", "review.json"]);
    }
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    let (out, run) = repo.replan(&id, &["one.md"], revise_a("verified"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let a = sessions(&run, "a");
    assert_eq!(a[0]["status"], "superseded");
    assert_eq!(strings(&run["spec_replans"][0]["invalidated_sessions"]), [a[0]["id"].as_str().unwrap()]);
    assert_eq!(sessions(&run, "b")[0]["status"], "implemented", "unaffected active work is kept");
    let integrate = repo.cli(&["integrate", &id, "a"]);
    assert!(!integrate.status.success(), "a stale result must not be integrated");
    let after = repo.ok(&["inspect", &id]);
    assert!(after["integrations"].as_array().unwrap().iter().all(|i| i["ticket_id"] != "a"));
    repo.ok(&["integrate", &id, "b"]);
}

#[test]
fn replanning_waits_for_a_live_scheduler_to_stop() {
    let repo = Repo::new();
    let id = repo.planned();
    let mut a = works("a.txt", "hello");
    a["await_file"] = json!("release");
    repo.write("scenario.json", json!({"tickets":{"a":a,"b":works("b.txt","count"),"c":works("c.txt","greetings")}}));
    let mut child = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(["run", &id, "--fixture", "scenario.json"])
        .current_dir(&repo.path)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !repo.ok(&["inspect", &id])["sessions"].as_array().unwrap().iter().any(|s| s["ticket_id"] == "a" && s["status"] == "running") {
        assert!(std::time::Instant::now() < deadline, "session a never started");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    let (out, _) = repo.replan(&id, &["one.md"], revise_a("verified"));
    fs::write(repo.path.join("release"), "").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(!out.status.success(), "replanning must not race a live scheduler");
    assert!(String::from_utf8_lossy(&out.stderr).contains("active in another process"));
    let (out, run) = repo.replan(&id, &["one.md"], revise_a("verified"));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(strings(&run["spec_replans"][0]["affected_tickets"]), ["a"]);
}

/// Engine seam: a replan that lands while an affected implementation session is
/// still in flight must not let that session's stale result become integrable.
struct ReplanMidSession {
    engine: kiln::Engine,
    run: String,
    replan: PathBuf,
}
impl kiln::execution::ImplementationAgent for ReplanMidSession {
    fn implement(&self, request: &kiln::execution::ImplementationRequest) -> anyhow::Result<kiln::execution::AgentResult> {
        fs::write(request.worktree.join("a.txt"), "hello").unwrap();
        let agent = kiln::spec_replanning::FixtureSpecReplanning::load(&self.replan)?;
        let run = self.engine.replan_specs(&self.run, &[PathBuf::from("one.md")], &agent)?;
        assert_eq!(run.spec_replans[0].outcome, "replanned");
        Ok(kiln::execution::AgentResult { outcome: "completed".into(), log: String::new() })
    }
}

#[test]
fn a_session_finishing_after_a_replan_stays_superseded() {
    let repo = Repo::new();
    let id = repo.planned();
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    repo.write("replan.json", revise_a("verified"));
    let engine = kiln::Engine::open(&repo.path).unwrap();
    let agent = ReplanMidSession { engine: engine.clone(), run: id.clone(), replan: repo.path.join("replan.json") };
    let run = engine.implement_ticket(&id, "a", &agent).unwrap();
    let session = run.sessions.iter().find(|s| s.ticket_id == "a").unwrap();
    assert_eq!(session.status, "superseded", "the late result must not revive stale work");
    assert!(session.commit.is_some(), "the late result stays inspectable");
    assert!(engine.integrate_ticket(&id, "a", None).is_err());
}

struct ReplanMidCorrection(ReplanMidSession);
impl kiln::correction::CorrectionAgent for ReplanMidCorrection {
    fn correct(&self, request: &kiln::correction::CorrectionRequest) -> anyhow::Result<kiln::execution::AgentResult> {
        fs::write(request.worktree.join("a.txt"), "corrected").unwrap();
        let agent = kiln::spec_replanning::FixtureSpecReplanning::load(&self.0.replan)?;
        self.0.engine.replan_specs(&self.0.run, &[PathBuf::from("one.md")], &agent)?;
        Ok(kiln::execution::AgentResult { outcome: "completed".into(), log: String::new() })
    }
}

#[test]
fn a_correction_finishing_after_a_replan_cannot_be_integrated() {
    let repo = Repo::new();
    let id = repo.planned();
    let engine = kiln::Engine::open(&repo.path).unwrap();
    repo.write("implement.json", json!({"files":{"a.txt":"hello"},"outcome":"completed"}));
    repo.ok(&["implement", &id, "a", "--fixture", "implement.json"]);
    repo.write("rejected.json", json!({"standards":approved(),"spec":{"outcome":"rejected","findings":[{"code":"wrong","message":"No","evidence":"x","required":true}],"evidence":"Observed"}}));
    let _ = repo.cli(&["review", &id, "a", "--fixture", "rejected.json"]);
    fs::write(repo.path.join("one.md"), ONE_REVISED).unwrap();
    repo.write("replan.json", revise_a("verified"));
    repo.write("review.json", json!({"standards":approved(),"spec":approved()}));
    let reviewer = kiln::review::FixtureReviewAgent::load(&repo.path.join("review.json")).unwrap();
    let corrector = ReplanMidCorrection(ReplanMidSession { engine: engine.clone(), run: id.clone(), replan: repo.path.join("replan.json") });
    let _ = engine.correct_ticket(&id, "a", &corrector, &reviewer);
    let run = engine.inspect(&id).unwrap();
    assert_eq!(run.spec_replans.len(), 1);
    assert!(run.sessions.iter().filter(|s| s.ticket_id == "a").all(|s| s.status == "superseded"), "{:?}",
        run.sessions.iter().map(|s| &s.status).collect::<Vec<_>>());
    assert!(engine.integrate_ticket(&id, "a", None).is_err());
}
