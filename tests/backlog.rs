//! Public CLI coverage for starting and delivering one read-only GitHub issue run.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Project {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    remote: PathBuf,
}
impl Project {
    fn git_at(&self, path: &PathBuf, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn git(&self, args: &[&str]) -> String {
        self.git_at(&self.repo, args)
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.repo)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn write(&self, path: &str, value: Value) {
        fs::write(self.repo.join(path), value.to_string()).unwrap();
    }
}

#[test]
fn one_cli_start_snapshots_plans_runs_validates_and_publishes_without_mutating_issue() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    let p = Project {
        _temp: temp,
        repo,
        remote,
    };
    p.git_at(
        &p.repo.parent().unwrap().to_path_buf(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["config", "user.name", "Test"]);
    p.git(&["config", "user.email", "test@example.com"]);
    p.git(&["remote", "add", "origin", p.remote.to_str().unwrap()]);
    fs::write(p.repo.join("README.md"), "# Demo\nA tiny project.\n").unwrap();
    fs::write(p.repo.join(".gitignore"), ".kiln/\n").unwrap();
    let criterion = "When the app starts, it writes the greeting to greeting.txt";
    p.write("kiln.json", json!({
        "build":["git","diff","--check"], "test":["git","diff","--check"], "startup":["git","--version"],
        "acceptance_criteria":[criterion],
        "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"],["test","-f","greeting.txt"]]},
        "validation":{"workflows":[{"criterion":"github-example-project-7.md#ac-1","command":["test","-f","greeting.txt"]}]},
        "publication":{"github_repository":"example/project","target_branch":"main"}
    }));
    p.git(&["add", "."]);
    p.git(&["commit", "-qm", "initial"]);
    p.git(&["push", "-q", "origin", "main"]);
    let issue = json!({"issues":[{"number":7,"url":"https://github.com/example/project/issues/7","title":"Add greeting output","body":"When the app starts, write a friendly greeting to a text file so users can read it later.","labels":["enhancement"],"assignee":"reporter","comments":["Please preserve existing output."],"blocked_by":[],"state":"OPEN","dependency_source":"native-and-body"}]});
    p.write("issues.json", issue.clone());
    p.write("planning.json", json!({"inferred_requirements":[criterion],"tickets":[{
        "id":"github:example/project#7","title":"Add greeting output","description":"Implement the issue using repository context.",
        "acceptance_criteria":[criterion],"covers":["github-example-project-7.md#ac-1"],"blocked_by":[]
    }],"verification":{"outcome":"verified","findings":[]}}));
    p.write("scenario.json", json!({"tickets":{"github:example/project#7":{
        "implementation":{"files":{"greeting.txt":"hello\\n"},"outcome":"completed"},
        "review":{"standards":{"outcome":"rejected","findings":[{"code":"format","message":"Use the agreed greeting wording","evidence":"greeting.txt contents","required":true}],"evidence":"Observed greeting.txt"},"spec":{"outcome":"approved","findings":[],"evidence":"Matches the issue criterion"}},
        "corrections":{"corrections":[{"files":{"greeting.txt":"Hello from the app!\\n"},"outcome":"completed"}],"reviews":[{"standards":{"outcome":"approved","findings":[],"evidence":"Observed corrected greeting"},"spec":{"outcome":"approved","findings":[],"evidence":"Matches the issue criterion"}}]}
    }}}));
    p.write("verifier.json", json!({"acceptance_checks":[]}));
    p.write("github.json", json!({"pull_requests":[]}));

    let run = p.ok(&[
        "start-issue",
        "--config",
        "kiln.json",
        "--github-repo",
        "example/project",
        "--issue",
        "7",
        "--issue-fixture",
        "issues.json",
        "--planning-fixture",
        "planning.json",
        "--run-fixture",
        "scenario.json",
        "--verifier",
        "verifier.json",
        "--publication-fixture",
        "github.json",
    ]);
    assert_eq!(run["publication"]["status"], "published");
    assert_eq!(run["imported_issues"], issue["issues"]);
    assert_eq!(
        run["backlog"]["issue_snapshot"][0]["comments"][0],
        "Please preserve existing output."
    );
    assert_eq!(run["backlog"]["issue_snapshot"][0]["assignee"], "reporter");
    assert_eq!(run["backlog"]["issue_snapshot"][0]["state"], "OPEN");
    assert_eq!(run["plan"]["requirements"][0]["criterion"], criterion);
    assert_eq!(run["backlog"]["inferred_requirements"][0], criterion);
    assert_eq!(run["plan"]["executable"], true);
    assert_eq!(run["plan"]["verification"]["outcome"], "verified");
    assert_eq!(run["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(run["corrections"].as_array().unwrap().len(), 1);
    assert_eq!(run["validation_reports"][0]["outcome"], "verified");
    assert_eq!(run["publication"]["pull_request"]["draft"], true);
    assert!(run["backlog"]["skill_version"].as_str().is_some());
    assert_eq!(run["backlog"]["outcome"], "published");
    assert_eq!(
        run["backlog"]["dispositions"][0]["issue"],
        "github:example/project#7"
    );
    assert_eq!(run["backlog"]["dispositions"][0]["status"], "completed");
    let implementation_context = fs::read_to_string(p.repo.join(".kiln/contexts").join(format!(
        "{}.json",
        run["corrections"][0]["before"]["context_id"].as_str().unwrap()
    )))
    .unwrap();
    assert!(implementation_context.contains("write one behavior test first and observe it fail"));
    let review_context =
        fs::read_to_string(p.repo.join(".kiln/contexts").join(
            format!("{}.json", run["reviews"][0]["standards"]["context_id"].as_str().unwrap()),
        ))
        .unwrap();
    assert!(review_context.contains("assess standards and spec coverage independently"));
    let correction_context = fs::read_to_string(p.repo.join(".kiln/contexts").join(format!(
        "{}.json",
        run["corrections"][0]["id"].as_str().unwrap()
    )))
    .unwrap();
    assert!(correction_context.contains("kiln-mattpocock-workflow-1.0.0"));
    assert!(correction_context.contains("write one behavior test first and observe it fail"));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(p.repo.join("issues.json")).unwrap()).unwrap(),
        issue
    );
}

