use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    /// Two approved specs, deterministic checks, optional implementation concurrency cap.
    fn new(concurrency: Option<u64>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let repo = Self { _temp: temp, path };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        fs::write(repo.path.join("one.md"), "# One\n## Acceptance criteria\n- Works\n").unwrap();
        fs::write(repo.path.join("two.md"), "# Two\n## Acceptance criteria\n- Also works\n").unwrap();
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
        let out = Command::new("git").args(args).current_dir(&self.path).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln")).args(args).current_dir(&self.path).output().unwrap()
    }
    /// Prepares a run with an accepted plan. `tickets` are (id, spec, blocked_by);
    /// tests must cover both specs.
    fn planned(&self, tickets: &[(&str, &str, &[&str])]) -> String {
        let prepared = self.cli(&["prepare", "--config", "kiln.json", "--spec", "one.md", "--spec", "two.md"]);
        assert!(prepared.status.success(), "{}", String::from_utf8_lossy(&prepared.stderr));
        let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
        let id = prepared["id"].as_str().unwrap().to_owned();
        let plan_tickets: Vec<Value> = tickets
            .iter()
            .map(|(name, spec, blockers)| json!({"id":name,"title":name,"description":"Deliver","acceptance_criteria":["works"],"covers":[format!("{spec}#ac-1")],"blocked_by":blockers}))
            .collect();
        fs::write(self.path.join("plan.json"), json!({"tickets":plan_tickets,"verification":{"outcome":"verified","findings":[]}}).to_string()).unwrap();
        let planned = self.cli(&["plan", &id, "--fixture", "plan.json"]);
        assert!(planned.status.success(), "{}", String::from_utf8_lossy(&planned.stderr));
        id
    }
    fn scenario(&self, scenario: Value) {
        fs::write(self.path.join("scenario.json"), scenario.to_string()).unwrap();
    }
    fn run(&self, id: &str) -> (Output, Value) {
        let out = self.cli(&["run", id, "--fixture", "scenario.json"]);
        let run = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("no run state: {}", String::from_utf8_lossy(&out.stderr)));
        (out, run)
    }
}
fn approved() -> Value {
    json!({"outcome":"approved","findings":[],"evidence":"Observed concrete change"})
}
/// A deterministic ticket that writes `file` and passes both review axes.
fn works(file: &str) -> Value {
    json!({"implementation":{"files":{file:"works"},"outcome":"completed"},"review":{"standards":approved(),"spec":approved()}})
}
fn ticket<'a>(run: &'a Value, id: &str) -> &'a Value {
    run["scheduler"]["tickets"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()
}
fn sessions_for<'a>(run: &'a Value, id: &str) -> Vec<&'a Value> {
    run["sessions"].as_array().unwrap().iter().filter(|s| s["ticket_id"] == id).collect()
}
fn integrated_commit(run: &Value, id: &str) -> String {
    run["integrations"].as_array().unwrap().iter().find(|i| i["ticket_id"] == id && i["status"] == "integrated").unwrap()["integrated_commit"].as_str().unwrap().to_owned()
}
fn assert_ancestor(repo: &Path, ancestor: &str, descendant: &str) {
    let ok = Command::new("git").args(["merge-base", "--is-ancestor", ancestor, descendant]).current_dir(repo).status().unwrap();
    assert!(ok.success(), "{ancestor} is not an ancestor of {descendant}");
}

#[test]
fn cross_spec_prerequisite_is_integrated_and_verified_before_dependent_starts() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &["a"])]);
    // b can only succeed when a's integrated change is already present in its worktree.
    let mut b = works("b.txt");
    b["require_files"] = json!(["a.txt"]);
    repo.scenario(json!({"tickets":{"a":works("a.txt"),"b":b}}));
    let (out, run) = repo.run(&id);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(run["status"], "awaiting_validation");
    assert_eq!(run["scheduler"]["implementation_concurrency"], 3);
    for t in ["a", "b"] {
        assert_eq!(ticket(&run, t)["state"], "integrated");
        assert_eq!(sessions_for(&run, t).len(), 1);
        assert_eq!(sessions_for(&run, t)[0]["status"], "integrated");
    }
    let b_base = sessions_for(&run, "b")[0]["base_commit"].as_str().unwrap().to_owned();
    assert_ancestor(&repo.path, &integrated_commit(&run, "a"), &b_base);
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    assert_eq!(tip, integrated_commit(&run, "b"));
    assert_eq!(repo.git(&["show", &format!("{tip}:a.txt")]), "works");
}
