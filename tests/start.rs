//! Public CLI coverage for `kiln start`: full backlog delivery with inferred
//! defaults, reusing an unchanged plan recorded by `kiln plan`.
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Delivery fixtures that stand in for the provider and GitHub. Planning and
/// issue fixtures are passed per call so tests can change them between runs.
const DELIVERY: &[&str] = &[
    "--run-fixture",
    "scenario.json",
    "--publication-fixture",
    "github.json",
    "--repair-fixture",
    "repair.json",
];

/// A target repository whose `origin` is on GitHub (so the GitHub repository is
/// inferred) and whose publication pushes to a local bare `fixture` remote.
struct Target {
    _temp: tempfile::TempDir,
    repo: PathBuf,
}
impl Target {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(
            temp.path(),
            &["init", "-q", "--bare", "-b", "main", "remote.git"],
        );
        let target = Self { repo, _temp: temp };
        let remote = target.repo.parent().unwrap().join("remote.git");
        target.git(&["init", "-q", "-b", "main"]);
        target.git(&["config", "user.name", "Kiln Test"]);
        target.git(&["config", "user.email", "kiln@example.test"]);
        target.git(&["config", "commit.gpgsign", "false"]);
        target.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ]);
        target.git(&["remote", "add", "fixture", remote.to_str().unwrap()]);
        fs::write(target.repo.join(".gitignore"), ".kiln/\n").unwrap();
        target.git(&["add", "."]);
        target.git(&["commit", "-qm", "initial"]);
        target.git(&["push", "-q", "fixture", "main"]);
        target.write("kiln.json", json!({
            "build":["git","diff","--check"], "test":["git","diff","--check"], "startup":["git","--version"],
            "acceptance_criteria":["Open issues are delivered after verification"],
            "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]},
            "validation":{"workflows":[
                {"criterion":"github-example-project-1.md#ac-1","command":["git","diff","--check"]},
                {"criterion":"github-example-project-2.md#ac-1","command":["git","diff","--check"]}
            ]},
            "publication":{"github_repository":"example/project","target_branch":"main","remote":"fixture"}
        }));
        target.issues(&[1]);
        target.planning(&[1], "verified");
        let approved = json!({"outcome":"approved","evidence":"Reviewed"});
        let ticket = |file: &str| {
            json!({"implementation":{"files":{file:"done\n"},"outcome":"completed"},
                "review":{"standards":approved,"spec":approved}})
        };
        target.write(
            "scenario.json",
            json!({"tickets":{
                "github:example/project#1":ticket("first.txt"),
                "github:example/project#2":ticket("second.txt")
            }}),
        );
        target.write("github.json", json!({"pull_requests":[]}));
        target.write("repair.json", json!({"corrections":[],"reviews":[]}));
        target
    }
    fn git(&self, args: &[&str]) {
        git(&self.repo, args);
    }
    fn write(&self, path: &str, value: Value) {
        fs::write(self.repo.join(path), value.to_string()).unwrap();
    }
    /// Open issues fixture with the given issue numbers.
    fn issues(&self, numbers: &[u64]) {
        let issues: Vec<Value> = numbers
            .iter()
            .map(|n| {
                json!({"number":n,"url":format!("https://github.com/example/project/issues/{n}"),
                    "title":format!("Issue {n}"),"body":format!("## Acceptance criteria\n- Implement {n}\n"),
                    "labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"})
            })
            .collect();
        self.write("issues.json", json!({ "issues": issues }));
    }
    /// Planning fixture for the given issues; `rejected` makes any planning fail.
    fn planning(&self, numbers: &[u64], outcome: &str) {
        let tickets: Vec<Value> = numbers
            .iter()
            .map(|n| {
                json!({"id":format!("github:example/project#{n}"),"title":format!("Issue {n}"),
                    "description":"Implement the issue","acceptance_criteria":[format!("Implement {n}")],
                    "covers":[format!("github-example-project-{n}.md#ac-1")],"blocked_by":[]})
            })
            .collect();
        let findings = if outcome == "verified" {
            json!([])
        } else {
            json!([{"code":"planned-twice","message":"planning must not run again"}])
        };
        self.write(
            "planning.json",
            json!({"tickets":tickets,"verification":{"outcome":outcome,"findings":findings}}),
        );
    }
    fn kiln(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.repo)
            // No real provider may be found: fixtures stand in for every agent.
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap()
    }
    fn start(&self, extra: &[&str]) -> Output {
        let mut args = vec![
            "start",
            "--issue-fixture",
            "issues.json",
            "--planning-fixture",
            "planning.json",
        ];
        args.extend_from_slice(DELIVERY);
        args.extend_from_slice(extra);
        self.kiln(&args)
    }
    fn plan(&self) -> Value {
        json_of(&self.kiln(&[
            "plan",
            "--json",
            "--issue-fixture",
            "issues.json",
            "--planning-fixture",
            "planning.json",
        ]))
    }
    fn runs(&self) -> Vec<String> {
        serde_json::from_value(json_of(&self.kiln(&["inspect"]))).unwrap()
    }
    fn pull_requests(&self) -> usize {
        let github: Value =
            serde_json::from_slice(&fs::read(self.repo.join("github.json")).unwrap()).unwrap();
        github["pull_requests"].as_array().unwrap().len()
    }
}
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn json_of(output: &Output) -> Value {
    serde_json::from_str(&success(output)).expect("--json prints the run as JSON")
}

