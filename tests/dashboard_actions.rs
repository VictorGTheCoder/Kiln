//! Dashboard action buttons: Plan, Start, Pause, Resume and Cancel over real
//! HTTP against `kiln dashboard`, using the same default inference as the CLI.
//! Fixtures stand in for the provider and GitHub.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const FIRST: &str = "github:example/project#1";
const SECOND: &str = "github:example/project#2";

/// Hidden dashboard flags that hand fixtures to the runs its buttons launch.
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

/// A target repository whose `origin` is on GitHub, with two issues where #2
/// waits for #1. When `hold` is set, ticket #1 holds its implementation until
/// `.kiln/release` exists.
struct Target {
    _temp: tempfile::TempDir,
    repo: PathBuf,
}
impl Target {
    fn new(hold: bool) -> Self {
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
        target.planning("verified");
        let approved = json!({"outcome":"approved","evidence":"Reviewed"});
        let mut first = json!({"implementation":{"files":{"first.txt":"done\n"},"outcome":"completed"},
            "review":{"standards":approved,"spec":approved}});
        let mut second = json!({"implementation":{"files":{"second.txt":"done\n"},"outcome":"completed"},
            "review":{"standards":approved,"spec":approved}});
        if hold {
            first["await_file"] = json!(".kiln/release");
            second["await_file"] = json!(".kiln/release-second");
        }
        target.write(
            "scenario.json",
            json!({"tickets":{FIRST:first, SECOND:second}}),
        );
        target.write("github.json", json!({"pull_requests":[]}));
        target.write("repair.json", json!({"corrections":[],"reviews":[]}));
        target
    }
    /// Planning fixture; any outcome but `verified` rejects the plan.
    fn planning(&self, outcome: &str) {
        let ticket = |n: u64, blocked_by: Vec<&str>| {
            json!({"id":format!("github:example/project#{n}"),"title":format!("Issue {n}"),
                "description":"Implement the issue","acceptance_criteria":[format!("Implement {n}")],
                "covers":[format!("github-example-project-{n}.md#ac-1")],"blocked_by":blocked_by})
        };
        let findings = if outcome == "verified" {
            json!([])
        } else {
            json!([{"code":"planned-twice","message":"planning must not run again"}])
        };
        self.write(
            "planning.json",
            json!({"tickets":[ticket(1, vec![]), ticket(2, vec![FIRST])],
                "verification":{"outcome":outcome,"findings":findings}}),
        );
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
            if let Some(id) = self.runs().first() {
                let out = self.kiln(&["inspect", id]);
                if out.status.success() {
                    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
                    if ready(&run) {
                        return id.clone();
                    }
                }
            }
            thread::sleep(Duration::from_millis(30));
        }
        panic!("the run never reached: {what}");
    }
    fn pull_requests(&self) -> usize {
        let github: Value =
            serde_json::from_slice(&fs::read(self.repo.join("github.json")).unwrap()).unwrap();
        github["pull_requests"].as_array().unwrap().len()
    }
    /// Start `kiln dashboard` with `args` and read its announced address.
    fn dashboard(&self, args: &[&str]) -> Server {
        let mut command = self.command(&["dashboard", "--bind", "127.0.0.1:0"]);
        command.args(args);
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line
            .trim()
            .strip_prefix("Kiln web view: http://")
            .unwrap_or_else(|| panic!("dashboard announces its URL, got {line:?}"))
            .to_owned();
        Server { child, address }
    }
}
fn ticket_state(run: &Value, ticket: &str) -> Option<String> {
    run["scheduler"]["tickets"]
        .as_array()?
        .iter()
        .find(|t| t["id"] == ticket)
        .and_then(|t| t["state"].as_str().map(str::to_owned))
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
fn json_of(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Server {
    child: Child,
    address: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Reply {
    status: u16,
    body: String,
}
impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|_| panic!("JSON reply, got {}: {}", self.status, self.body))
    }
}
impl Server {
    fn local(&self) -> String {
        format!("localhost:{}", self.address.rsplit(':').next().unwrap())
    }
    fn request(&self, head: &str) -> Reply {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(stream, "{head}Connection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        Reply {
            status: head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            body: body.to_owned(),
        }
    }
    fn get(&self, path: &str) -> Reply {
        self.request(&format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\n",
            self.local()
        ))
    }
    /// POST as the dashboard page's own script does: same-origin, empty form body.
    fn post_from(&self, path: &str, host: &str, origin: &str) -> Reply {
        self.request(&format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 0\r\n"
        ))
    }
    fn post(&self, path: &str) -> Reply {
        let host = self.local();
        self.post_from(path, &host, &format!("http://{host}"))
    }
    /// Actions currently offered: globally, and per run id.
    fn actions(&self) -> Value {
        let reply = self.get("/api/dashboard/actions");
        assert_eq!(reply.status, 200, "{}", reply.body);
        reply.json()
    }
    /// Wait until the last launched action finished, and return it.
    fn finished_action(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let last = self.actions()["last"].clone();
            if last["state"] != "running" {
                return last;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("the action never finished");
    }
}
fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

