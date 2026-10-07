//! Public CLI coverage for starting and delivering one read-only GitHub issue run.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
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
    assert_eq!(dispositions[1].status, "skipped");
    assert_eq!(dispositions[1].kind, "container");
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
fn start_backlog_freezes_and_independently_verifies_the_whole_issue_graph() {
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
            "build":["git"], "test":["git"], "startup":["git"],
            "acceptance_criteria":["Backlog plans are verified before execution"],
            "isolation":{"network":"none","runtime":"system","commands":[["git"]]}
        })
        .to_string(),
    )
    .unwrap();
    let criterion = |text: &str| format!("## Acceptance criteria\n- {text}\n");
    fs::write(
        repo.join("issues.json"),
        json!({"issues":[
            {"number":1,"url":"https://github.com/example/project/issues/1","title":"Base","body":criterion("Create base"),"labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"},
            {"number":2,"url":"https://github.com/example/project/issues/2","title":"Dependent","body":format!("{}\n## Blocked by\n- #1\n", criterion("Use base")),"labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"},
            {"number":3,"url":"https://github.com/example/project/issues/3","title":"Native dependency","body":criterion("Use base natively"),"labels":[],"comments":[],"assignee":null,"blocked_by":["github:example/project#1"],"state":"OPEN"},
            {"number":4,"url":"https://github.com/example/project/issues/4","title":"Epic","body":"Tracks child work","labels":["epic"],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"},
            {"number":5,"url":"https://github.com/example/project/issues/5","title":"Ambiguous","body":"Make it better","labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"},
            {"number":6,"url":"https://github.com/example/project/issues/6","title":"Depends on ambiguous","body":criterion("Wait for issue 5"),"labels":[],"comments":[],"assignee":null,"blocked_by":["github:example/project#5"],"state":"OPEN"},
            {"number":7,"url":"https://github.com/example/project/issues/7","title":"Inferred","body":"Write an observable behavior","labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"},
            {"number":8,"url":"https://github.com/example/project/issues/8","title":"Closed at snapshot","body":criterion("Must not appear"),"labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"CLOSED"}
        ]}).to_string(),
    )
    .unwrap();
    fs::write(
        repo.join("planning.json"),
        json!({
            "tickets":[
                {"id":"github:example/project#1","title":"Base","description":"Create the base behavior","acceptance_criteria":["Create base"],"covers":["github-example-project-1.md#ac-1"],"blocked_by":[]},
                {"id":"github:example/project#2","title":"Dependent","description":"Use the base behavior","acceptance_criteria":["Use base"],"covers":["github-example-project-2.md#ac-1"],"blocked_by":["github:example/project#1"]},
                {"id":"github:example/project#3","title":"Native dependency","description":"Use the base behavior","acceptance_criteria":["Use base natively"],"covers":["github-example-project-3.md#ac-1"],"blocked_by":["github:example/project#1"]},
                {"id":"github:example/project#7","title":"Inferred","description":"Implement inferred observable behavior","acceptance_criteria":["Observable inferred behavior"],"covers":["github-example-project-7.md#ac-1"],"blocked_by":[]}
            ],
            "verification":{"outcome":"verified","findings":[]},
            "inference_by_issue":{"5":[],"7":["Observable inferred behavior"]}
        }).to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args([
            "start-backlog",
            "--config",
            "kiln.json",
            "--github-repo",
            "example/project",
            "--issue-fixture",
            "issues.json",
            "--planning-fixture",
            "planning.json",
            "--plan-only",
        ])
        .current_dir(repo)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        run["backlog"]["issue_snapshot"].as_array().unwrap().len(),
        7
    );
    assert_eq!(run["plan"]["verification"]["outcome"], "verified");
    assert_eq!(
        run["plan"]["dependency_graph"]["github:example/project#2"][0],
        "github:example/project#1"
    );
    assert_eq!(
        run["plan"]["dependency_graph"]["github:example/project#3"][0],
        "github:example/project#1"
    );
    assert!(run["specs"].as_array().unwrap().iter().any(|spec| {
        spec["path"] == "github-example-project-3.md"
            && spec["content"]
                .as_str()
                .unwrap()
                .contains("github:example/project#1")
    }));
    assert_eq!(run["backlog"]["dispositions"][1]["status"], "blocked");
    assert_eq!(run["backlog"]["dispositions"][3]["status"], "skipped");
    assert_eq!(run["backlog"]["dispositions"][3]["kind"], "container");
    assert_eq!(
        run["backlog"]["dispositions"][4]["status"],
        "unable-to-verify"
    );
    assert!(run["backlog"]["dispositions"][4]["reason"]
        .as_str()
        .unwrap()
        .contains("no explicit acceptance bullets"));
    assert_eq!(run["backlog"]["dispositions"][5]["status"], "blocked");
    assert_eq!(
        run["backlog"]["dispositions"][6]["inferred_criteria"][0],
        "Observable inferred behavior"
    );
}

#[test]
fn start_backlog_persists_delivery_block_when_publication_is_not_configured() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    let project = Project {
        _temp: temp,
        repo,
        remote,
    };
    project.git_at(
        &project.repo.parent().unwrap().to_path_buf(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    project.git(&["init", "-q", "-b", "main"]);
    project.git(&["config", "user.name", "Kiln Test"]);
    project.git(&["config", "user.email", "kiln@example.test"]);
    project.git(&["config", "commit.gpgsign", "false"]);
    project.git(&["remote", "add", "origin", project.remote.to_str().unwrap()]);
    project.write("README.md", json!("initial"));
    project.git(&["add", "."]);
    project.git(&["commit", "-qm", "initial"]);
    project.git(&["push", "-q", "origin", "main"]);
    project.write("kiln.json", json!({
        "build":["git","diff","--check"], "test":["git","diff","--check"], "startup":["git","--version"],
        "acceptance_criteria":["The issue is delivered after verification"],
        "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}
    }));
    project.write("issues.json", json!({"issues":[{
        "number":1,"url":"https://github.com/example/project/issues/1","title":"Feature",
        "body":"## Acceptance criteria\n- Implement feature\n","labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"
    }]}));
    project.write("planning.json", json!({
        "tickets":[{"id":"github:example/project#1","title":"Feature","description":"Implement feature",
            "acceptance_criteria":["Implement feature"],"covers":["github-example-project-1.md#ac-1"],"blocked_by":[]}],
        "verification":{"outcome":"verified","findings":[]}
    }));
    project.write("scenario.json", json!({"tickets":{
        "github:example/project#1":{
            "implementation":{"files":{"feature.txt":"done\n"},"outcome":"completed"},
            "review":{"standards":{"outcome":"approved","evidence":"Reviewed"},"spec":{"outcome":"approved","evidence":"Matches spec"}}
        }
    }}));
    let output = project.cli(&[
        "start-backlog",
        "--config",
        "kiln.json",
        "--github-repo",
        "example/project",
        "--issue-fixture",
        "issues.json",
        "--planning-fixture",
        "planning.json",
        "--run-fixture",
        "scenario.json",
    ]);
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("publication is not configured"));
    let run: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["delivery_groups"][0]["status"], "delivery-blocked");
    assert!(run["delivery_groups"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("publication is not configured"));
    let stored = project.cli(&["report", run["id"].as_str().unwrap()]);
    assert!(stored.status.success());
    let stored: Value = serde_json::from_slice(&stored.stdout).unwrap();
    assert_eq!(stored["delivery_groups"][0]["status"], "delivery-blocked");
}

#[test]
fn start_backlog_records_partial_results_and_keeps_independent_work_moving() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    let project = Project {
        _temp: temp,
        repo,
        remote,
    };
    project.git_at(
        &project.repo.parent().unwrap().to_path_buf(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    project.git(&["init", "-q", "-b", "main"]);
    project.git(&["config", "user.name", "Kiln Test"]);
    project.git(&["config", "user.email", "kiln@example.test"]);
    project.git(&["config", "commit.gpgsign", "false"]);
    project.git(&["remote", "add", "origin", project.remote.to_str().unwrap()]);
    fs::write(project.repo.join("README.md"), "# Backlog test\n").unwrap();
    fs::write(project.repo.join(".gitignore"), ".kiln/\n").unwrap();
    project.git(&["add", "."]);
    project.git(&["commit", "-qm", "initial"]);
    project.git(&["push", "-q", "origin", "main"]);
    let acceptance = |criterion: &str| format!("## Acceptance criteria\n- {criterion}\n");
    project.write("kiln.json", json!({
        "build":["git","diff","--check"], "test":["git","diff","--check"], "startup":["git","--version"],
        "acceptance_criteria":["Open issue work is scheduled according to dependency edges"],
        "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]},
        "validation":{"workflows":[{"criterion":"github-example-project-3.md#ac-1","command":["git","diff","--check"]}]},
        "publication":{"github_repository":"example/project","target_branch":"main","remote":"origin","required_checks_timeout_seconds":30,"required_checks_poll_seconds":1}
    }));
    project.write("issues.json", json!({"issues":[
        {"number":1,"url":"https://github.com/example/project/issues/1","title":"Fails","body":acceptance("Fail this implementation"),"labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"},
        {"number":2,"url":"https://github.com/example/project/issues/2","title":"Depends","body":acceptance("Wait for the failing issue"),"labels":[],"comments":[],"assignee":null,"blocked_by":["github:example/project#1"],"state":"OPEN"},
        {"number":3,"url":"https://github.com/example/project/issues/3","title":"Independent","body":acceptance("Complete independently"),"labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"}
    ]}));
    let ticket = |number: u64, criterion: &str, blockers: Vec<&str>| {
        json!({
            "id":format!("github:example/project#{number}"), "title":format!("Issue {number}"),
            "description":"Implement the issue", "acceptance_criteria":[criterion],
            "covers":[format!("github-example-project-{number}.md#ac-1")], "blocked_by":blockers
        })
    };
    project.write("planning.json", json!({
        "tickets":[ticket(1,"Fail this implementation",vec![]), ticket(2,"Wait for the failing issue",vec!["github:example/project#1"]), ticket(3,"Complete independently",vec![])],
        "verification":{"outcome":"verified","findings":[]}
    }));
    let approved = json!({"outcome":"approved","findings":[],"evidence":"Observed independent implementation"});
    project.write("scenario.json", json!({"tickets":{
        "github:example/project#1":{"implementation":{"files":{},"outcome":"failed"},"review":{"standards":approved,"spec":approved}},
        "github:example/project#2":{"implementation":{"files":{"should-not-exist.txt":"bad"},"outcome":"completed"},"review":{"standards":approved,"spec":approved}},
        "github:example/project#3":{"implementation":{"files":{"independent.txt":"done"},"outcome":"completed"},"review":{"standards":approved,"spec":approved}}
    }}));
    project.write(
        "start-github.json",
        json!({
            "pull_requests":[],"required_check_names":["linux"],"check_runs":[],
            "check_snapshots":[
                {"required_check_names":["linux"],"check_runs":[{"pull_request":1,"commit":"current","name":"linux","status":"pending","id":"run-1"}]},
                {"required_check_names":["linux"],"check_runs":[{"pull_request":1,"commit":"current","name":"linux","status":"pending","id":"run-2"}]},
                {"required_check_names":["linux"],"check_runs":[{"pull_request":1,"commit":"current","name":"linux","status":"completed","conclusion":"success","id":"run-3"}]}
            ]
        }),
    );
    project.write("start-repair.json", json!({"corrections":[],"reviews":[]}));

    let start_args = [
        "start-backlog",
        "--config",
        "kiln.json",
        "--github-repo",
        "example/project",
        "--issue-fixture",
        "issues.json",
        "--planning-fixture",
        "planning.json",
        "--run-fixture",
        "scenario.json",
        "--publication-fixture",
        "start-github.json",
        "--repair-fixture",
        "start-repair.json",
    ];
    let child = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(start_args)
        .current_dir(&project.repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let (run_id, initial_sessions) = loop {
        let run_path = fs::read_dir(project.repo.join(".kiln/runs"))
            .ok()
            .and_then(|entries| {
                entries.flatten().find_map(|entry| {
                    (entry.path().extension().and_then(|e| e.to_str()) == Some("json"))
                        .then_some(entry.path())
                })
            });
        if let Some(path) = run_path {
            if let Ok(bytes) = fs::read(path) {
                if let Ok(state) = serde_json::from_slice::<Value>(&bytes) {
                    if state["status"] == "running"
                        && state["delivery_groups"].as_array().is_some_and(|groups| {
                            groups.iter().any(|group| {
                                group["pull_request"].is_object()
                                    && group["ci_attempts"]
                                        .as_array()
                                        .is_some_and(|a| !a.is_empty())
                            })
                        })
                    {
                        break (
                            state["id"].as_str().unwrap().to_owned(),
                            state["sessions"].as_array().unwrap().len(),
                        );
                    }
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run never reached pending CI"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let pause = project.cli(&["pause", &run_id]);
    assert!(
        pause.status.success(),
        "{}",
        String::from_utf8_lossy(&pause.stderr)
    );
    let start_output = child.wait_with_output().unwrap();
    assert!(
        start_output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&start_output.stderr),
        String::from_utf8_lossy(&start_output.stdout)
    );
    let paused: Value = serde_json::from_slice(&start_output.stdout).unwrap();
    assert_eq!(paused["status"], "paused");
    let paused_group = paused["delivery_groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["pull_request"].is_object())
        .unwrap();
    assert_eq!(paused_group["status"], "ci-pending");
    assert_eq!(paused_group["ci_attempts"][0]["status"], "pending");
    let paused_pr = paused_group["pull_request"].clone();
    let resumed = project.cli(&[
        "resume",
        &run_id,
        "--fixture",
        "scenario.json",
        "--publication-fixture",
        "start-github.json",
        "--repair-fixture",
        "start-repair.json",
    ]);
    assert!(
        !resumed.status.success(),
        "the unrelated failed issue remains blocked"
    );
    let run: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(run["status"], "blocked");
    assert_eq!(run["sessions"].as_array().unwrap().len(), initial_sessions);
    let resumed_group = run["delivery_groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["tickets"] == json!(["github:example/project#3"]))
        .unwrap();
    assert_eq!(resumed_group["status"], "verified");
    assert_eq!(resumed_group["pull_request"]["number"], paused_pr["number"]);
    let initial_published: Value =
        serde_json::from_slice(&fs::read(project.repo.join("start-github.json")).unwrap()).unwrap();
    assert_eq!(
        initial_published["pull_requests"].as_array().unwrap().len(),
        1
    );
    let mut project_config: Value =
        serde_json::from_slice(&fs::read(project.repo.join("kiln.json")).unwrap()).unwrap();
    project_config["publication"]["required_checks_timeout_seconds"] = json!(1);
    project.write("kiln.json", project_config);

    let dispositions = run["backlog"]["dispositions"].as_array().unwrap();
    assert_eq!(dispositions[0]["status"], "failed");
    assert_eq!(dispositions[1]["status"], "blocked");
    assert_eq!(dispositions[2]["status"], "completed");
    assert!(!run["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|session| { session["ticket_id"] == "github:example/project#2" }));
    assert!(run["scheduler"]["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |ticket| ticket["id"] == "github:example/project#3" && ticket["state"] == "integrated"
        ));
    let groups = run["delivery_groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    let dependent_group = groups
        .iter()
        .find(|group| {
            group["tickets"]
                .as_array()
                .unwrap()
                .contains(&json!("github:example/project#2"))
        })
        .unwrap();
    assert_eq!(
        dependent_group["tickets"],
        json!(["github:example/project#1", "github:example/project#2"])
    );
    assert_eq!(
        dependent_group["dependency_edges"][0],
        json!({"ticket":"github:example/project#2","prerequisite":"github:example/project#1"})
    );
    assert_eq!(dependent_group["status"], "failed");
    let independent_group = groups
        .iter()
        .find(|group| {
            group["tickets"]
                .as_array()
                .unwrap()
                .contains(&json!("github:example/project#3"))
        })
        .unwrap();
    assert_eq!(
        independent_group["tickets"],
        json!(["github:example/project#3"])
    );
    assert_eq!(independent_group["pull_request"]["draft"], true);
    assert_eq!(independent_group["status"], "verified");
    project.write(
        "github.json",
        json!({"pull_requests":[],"required_check_names":["linux"],"check_runs":[{"pull_request":1,"commit":"stale-head","name":"linux","status":"completed","conclusion":"success","id":"stale-1"}]}),
    );
    project.write(
        "repair.json",
        json!({
            "corrections":[{"files":{"ci-repaired.txt":"repair applied\n"},"outcome":"completed"}],
            "reviews":[{"standards":{"outcome":"approved","evidence":"Reviewed standards for repaired group"},"spec":{"outcome":"approved","evidence":"Reviewed group requirements after repair"}}]
        }),
    );
    let first_publish = project.cli(&[
        "publish",
        run["id"].as_str().unwrap(),
        "--fixture",
        "github.json",
        "--repair-fixture",
        "repair.json",
    ]);
    assert!(
        first_publish.status.success(),
        "{}",
        String::from_utf8_lossy(&first_publish.stderr)
    );
    let published: Value = serde_json::from_slice(&first_publish.stdout).unwrap();
    let group = published["delivery_groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["tickets"] == json!(["github:example/project#3"]))
        .unwrap();
    assert_eq!(group["status"], "ci-pending");
    assert_eq!(group["ci_attempts"][2]["checks"][0]["status"], "pending");
    assert_eq!(
        group["ci_attempts"][2]["checks"][0]["commit"],
        group["commit"]
    );
    assert!(!group["ci_attempts"][2]["observations"]
        .as_array()
        .unwrap()
        .is_empty());
    let fixture: Value =
        serde_json::from_slice(&fs::read(project.repo.join("github.json")).unwrap()).unwrap();
    assert_eq!(fixture["pull_requests"].as_array().unwrap().len(), 1);
    assert_eq!(fixture["pull_requests"][0]["head"], group["branch"]);
    assert_eq!(fixture["pull_requests"][0]["draft"], true);
    let commit = group["commit"].as_str().unwrap();
    project.write(
        "github.json",
        json!({
            "pull_requests":fixture["pull_requests"].clone(),
            "required_check_names":["linux"],
            "check_runs":[{"pull_request":1,"commit":commit,"name":"linux","status":"completed","conclusion":"success","id":"run-17","url":"https://github.com/example/project/actions/runs/17"}]
        }),
    );
    let second_publish = project.cli(&[
        "publish",
        run["id"].as_str().unwrap(),
        "--fixture",
        "github.json",
        "--repair-fixture",
        "repair.json",
    ]);
    assert!(
        second_publish.status.success(),
        "{}",
        String::from_utf8_lossy(&second_publish.stderr)
    );
    let verified: Value = serde_json::from_slice(&second_publish.stdout).unwrap();
    let group = verified["delivery_groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["tickets"] == json!(["github:example/project#3"]))
        .unwrap();
    assert_eq!(group["status"], "verified");
    assert_eq!(group["ci_attempts"][3]["checks"][0]["commit"], commit);
    assert!(group["pull_request"]["body"]
        .as_str()
        .unwrap()
        .contains("Required GitHub checks"));
    let fixture: Value =
        serde_json::from_slice(&fs::read(project.repo.join("github.json")).unwrap()).unwrap();
    let mut checks = fixture["check_runs"].as_array().unwrap().clone();
    checks[0]["conclusion"] = json!("failure");
    project.write(
        "github.json",
        json!({
            "pull_requests":fixture["pull_requests"].clone(),
            "required_check_names":["linux"],
            "check_runs":checks,
            "check_snapshots":[
                {"required_check_names":["linux"],"check_runs":[{"pull_request":1,"commit":commit,"name":"linux","status":"completed","conclusion":"failure","id":"run-18","url":"https://github.com/example/project/actions/runs/18"}]},
                {"required_check_names":["linux"],"check_runs":[{"pull_request":1,"commit":"current","name":"linux","status":"completed","conclusion":"success","id":"run-19","url":"https://github.com/example/project/actions/runs/19"}]}
            ]
        }),
    );
    let failed_publish = project.cli(&[
        "publish",
        run["id"].as_str().unwrap(),
        "--fixture",
        "github.json",
        "--repair-fixture",
        "repair.json",
    ]);
    assert!(
        failed_publish.status.success(),
        "{}",
        String::from_utf8_lossy(&failed_publish.stderr)
    );
    let failed: Value = serde_json::from_slice(&failed_publish.stdout).unwrap();
    let group = failed["delivery_groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["tickets"] == json!(["github:example/project#3"]))
        .unwrap();
    assert_eq!(group["status"], "verified");
    assert_eq!(group["repair_attempts"], 1);
    assert_eq!(group["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(
        group["ci_attempts"][4]["checks"][0]["conclusion"],
        "failure"
    );
    assert_eq!(
        group["ci_attempts"][5]["checks"][0]["commit"],
        group["commit"]
    );
    assert!(group["pull_request"]["draft"].as_bool().unwrap());
    assert!(group["pull_request"]["body"]
        .as_str()
        .unwrap()
        .contains("Required GitHub checks"));
    project.write(
        "github.json",
        json!({
            "pull_requests":fixture["pull_requests"].clone(),
            "required_check_names":["linux"],
            "check_runs":[{"pull_request":1,"commit":"current","name":"linux","status":"completed","conclusion":"failure","id":"run-20","url":"https://github.com/example/project/actions/runs/20"}]
        }),
    );
    project.write("empty-repair.json", json!({"corrections":[],"reviews":[]}));
    let unresolved = project.cli(&[
        "publish",
        run["id"].as_str().unwrap(),
        "--fixture",
        "github.json",
        "--repair-fixture",
        "empty-repair.json",
    ]);
    assert!(
        unresolved.status.success(),
        "{}",
        String::from_utf8_lossy(&unresolved.stderr)
    );
    let unresolved: Value = serde_json::from_slice(&unresolved.stdout).unwrap();
    assert_ne!(unresolved["status"], "completed");
    let group = unresolved["delivery_groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["tickets"] == json!(["github:example/project#3"]))
        .unwrap();
    assert_eq!(group["status"], "repair-failed");
    assert!(group["reason"]
        .as_str()
        .unwrap()
        .contains("correction fixture sequence exhausted"));
    assert!(group["pull_request"]["draft"].as_bool().unwrap());
    assert!(group["pull_request"]["body"]
        .as_str()
        .unwrap()
        .contains("CI finding"));
    assert_eq!(
        unresolved["backlog"]["dispositions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["issue"] == "github:example/project#3")
            .unwrap()["status"],
        "failed"
    );
    let report = project.cli(&["report", run["id"].as_str().unwrap()]);
    assert!(report.status.success());
    let report: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(
        report["delivery_groups"][1]["ci_attempts"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
}

#[test]
fn repeated_start_backlog_skips_completed_open_issues_and_delivers_only_new_issues() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    let project = Project {
        _temp: temp,
        repo,
        remote,
    };
    project.git_at(
        &project.repo.parent().unwrap().to_path_buf(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    project.git(&["init", "-q", "-b", "main"]);
    project.git(&["config", "user.name", "Kiln Test"]);
    project.git(&["config", "user.email", "kiln@example.test"]);
    project.git(&["config", "commit.gpgsign", "false"]);
    project.git(&["remote", "add", "origin", project.remote.to_str().unwrap()]);
    project.write(".gitignore", json!(".kiln/\n"));
    project.git(&["add", "."]);
    project.git(&["commit", "-qm", "initial"]);
    project.git(&["push", "-q", "origin", "main"]);
    let criterion = |text: &str| format!("## Acceptance criteria\n- {text}\n");
    project.write("kiln.json", json!({
        "build":["git","diff","--check"], "test":["git","diff","--check"], "startup":["git","--version"],
        "acceptance_criteria":["Completed backlog issues are not scheduled twice"],
        "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]},
        "validation":{"workflows":[
            {"criterion":"github-example-project-1.md#ac-1","command":["git","diff","--check"]},
            {"criterion":"github-example-project-2.md#ac-1","command":["git","diff","--check"]}
        ]},
        "publication":{"github_repository":"example/project","target_branch":"main","remote":"origin"}
    }));
    let issue = |number: u64, title: &str, body: String| {
        json!({
            "number":number,"url":format!("https://github.com/example/project/issues/{number}"),"title":title,
            "body":body,"labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"
        })
    };
    project.write(
        "issues.json",
        json!({"issues":[issue(1,"First",criterion("Implement first"))]}),
    );
    let ticket = |number: u64, text: &str| {
        json!({
            "id":format!("github:example/project#{number}"),"title":format!("Issue {number}"),
            "description":"Implement the issue", "acceptance_criteria":[text],
            "covers":[format!("github-example-project-{number}.md#ac-1")],"blocked_by":[]
        })
    };
    project.write("planning.json", json!({
        "tickets":[ticket(1,"Implement first")],"verification":{"outcome":"verified","findings":[]}
    }));
    project.write("scenario.json", json!({"tickets":{
        "github:example/project#1":{"implementation":{"files":{"first.txt":"done\n"},"outcome":"completed"},
            "review":{"standards":{"outcome":"approved","evidence":"Reviewed"},"spec":{"outcome":"approved","evidence":"Matches issue"}}}
    }}));
    project.write("github.json", json!({"pull_requests":[]}));
    project.write("repair.json", json!({"corrections":[],"reviews":[]}));
    let start = [
        "start-backlog",
        "--config",
        "kiln.json",
        "--github-repo",
        "example/project",
        "--issue-fixture",
        "issues.json",
        "--planning-fixture",
        "planning.json",
        "--run-fixture",
        "scenario.json",
        "--publication-fixture",
        "github.json",
        "--repair-fixture",
        "repair.json",
    ];
    let first = project.ok(&start);
    assert_eq!(first["backlog"]["dispositions"][0]["status"], "completed");
    assert_eq!(first["sessions"].as_array().unwrap().len(), 1);
    let published: Value =
        serde_json::from_slice(&fs::read(project.repo.join("github.json")).unwrap()).unwrap();
    assert_eq!(published["pull_requests"].as_array().unwrap().len(), 1);
    let repeated = project.ok(&start);
    assert_eq!(repeated["id"], first["id"]);
    assert_eq!(
        repeated["delivery_groups"][0]["pull_request"]["number"],
        first["delivery_groups"][0]["pull_request"]["number"]
    );
    let published: Value =
        serde_json::from_slice(&fs::read(project.repo.join("github.json")).unwrap()).unwrap();
    assert_eq!(published["pull_requests"].as_array().unwrap().len(), 1);

    let mut dependent = issue(3, "Dependent", criterion("Wait for first"));
    dependent["blocked_by"] = json!(["github:example/project#1"]);
    project.write("issues.json", json!({"issues":[
        issue(1,"First",criterion("Implement first")),issue(2,"Second",criterion("Implement second")),dependent
    ]}));
    project.write("planning.json", json!({
        "tickets":[ticket(2,"Implement second")],"verification":{"outcome":"verified","findings":[]}
    }));
    project.write("scenario.json", json!({"tickets":{
        "github:example/project#2":{"implementation":{"files":{"second.txt":"done\n"},"outcome":"completed"},
            "review":{"standards":{"outcome":"approved","evidence":"Reviewed"},"spec":{"outcome":"approved","evidence":"Matches issue"}}}
    }}));
    let second = project.ok(&start);
    assert_eq!(second["backlog"]["dispositions"][0]["status"], "completed");
    assert!(second["backlog"]["dispositions"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("previous backlog run"));
    assert_eq!(second["backlog"]["dispositions"][2]["status"], "blocked");
    assert!(second["backlog"]["dispositions"][2]["reason"]
        .as_str()
        .unwrap()
        .contains("unmerged pull request"));
    assert_eq!(second["plan"]["tickets"].as_array().unwrap().len(), 1);
    assert_eq!(
        second["plan"]["tickets"][0]["id"],
        "github:example/project#2"
    );
    assert_eq!(second["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        second["sessions"][0]["ticket_id"],
        "github:example/project#2"
    );
    let published: Value =
        serde_json::from_slice(&fs::read(project.repo.join("github.json")).unwrap()).unwrap();
    assert_eq!(published["pull_requests"].as_array().unwrap().len(), 2);

    let resumed = project.ok(&[
        "resume",
        second["id"].as_str().unwrap(),
        "--fixture",
        "scenario.json",
        "--publication-fixture",
        "github.json",
        "--repair-fixture",
        "repair.json",
    ]);
    assert_eq!(resumed["status"], "partial");
    assert_eq!(resumed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        resumed["delivery_groups"][0]["pull_request"]["number"],
        second["delivery_groups"][0]["pull_request"]["number"]
    );
    let published: Value =
        serde_json::from_slice(&fs::read(project.repo.join("github.json")).unwrap()).unwrap();
    assert_eq!(published["pull_requests"].as_array().unwrap().len(), 2);
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
