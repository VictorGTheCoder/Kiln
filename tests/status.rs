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
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let out = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&path)
            .output()
            .unwrap();
        assert!(out.status.success());
        Self { _temp: temp, path }
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "kiln {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
    /// Record a run state file as an earlier Kiln version would have written it.
    fn record(&self, run: Value) {
        let dir = self.path.join(".kiln/runs");
        fs::create_dir_all(&dir).unwrap();
        let id = run["id"].as_str().unwrap().to_owned();
        fs::write(dir.join(format!("{id}.json")), run.to_string()).unwrap();
    }
}

/// Minimal recorded run; `extra` fields override or extend it.
fn run(id: &str, created: u64, status: &str, extra: Value) -> Value {
    let mut run = json!({
        "schema_version": 1,
        "id": id,
        "repository": "/tmp/repo",
        "created_unix_ms": created,
        "status": status,
        "config": {
            "build": ["true"], "test": ["true"], "startup": ["true"],
            "acceptance_criteria": [],
            "isolation": {"network": "none", "runtime": "system", "commands": []}
        },
        "specs": []
    });
    for (key, value) in extra.as_object().unwrap() {
        run[key] = value.clone();
    }
    run
}

fn ticket(id: &str) -> Value {
    json!({"id": id, "title": format!("Title of {id}"), "description": "",
           "acceptance_criteria": [], "covers": [], "blocked_by": []})
}

fn scheduled(id: &str, state: &str) -> Value {
    json!({"id": id, "blocked_by": [], "state": state, "waiting_on": [],
           "blocker": null, "session_id": null})
}

/// A whole-graph backlog run in delivery: one ticket delivered with a
/// verified PR, one implementing, one waiting.
fn delivering_run(id: &str, created: u64) -> Value {
    run(
        id,
        created,
        "running",
        json!({
            "backlog": {
                "github_repository": "example/project", "mode": "issue-graph",
                "selected_issue": 0, "snapshot_unix_ms": 0, "issue_snapshot": [],
                "skill_version": "1", "outcome": "planned"
            },
            "plan": {"requirements": [], "tickets": [ticket("t-1"), ticket("t-2"), ticket("t-3")],
                     "findings": [], "dependency_graph": {}, "executable": true,
                     "generation_context": "g", "verification_context": "v",
                     "verification": {"outcome": "verified", "findings": []}},
            "scheduler": {
                "status": "running", "implementation_concurrency": 1,
                "active": ["t-2"], "peak_active": 1,
                "tickets": [scheduled("t-1", "integrated"), scheduled("t-2", "implementing"),
                            scheduled("t-3", "waiting")]
            },
            "delivery_groups": [{
                "id": "group-a", "tickets": ["t-1"], "status": "verified",
                "pull_request": {"number": 12, "url": "https://github.com/example/project/pull/12",
                                 "head": "kiln/group-a", "base": "main"},
                "ci_attempts": [{"id": "ci-1", "pull_request": 12, "commit": "abc",
                                 "observed_unix_ms": 1, "status": "passed", "checks": [],
                                 "failure": null}]
            }]
        }),
    )
}

#[test]
fn status_without_runs_says_so_and_succeeds() {
    let repo = Repo::new();
    let out = repo.ok(&["status"]);
    assert!(out.contains("No Kiln runs"), "{out}");
}

#[test]
fn status_summarises_the_latest_run() {
    let repo = Repo::new();
    repo.record(run("run-old", 100, "completed", json!({})));
    repo.record(delivering_run("run-new", 200));
    let out = repo.ok(&["status"]);
    assert!(out.contains("Run run-new"), "{out}");
    assert!(!out.contains("run-old"), "{out}");
    assert!(out.contains("Status: running"), "{out}");
    assert!(out.contains("example/project"), "{out}");
    // tickets per stage
    assert!(out.contains("delivered 1"), "{out}");
    assert!(out.contains("implementing 1"), "{out}");
    assert!(out.contains("waiting 1"), "{out}");
    // active ticket
    assert!(out.contains("Active: t-2  Title of t-2"), "{out}");
    // pull requests and CI
    assert!(
        out.contains("#12 https://github.com/example/project/pull/12  CI: passed"),
        "{out}"
    );
}