#[test]
fn frozen_backlog_gives_every_open_issue_a_disposition_and_retains_open_edges() {
    use kiln::{backlog::classify_snapshot, import::ImportedIssue};
    let issue = |number, labels: &[&str], blocked_by: &[&str]| ImportedIssue {
        number,
        url: format!("https://github.com/example/project/issues/{number}"),
        title: format!("Issue {number}"),
        body: String::new(),
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
        assignee: None,
        comments: Vec::new(),
        state: "OPEN".into(),
        blocked_by: blocked_by.iter().map(|blocker| (*blocker).into()).collect(),
        dependency_source: "native-and-body".into(),
    };
    let snapshot = vec![
        issue(1, &[], &[]),
        issue(2, &["epic"], &[]),
        issue(3, &[], &["github:example/project#1"]),
        issue(4, &[], &["github:other/repo#8"]),
    ];

    let dispositions = classify_snapshot("example/project", &snapshot, 1);

    assert_eq!(dispositions.len(), snapshot.len());
    assert_eq!(dispositions[0].status, "eligible");
    assert_eq!(dispositions[1].status, "container");
    assert_eq!(dispositions[2].status, "blocked");
    assert_eq!(dispositions[2].dependencies, ["github:example/project#1"]);
    assert_eq!(dispositions[3].status, "unable-to-verify");
    assert_eq!(dispositions[3].dependencies, ["github:other/repo#8"]);
    assert!(!dispositions[3].selected);
    assert!(dispositions
        .iter()
        .all(|disposition| !disposition.reason.is_empty()));
}

