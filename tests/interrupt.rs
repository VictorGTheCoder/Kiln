//! Public CLI coverage for Ctrl-C during `kiln start`: SIGINT pauses the run at
//! a safe point, `kiln resume` continues it without repeating completed effects,
//! and `kiln pause` / `kiln cancel` act on the latest active run.
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const FIRST: &str = "github:example/project#1";
const SECOND: &str = "github:example/project#2";

/// Fixtures that stand in for the provider and GitHub.
const FIXTURES: &[&str] = &[
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
const RESUME_FIXTURES: &[&str] = &[
    "--fixture",
    "scenario.json",
    "--publication-fixture",
    "github.json",
    "--repair-fixture",
    "repair.json",
];

/// A target repository with two issues where #2 waits for #1. Ticket #1 holds
/// its implementation until `.kiln/release` exists.
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
            "publication":{"github_repository":"example/project","target_branch":"main","remote":"fixture",
                "required_checks_timeout_seconds":60,"required_checks_poll_seconds":1}
        }));
        let issue = |n: u64, blocked_by: Vec<&str>| {
            json!({"number":n,"url":format!("https://github.com/example/project/issues/{n}"),
                "title":format!("Issue {n}"),"body":format!("## Acceptance criteria\n- Implement {n}\n"),
                "labels":[],"comments":[],"assignee":null,"blocked_by":blocked_by,"state":"OPEN"})
        };
        target.write(
            "issues.json",
            json!({"issues":[issue(1, vec![]), issue(2, vec![FIRST])]}),
        );
        let ticket = |n: u64, blocked_by: Vec<&str>| {
            json!({"id":format!("github:example/project#{n}"),"title":format!("Issue {n}"),
                "description":"Implement the issue","acceptance_criteria":[format!("Implement {n}")],
                "covers":[format!("github-example-project-{n}.md#ac-1")],"blocked_by":blocked_by})
        };
        target.write(
            "planning.json",
            json!({"tickets":[ticket(1, vec![]), ticket(2, vec![FIRST])],
                "verification":{"outcome":"verified","findings":[]}}),
        );
        let approved = json!({"outcome":"approved","evidence":"Reviewed"});
        target.write(
            "scenario.json",
            json!({"tickets":{
                FIRST:{"implementation":{"files":{"first.txt":"done\n"},"outcome":"completed"},
                    "review":{"standards":approved,"spec":approved},"await_file":".kiln/release"},
                SECOND:{"implementation":{"files":{"second.txt":"done\n"},"outcome":"completed"},
                    "review":{"standards":approved,"spec":approved}}
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
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln"));
        command
            .args(args)
            .current_dir(&self.repo)
            // No real provider may be found: fixtures stand in for every agent.
            .env("PATH", "/usr/bin:/bin");
        command
    }
    fn kiln(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    /// Spawn `kiln start` in the background with every fixture.
    fn spawn_start(&self) -> Child {
        self.command(&[&["start"], FIXTURES].concat())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
    fn release(&self) {
        fs::write(self.repo.join(".kiln/release"), "").unwrap();
    }
    fn runs(&self) -> Vec<String> {
        serde_json::from_value(json_of(&self.kiln(&["inspect"]))).unwrap()
    }
    fn run(&self, id: &str) -> Value {
        json_of(&self.kiln(&["inspect", id]))
    }
    /// Poll the only run until `ready` holds.
    fn wait_for(&self, what: &str, ready: impl Fn(&Value) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let out = self.kiln(&["inspect"]);
            if out.status.success() {
                let ids: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
                if let Some(id) = ids.first() {
                    let out = self.kiln(&["inspect", id]);
                    if out.status.success() {
                        let run: Value = serde_json::from_slice(&out.stdout).unwrap();
                        if ready(&run) {
                            return id.clone();
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(30));
        }
        panic!("the run never reached: {what}");
    }
    fn wait_until_implementing_first(&self) -> String {
        self.wait_for("ticket #1 implementing", |run| {
            run["status"] == "running"
                && ticket_state(run, FIRST).as_deref() == Some("implementing")
        })
    }
    fn pull_requests(&self) -> usize {
        let github: Value =
            serde_json::from_slice(&fs::read(self.repo.join("github.json")).unwrap()).unwrap();
        github["pull_requests"].as_array().unwrap().len()
    }
    fn sessions_for(&self, id: &str, ticket: &str) -> usize {
        self.run(id)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["ticket_id"] == ticket)
            .count()
    }
}
fn ticket_state(run: &Value, ticket: &str) -> Option<String> {
    run["scheduler"]["tickets"]
        .as_array()?
        .iter()
        .find(|t| t["id"] == ticket)
        .and_then(|t| t["state"].as_str().map(str::to_owned))
}
fn sigint(child: &Child) {
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
}
/// Wait for the child to exit within a deadline and collect its output.
fn finish(mut child: Child) -> (std::process::ExitStatus, String, String) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("kiln did not exit after SIGINT");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    (status, stdout, stderr)
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
    serde_json::from_str(&success(output)).expect("JSON output")
}

#[test]
fn ctrl_c_during_start_pauses_the_run_and_resume_completes_it() {
    let target = Target::new();
    let child = target.spawn_start();
    let id = target.wait_until_implementing_first();

    sigint(&child);
    // The active ticket reaches its safe point only once released.
    thread::sleep(Duration::from_millis(300));
    target.release();
    let (status, _stdout, stderr) = finish(child);

    assert!(status.success(), "clean exit after Ctrl-C: {stderr}");
    assert!(stderr.contains("Pausing"), "{stderr}");
    assert!(stderr.contains("kiln resume"), "{stderr}");
    let run = target.run(&id);
    assert_eq!(run["status"], "paused", "{run:#}");
    assert_eq!(ticket_state(&run, FIRST).as_deref(), Some("integrated"));
    assert_eq!(ticket_state(&run, SECOND).as_deref(), Some("waiting"));
    assert_eq!(
        target.pull_requests(),
        0,
        "nothing is published while paused"
    );

    let resumed = target.kiln(&[&["resume"], RESUME_FIXTURES].concat());

    success(&resumed);
    let run = target.run(&id);
    assert_eq!(run["status"], "completed", "{run:#}");
    assert_eq!(target.runs(), vec![id.clone()]);
    assert_eq!(
        target.sessions_for(&id, FIRST),
        1,
        "#1 is not implemented again"
    );
    assert_eq!(target.sessions_for(&id, SECOND), 1);
    assert_eq!(target.pull_requests(), 1);
}

#[test]
fn pause_and_cancel_without_an_id_act_on_the_latest_active_run() {
    let target = Target::new();
    let out = target.kiln(&["pause"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no active run"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let child = target.spawn_start();
    let id = target.wait_until_implementing_first();
    let paused = success(&target.kiln(&["pause"]));
    assert!(paused.contains(&id), "{paused}");
    target.release();
    let (status, _, stderr) = finish(child);
    assert!(status.success(), "{stderr}");
    assert_eq!(target.run(&id)["status"], "paused");

    fs::remove_file(target.repo.join(".kiln/release")).unwrap();
    let mut scenario: Value =
        serde_json::from_slice(&fs::read(target.repo.join("scenario.json")).unwrap()).unwrap();
    scenario["tickets"][SECOND]["await_file"] = json!(".kiln/release");
    target.write("scenario.json", scenario);
    let child = target
        .command(&[&["resume"], RESUME_FIXTURES].concat())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    target.wait_for("ticket #2 implementing", |run| {
        run["status"] == "running" && ticket_state(run, SECOND).as_deref() == Some("implementing")
    });
    let cancelled = success(&target.kiln(&["cancel"]));
    assert!(cancelled.contains(&id), "{cancelled}");
    let (_, _, stderr) = finish(child);
    let run = target.run(&id);
    assert_eq!(run["status"], "cancelled", "{stderr}\n{run:#}");
    assert_eq!(target.pull_requests(), 0);
}

#[test]
fn repeated_ctrl_c_while_awaiting_ci_pauses_once_and_resume_keeps_the_pull_request() {
    let target = Target::new();
    fs::create_dir_all(target.repo.join(".kiln")).unwrap();
    target.release();
    // Required CI stays pending until the fixture is changed below.
    target.write(
        "github.json",
        json!({"pull_requests":[],"required_check_names":["linux"],"check_runs":[]}),
    );
    let child = target.spawn_start();
    let id = target.wait_for("a pull request awaiting CI", |run| {
        run["status"] == "running"
            && run["delivery_groups"][0]["status"] == "awaiting-ci"
            && run["delivery_groups"][0]["pull_request"].is_object()
    });

    sigint(&child);
    thread::sleep(Duration::from_millis(100));
    sigint(&child);
    sigint(&child);
    let (status, _stdout, stderr) = finish(child);

    assert!(status.success(), "clean exit after Ctrl-C: {stderr}");
    assert!(stderr.contains("Still pausing"), "{stderr}");
    let run = target.run(&id);
    assert_eq!(run["status"], "paused", "{run:#}");
    assert_eq!(target.pull_requests(), 1);

    let mut github: Value =
        serde_json::from_slice(&fs::read(target.repo.join("github.json")).unwrap()).unwrap();
    github["check_runs"] = json!([{"pull_request":1,"commit":"current","name":"linux",
        "status":"completed","conclusion":"success","id":"run-1"}]);
    target.write("github.json", github);
    let resumed = target.kiln(&[&["resume", "--json"], RESUME_FIXTURES].concat());

    let resumed = json_of(&resumed);
    assert_eq!(resumed["id"], id.as_str());
    assert_eq!(resumed["status"], "completed", "{resumed:#}");
    assert_eq!(
        target.pull_requests(),
        1,
        "the pull request is not opened again"
    );
    assert_eq!(target.sessions_for(&id, FIRST), 1);
    assert_eq!(target.sessions_for(&id, SECOND), 1);
}

#[test]
fn ctrl_c_requests_the_pause_once_instead_of_rewriting_it() {
    use std::os::unix::fs::MetadataExt;
    let target = Target::new();
    let child = target.spawn_start();
    let id = target.wait_until_implementing_first();
    let control = target.repo.join(".kiln/runs").join(format!("{id}.control"));

    sigint(&child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !control.exists() {
        assert!(Instant::now() < deadline, "Ctrl-C never requested a pause");
        thread::sleep(Duration::from_millis(10));
    }
    let stamp = |path: &Path| {
        let metadata = fs::metadata(path).unwrap();
        (metadata.ino(), metadata.mtime(), metadata.mtime_nsec())
    };
    let first = stamp(&control);
    thread::sleep(Duration::from_millis(400));
    let later = stamp(&control);
    target.release();
    let (status, _stdout, stderr) = finish(child);

    assert_eq!(first, later, "the pause request was written again");
    assert!(status.success(), "clean exit after Ctrl-C: {stderr}");
    assert_eq!(target.run(&id)["status"], "paused");
}
