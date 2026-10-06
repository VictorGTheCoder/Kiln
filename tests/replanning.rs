//! Bounded autonomous replanning through the CLI boundary: a ticket that exhausts
//! its correction cycles receives at most one independently verified replanning
//! attempt; persistent failure becomes a visible blockage of only its descendants.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
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
        fs::write(repo.path.join("one.md"), "# One\n## Acceptance criteria\n- Works\n").unwrap();
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
    /// `tickets` are (id, blocked_by).
    fn planned(&self, tickets: &[(&str, &[&str])]) -> String {
        let prepared = self.cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
        assert!(prepared.status.success(), "{}", String::from_utf8_lossy(&prepared.stderr));
        let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
        let id = prepared["id"].as_str().unwrap().to_owned();
        let plan_tickets: Vec<Value> = tickets
            .iter()
            .map(|(name, blockers)| json!({"id":name,"title":name,"description":"Deliver","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":blockers}))
            .collect();
        fs::write(
            self.path.join("plan.json"),
            json!({"tickets":plan_tickets,"verification":{"outcome":"verified","findings":[]}}).to_string(),
        )
        .unwrap();
        let planned = self.cli(&["plan", &id, "--fixture", "plan.json"]);
        assert!(planned.status.success(), "{}", String::from_utf8_lossy(&planned.stderr));
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
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
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
    let reviews: Vec<Value> = (0..cycles).map(|_| json!({"standards":approved(),"spec":rejected()})).collect();
    json!({"implementation":{"files":{"bad.txt":"wrong"},"outcome":"completed"},
           "review":{"standards":approved(),"spec":rejected()},
           "corrections":{"corrections":corrections,"reviews":reviews}})
}
fn ticket<'a>(run: &'a Value, id: &str) -> &'a Value {
    run["scheduler"]["tickets"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()
}
fn count(run: &Value, field: &str, ticket: &str) -> usize {
    run[field].as_array().unwrap().iter().filter(|s| s["ticket_id"] == ticket).count()
}

/// One replanning attempt for `id`: the revised ticket, its independent verification,
/// and the deterministic implementation and review of the replanned work.
fn replanning(id: &str, verification: &str, implementation: &str, review: Value) -> Value {
    json!({"ticket":{"id":id,"title":id,"description":"Deliver by a smaller revised approach","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":[]},
           "verification":{"outcome":verification,"findings":[]},
           "implementation":{"files":{implementation:"works"},"outcome":"completed"},
           "review":{"standards":approved(),"spec":review}})
}
fn exhausting_then(replan: Value) -> Value {
    let mut t = never_corrected(5);
    t["replanning"] = replan;
    t
}
fn replans<'a>(run: &'a Value, ticket: &str) -> Vec<&'a Value> {
    run["replans"].as_array().unwrap().iter().filter(|r| r["ticket_id"] == ticket).collect()
}

#[test]
fn exhausted_corrections_receive_one_successful_replanning_attempt() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[("a", &[]), ("c", &["a"])]);
    let (out, run) = repo.run(&id, json!({"tickets":{
        "a":exhausting_then(replanning("a","verified","good.txt",approved())),
        "c":works("c.txt", json!(null))}}));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(run["status"], "awaiting_validation");
    assert_eq!(run["scheduler"]["limits"]["replanning_attempts"], 1);
    assert_eq!(count(&run, "corrections", "a"), 3, "replanning follows the exhausted correction cycles");
    let r = replans(&run, "a");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["attempt"], 1);
    assert_eq!(r[0]["outcome"], "replanned");
    assert_eq!(r[0]["result"], "integrated");
    assert_eq!(r[0]["previous"]["description"], "Deliver");
    assert!(!r[0]["failures"].as_array().unwrap().is_empty(), "the replanning context carries the unresolved findings");
    assert_ne!(r[0]["context_id"], r[0]["verification"]["verification_context"]);
    let plan_ticket = run["plan"]["tickets"].as_array().unwrap().iter().find(|t| t["id"] == "a").unwrap();
    assert_eq!(plan_ticket["description"], "Deliver by a smaller revised approach");
    let sessions: Vec<&str> = run["sessions"].as_array().unwrap().iter().filter(|s| s["ticket_id"] == "a").map(|s| s["status"].as_str().unwrap()).collect();
    assert_eq!(sessions, ["superseded", "integrated"]);
    assert_eq!(ticket(&run, "a")["state"], "integrated");
    assert_eq!(ticket(&run, "c")["state"], "integrated", "the replanned ticket releases its descendants");
}