#[test]
fn status_with_an_id_summarises_that_run() {
    let repo = Repo::new();
    repo.record(run("run-old", 100, "completed", json!({})));
    repo.record(delivering_run("run-new", 200));
    let out = repo.ok(&["status", "run-old"]);
    assert!(out.contains("Run run-old"), "{out}");
    assert!(out.contains("Status: completed"), "{out}");
    assert!(out.contains("Tickets: none planned"), "{out}");
}

#[test]
fn status_with_an_unknown_id_fails_clearly() {
    let repo = Repo::new();
    repo.record(delivering_run("run-new", 200));
    let out = repo.cli(&["status", "run-missing"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("run 'run-missing' was not found"), "{err}");
}

#[test]
fn status_json_is_a_machine_readable_summary() {
    let repo = Repo::new();
    repo.record(delivering_run("run-new", 200));
    let summary: Value = serde_json::from_str(&repo.ok(&["status", "--json"])).unwrap();
    assert_eq!(summary["id"], "run-new");
    assert_eq!(summary["status"], "running");
    assert_eq!(summary["github_repository"], "example/project");
    assert_eq!(
        summary["stages"],
        json!([{"stage": "waiting", "count": 1}, {"stage": "implementing", "count": 1},
               {"stage": "delivered", "count": 1}])
    );
    assert_eq!(summary["active"][0]["id"], "t-2");
    assert_eq!(summary["pull_requests"][0]["number"], 12);
    assert_eq!(summary["pull_requests"][0]["ci"], "passed");
    assert_eq!(summary["pull_requests"][0]["tickets"], json!(["t-1"]));
}

#[test]
fn status_json_without_runs_reports_no_run() {
    let repo = Repo::new();
    let summary: Value = serde_json::from_str(&repo.ok(&["status", "--json"])).unwrap();
    assert_eq!(summary, Value::Null);
}

#[test]
fn status_summarises_an_earlier_single_issue_run_from_its_publication() {
    let repo = Repo::new();
    repo.record(run(
        "run-issue",
        300,
        "published",
        json!({
            "backlog": {
                "github_repository": "example/project", "selected_issue": 7,
                "snapshot_unix_ms": 0, "issue_snapshot": [],
                "skill_version": "1", "outcome": "published"
            },
            "plan": {"requirements": [], "tickets": [ticket("t-7")],
                     "findings": [], "dependency_graph": {}, "executable": true,
                     "generation_context": "g", "verification_context": "v",
                     "verification": {"outcome": "verified", "findings": []}},
            "publication": {
                "status": "published", "validation_report": "r", "published_commit": "c",
                "github_repository": "example/project", "remote": "origin",
                "branch": "kiln/run-issue", "target_branch": "main",
                "pull_request": {"number": 3, "url": "https://github.com/example/project/pull/3",
                                 "head": "kiln/run-issue", "base": "main"},
                "reconciled": false, "merge_authorized": false, "deploy_authorized": false
            }
        }),
    ));
    let out = repo.ok(&["status"]);
    assert!(out.contains("Run run-issue for example/project"), "{out}");
    assert!(out.contains("Status: published"), "{out}");
    assert!(out.contains("Tickets (1): planned 1"), "{out}");
    assert!(
        out.contains("#3 https://github.com/example/project/pull/3  CI: published"),
        "{out}"
    );
}

#[test]
fn status_is_listed_in_help_and_inspect_stays_hidden() {
    let repo = Repo::new();
    let help = repo.ok(&["--help"]);
    assert!(help.contains("status"), "{help}");
    assert!(!help.contains("inspect"), "{help}");
    let ids: Value = serde_json::from_str(&repo.ok(&["inspect"])).unwrap();
    assert_eq!(ids, json!([]));
}