#[test]
fn one_issue_start_refuses_closed_or_blocked_issues_before_creating_a_run() {
    for state in ["CLOSED", "BLOCKED"] {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()
            .unwrap();
        fs::write(
            repo.join("kiln.json"),
            json!({
                "build":["git"],"test":["git"],"startup":["git"],"acceptance_criteria":["Works"],
                "isolation":{"network":"none","runtime":"system","commands":[["git"]]}
            })
            .to_string(),
        )
        .unwrap();
        let blocked_by = if state == "BLOCKED" {
            json!(["github:example/project#3"])
        } else {
            json!([])
        };
        let issue_state = if state == "CLOSED" { "CLOSED" } else { "OPEN" };
        fs::write(repo.join("issues.json"), json!({"issues":[{
            "number":7,"url":"https://github.com/example/project/issues/7","title":"Feature",
            "body":"## Acceptance criteria\n- Works\n","labels":[],"comments":[],"assignee":null,
            "blocked_by":blocked_by,"state":issue_state
        }]}).to_string()).unwrap();
        fs::write(
            repo.join("planning.json"),
            json!({"tickets":[],"verification":{"outcome":"verified","findings":[]}}).to_string(),
        )
        .unwrap();
        fs::write(repo.join("scenario.json"), "{\"tickets\":{}}").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args([
                "start-issue",
                "--config",
                "kiln.json",
                "--github-repo",
                "example/project",
                "--issue",
                "7",
                "--issue-fixture",
                "issues.json",
                "--planning-fixture",
                "planning.json",
                "--run-fixture",
                "scenario.json",
            ])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{state} issue unexpectedly started"
        );
        assert!(
            !repo.join(".kiln/runs").exists(),
            "{state} issue created durable work"
        );
    }
}

#[test]
fn ambiguous_plan_is_recorded_and_never_reaches_implementation() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .status()
        .unwrap();
    fs::write(
        repo.join("kiln.json"),
        json!({
            "build":["git"],"test":["git"],"startup":["git"],"acceptance_criteria":["Works"],
            "isolation":{"network":"none","runtime":"system","commands":[["git"]]}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        repo.join("issues.json"),
        json!({"issues":[{
            "number":7,"url":"https://github.com/example/project/issues/7","title":"Make the tool nicer",
            "body":"Could be faster or easier; whichever is best.","labels":[],"comments":[],"assignee":null,
            "blocked_by":[],"state":"OPEN"
        }]}).to_string(),
    )
    .unwrap();
    fs::write(
        repo.join("planning.json"),
        json!({
            "inferred_requirements":["The tool completes the explicitly requested improvement"],
            "tickets":[],
            "verification":{"outcome":"unable-to-verify","findings":[{"code":"ambiguous","message":"The request does not select speed or ease-of-use as the intended outcome."}]}
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args([
            "start-issue",
            "--config",
            "kiln.json",
            "--github-repo",
            "example/project",
            "--issue",
            "7",
            "--issue-fixture",
            "issues.json",
            "--planning-fixture",
            "planning.json",
            "--run-fixture",
            "unused.json",
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let run: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["status"], "plan_rejected");
    assert_eq!(run["sessions"].as_array().unwrap().len(), 0);
    assert!(run["plan"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == "ambiguous"));
}

#[test]
fn explicit_issue_acceptance_bullets_are_preserved_without_inference() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .status()
        .unwrap();
    fs::write(
        repo.join("kiln.json"),
        json!({
            "build":["git"],"test":["git"],"startup":["git"],"acceptance_criteria":["Works"],
            "isolation":{"network":"none","runtime":"system","commands":[["git"]]}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        repo.join("issues.json"),
        json!({"issues":[{
            "number":7,"url":"https://github.com/example/project/issues/7","title":"Add output",
            "body":"## Acceptance criteria\n- Keep the original acceptance wording exactly\n","labels":[],"comments":[],"assignee":null,
            "blocked_by":[],"state":"OPEN"
        }]}).to_string(),
    )
    .unwrap();
    fs::write(
        repo.join("planning.json"),
        json!({"tickets":[],"verification":{"outcome":"unable-to-verify","findings":[]}})
            .to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args([
            "start-issue",
            "--config",
            "kiln.json",
            "--github-repo",
            "example/project",
            "--issue",
            "7",
            "--issue-fixture",
            "issues.json",
            "--planning-fixture",
            "planning.json",
            "--run-fixture",
            "unused.json",
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let run: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        run["backlog"]["inferred_requirements"][0],
        "Keep the original acceptance wording exactly"
    );
    assert_eq!(
        run["plan"]["requirements"][0]["criterion"],
        "Keep the original acceptance wording exactly"
    );
}