#[test]
fn persistent_failure_after_replanning_blocks_only_descendants_without_retrying_forever() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[("a", &[]), ("b", &[]), ("c", &["a"])]);
    let (out, run) = repo.run(&id, json!({"tickets":{
        "a":exhausting_then(replanning("a","verified","still-bad.txt",rejected())),
        "b":works("b.txt", json!(null)),
        "c":works("c.txt", json!(null))}}));
    assert!(!out.status.success(), "persistent failure must not report success");
    assert_eq!(run["status"], "blocked");
    let a = ticket(&run, "a");
    assert_eq!(a["state"], "blocked");
    assert_eq!(a["exhaustion"], "replanning");
    assert!(a["blocker"].as_str().unwrap().contains("replanning"), "{a}");
    let r = replans(&run, "a");
    assert_eq!(r.len(), 1, "at most one replanning attempt by default");
    assert_eq!(r[0]["outcome"], "replanned");
    assert_eq!(r[0]["result"], "blocked");
    // Bounded: three corrections, the original session and one replanned session.
    assert_eq!(count(&run, "corrections", "a"), 3);
    assert_eq!(count(&run, "sessions", "a"), 2);
    assert_eq!(count(&run, "sessions", "c"), 0, "descendants never start");
    let c = ticket(&run, "c");
    assert_eq!(c["state"], "waiting");
    assert!(c["blocker"].as_str().unwrap().contains("prerequisite a is blocked"), "{c}");
    assert_eq!(ticket(&run, "b")["state"], "integrated", "independent work continues");
    assert_eq!(repo.inspect(&id)["replans"], run["replans"]);
}

#[test]
fn a_replanned_ticket_failing_independent_verification_is_blocked_before_new_work() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[("a", &[])]);
    let (out, run) = repo.run(&id, json!({"tickets":{"a":exhausting_then(replanning("a","failed","never.txt",approved()))}}));
    assert!(!out.status.success());
    let r = replans(&run, "a");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["outcome"], "rejected");
    assert!(r[0]["findings"].as_array().unwrap().iter().any(|f| f["code"] == "verification_not_verified"));
    assert_eq!(count(&run, "sessions", "a"), 1, "no revised work starts from an unverified replan");
    assert_eq!(ticket(&run, "a")["exhaustion"], "replanning");
    let plan_ticket = run["plan"]["tickets"].as_array().unwrap().iter().find(|t| t["id"] == "a").unwrap();
    assert_eq!(plan_ticket["description"], "Deliver", "the accepted plan is unchanged");
}

#[test]
fn replanning_is_configurable_and_can_be_disabled() {
    let repo = Repo::new(json!({"replanning_attempts": 0}));
    let id = repo.planned(&[("a", &[])]);
    let (out, run) = repo.run(&id, json!({"tickets":{"a":exhausting_then(replanning("a","verified","good.txt",approved()))}}));
    assert!(!out.status.success());
    assert_eq!(run["scheduler"]["limits"]["replanning_attempts"], 0);
    assert!(run["replans"].as_array().unwrap().is_empty());
    assert_eq!(ticket(&run, "a")["exhaustion"], "correction_cycles");

    let repo = Repo::new(json!({"replanning_attempts": "once"}));
    let prepared = repo.cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    assert!(!prepared.status.success(), "invalid replanning limits are rejected before launch");
    assert!(String::from_utf8_lossy(&prepared.stderr).contains("replanning_attempts"));
}

#[test]
fn replanning_waits_for_remaining_run_limits() {
    let repo = Repo::new(json!({"usage_token_limit": 500}));
    let id = repo.planned(&[("a", &[])]);
    let mut a = exhausting_then(replanning("a","verified","good.txt",approved()));
    // The last correction consumes the remaining usage allowance.
    a["corrections"]["corrections"][2]["log"] = json!(usage(700, json!(null)).to_string());
    let (out, run) = repo.run(&id, json!({"tickets":{"a":a}}));
    assert!(!out.status.success());
    assert_eq!(run["status"], "limit_exhausted");
    assert_eq!(count(&run, "corrections", "a"), 3);
    assert!(run["replans"].as_array().unwrap().is_empty(), "no replanning without remaining resources");
    assert_eq!(ticket(&run, "a")["state"], "stopped", "the ticket stays resumable");
}