#[test]
fn plan_then_start_deliver_the_backlog_reusing_the_unchanged_plan() {
    let target = Target::new(false);
    let server = target.dashboard(FIXTURES);
    assert_eq!(
        names(&server.actions()["available"]),
        ["plan", "start"],
        "with no run yet, only Plan and Start make sense"
    );

    let reply = server.post("/api/dashboard/actions/plan");
    assert_eq!(reply.status, 202, "{}", reply.body);
    assert_eq!(reply.json()["action"], "plan");
    let last = server.finished_action();
    assert_eq!(last["state"], "succeeded", "{last:#}");
    let planned = target.wait_for("planned", |run| run["status"] == "planned");
    assert!(target.run(&planned)["scheduler"].is_null());

    // Planning again would now be rejected: Start must deliver the recorded plan.
    target.planning("rejected");
    let reply = server.post("/api/dashboard/actions/start");
    assert_eq!(reply.status, 202, "{}", reply.body);
    let last = server.finished_action();
    assert_eq!(last["state"], "succeeded", "{last:#}");
    assert_eq!(target.runs(), [planned.as_str()], "start reused the plan");
    let run = target.run(&planned);
    assert_eq!(run["status"], "completed", "{run:#}");
    assert_eq!(target.pull_requests(), 1);
    assert!(
        server.get("/api/dashboard/actions").json()["runs"][&planned]
            .as_array()
            .unwrap()
            .is_empty(),
        "a completed run offers no run action"
    );
}

#[test]
fn pause_resume_and_cancel_drive_the_selected_run() {
    let target = Target::new(true);
    let server = target.dashboard(FIXTURES);
    let reply = server.post("/api/dashboard/actions/start");
    assert_eq!(reply.status, 202, "{}", reply.body);
    let id = target.wait_for("ticket #1 implementing", |run| {
        run["status"] == "running" && ticket_state(run, FIRST).as_deref() == Some("implementing")
    });
    let offered = server.actions();
    assert_eq!(names(&offered["runs"][&id]), ["pause", "cancel"]);
    assert!(
        names(&offered["available"]).is_empty(),
        "Plan and Start are disabled while a run is active"
    );

    let reply = server.post(&format!("/api/dashboard/runs/{id}/pause"));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.json()["action"], "pause");
    target.release();
    target.wait_for("paused", |run| run["status"] == "paused");
    assert_eq!(server.finished_action()["action"], "start");
    let offered = server.actions();
    assert_eq!(names(&offered["runs"][&id]), ["resume"]);
    assert_eq!(names(&offered["available"]), ["plan", "start"]);

    let reply = server.post(&format!("/api/dashboard/runs/{id}/resume"));
    assert_eq!(reply.status, 202, "{}", reply.body);
    target.wait_for("ticket #2 implementing", |run| {
        run["status"] == "running" && ticket_state(run, SECOND).as_deref() == Some("implementing")
    });
    let reply = server.post(&format!("/api/dashboard/runs/{id}/cancel"));
    assert_eq!(reply.status, 200, "{}", reply.body);
    let last = server.finished_action();
    assert_eq!(last["action"], "resume");
    assert_eq!(target.run(&id)["status"], "cancelled");
    assert!(
        names(&server.actions()["runs"][&id]).is_empty(),
        "a cancelled run offers no action"
    );
}