#[test]
fn start_with_no_flags_delivers_the_open_issue_graph_to_its_outcome() {
    let target = Target::new();

    let stdout = success(&target.start(&[]));

    assert!(
        serde_json::from_str::<Value>(&stdout).is_err(),
        "default output is human-readable: {stdout}"
    );
    let runs = target.runs();
    assert_eq!(runs.len(), 1);
    let run = json_of(&target.kiln(&["inspect", &runs[0]]));
    assert_eq!(run["status"], "completed", "{run:#}");
    assert_eq!(run["delivery_groups"][0]["status"], "verified");
    assert_eq!(target.pull_requests(), 1);
    let url = run["delivery_groups"][0]["pull_request"]["url"]
        .as_str()
        .unwrap();
    for expected in [
        runs[0].as_str(),
        "example/project",
        "completed",
        "Issue 1",
        url,
    ] {
        assert!(
            stdout.contains(expected),
            "missing {expected:?} in:\n{stdout}"
        );
    }
}

#[test]
fn start_after_plan_delivers_the_planned_run_without_planning_again() {
    let target = Target::new();
    let planned = target.plan();
    assert_eq!(planned["status"], "planned");
    let id = planned["id"].as_str().unwrap();
    // Any second planning would now be rejected.
    target.planning(&[1], "rejected");

    let output = target.start(&[]);

    success(&output);
    assert_eq!(target.runs(), vec![id.to_owned()]);
    let run = json_of(&target.kiln(&["inspect", id]));
    assert_eq!(run["status"], "completed", "{run:#}");
    assert_eq!(run["plan"], planned["plan"]);
    assert_eq!(target.pull_requests(), 1);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(id),
        "start says which plan it reuses"
    );
}

#[test]
fn start_plans_a_new_run_when_the_open_issues_changed_since_the_plan() {
    let target = Target::new();
    let planned = target.plan();
    target.issues(&[1, 2]);
    target.planning(&[1, 2], "verified");

    let output = target.start(&[]);

    success(&output);
    let runs = target.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    let new_id = runs.iter().find(|id| **id != planned["id"]).unwrap();
    let run = json_of(&target.kiln(&["inspect", new_id]));
    assert_eq!(run["status"], "completed", "{run:#}");
    assert_eq!(run["plan"]["tickets"].as_array().unwrap().len(), 2);
    let old = json_of(&target.kiln(&["inspect", planned["id"].as_str().unwrap()]));
    assert_eq!(old["status"], "planned", "the stale plan is left untouched");
}

#[test]
fn start_fresh_plans_anew_even_when_a_reusable_plan_exists() {
    let target = Target::new();
    let planned = target.plan();

    let run = json_of(&target.start(&["--fresh", "--json"]));

    assert_ne!(run["id"], planned["id"]);
    assert_eq!(run["status"], "completed", "{run:#}");
    assert_eq!(target.runs().len(), 2);
    let old = json_of(&target.kiln(&["inspect", planned["id"].as_str().unwrap()]));
    assert_eq!(old["status"], "planned");
}

#[test]
fn start_json_prints_the_recorded_run() {
    let target = Target::new();

    let run = json_of(&target.start(&["--json"]));

    let stored = json_of(&target.kiln(&["inspect", run["id"].as_str().unwrap()]));
    assert_eq!(run["status"], "completed");
    assert_eq!(run["id"], stored["id"]);
    assert_eq!(run["delivery_groups"], stored["delivery_groups"]);
}

#[test]
fn start_is_listed_in_help() {
    let target = Target::new();
    let help = success(&target.kiln(&["--help"]));
    assert!(
        help.lines().any(|line| line.trim_start().starts_with("start ")),
        "{help}"
    );
}