#[test]
fn actions_from_a_foreign_origin_or_host_are_rejected() {
    let target = Target::new(false);
    let server = target.dashboard(FIXTURES);
    let local = server.local();
    for (host, origin, status) in [
        (local.as_str(), "http://evil.example", 403),
        (local.as_str(), "null", 403),
        ("evil.example", "http://evil.example", 400),
    ] {
        for path in [
            "/api/dashboard/actions/plan",
            "/api/dashboard/actions/start",
        ] {
            let reply = server.post_from(path, host, origin);
            assert_eq!(reply.status, status, "{path} from {origin}: {}", reply.body);
        }
    }
    let missing_origin = server.request(&format!(
        "POST /api/dashboard/actions/start HTTP/1.1\r\nHost: {local}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 0\r\n"
    ));
    assert_eq!(missing_origin.status, 403);
    thread::sleep(Duration::from_millis(300));
    assert!(
        target.runs().is_empty(),
        "no rejected action launched a run"
    );
    assert!(server.actions()["last"].is_null());
}

#[test]
fn a_failing_action_shows_the_same_error_as_the_cli() {
    // No fixtures and no provider on PATH: inference fails before launching.
    let target = Target::new(false);
    let server = target.dashboard(&[]);
    let cli = target.kiln(&["plan"]);
    assert!(!cli.status.success());
    let cli_error = String::from_utf8_lossy(&cli.stderr)
        .trim()
        .strip_prefix("Kiln: ")
        .expect("the CLI prints `Kiln: <error>`")
        .to_owned();
    assert!(cli_error.contains("no provider found"), "{cli_error}");
    for action in ["plan", "start"] {
        let reply = server.post(&format!("/api/dashboard/actions/{action}"));
        assert_eq!(reply.status, 409, "{}", reply.body);
        assert_eq!(reply.json()["error"], cli_error.as_str());
    }
    assert!(server.actions()["last"].is_null(), "nothing was launched");

    // A failure after launch is reported with the error the command printed.
    let target = Target::new(false);
    target.planning("rejected");
    let cli = target.kiln(&[
        "plan",
        "--issue-fixture",
        "issues.json",
        "--planning-fixture",
        "planning.json",
    ]);
    let cli_error = String::from_utf8_lossy(&cli.stderr).trim().to_owned();
    let cli_error = cli_error.strip_prefix("Kiln: ").unwrap_or(&cli_error);
    let server = target.dashboard(FIXTURES);
    let reply = server.post("/api/dashboard/actions/plan");
    assert_eq!(reply.status, 202, "{}", reply.body);
    let last = server.finished_action();
    assert_eq!(last["state"], "failed", "{last:#}");
    assert_eq!(last["error"], cli_error, "{last:#}");
    assert!(cli_error.contains("plan rejected"), "{cli_error}");

    // Resuming when nothing is resumable reports the CLI's message.
    let reply = server.post("/api/dashboard/runs/run-1-1/resume");
    assert_eq!(reply.status, 409);
    let cli = target.kiln(&["resume"]);
    let cli_error = String::from_utf8_lossy(&cli.stderr).trim().to_owned();
    assert_eq!(
        format!("Kiln: {}", reply.json()["error"].as_str().unwrap()),
        cli_error
    );
}

#[test]
fn dashboard_page_has_the_action_buttons() {
    let target = Target::new(false);
    let server = target.dashboard(&[]);
    let page = server.get("/dashboard").body;
    let script = server.get("/dashboard.js").body;
    for action in ["plan", "start", "pause", "resume", "cancel"] {
        assert!(
            page.contains(&format!("data-action=\"{action}\"")),
            "page has a {action} button"
        );
    }
    assert!(
        script.contains("/api/dashboard/actions"),
        "script reads offered actions"
    );
    assert!(script.contains("application/x-www-form-urlencoded"));
}
