//! Public CLI coverage for the per-run event journal (`.kiln/runs/<id>/events.jsonl`)
//! and `kiln logs`.
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

const TICKET: &str = "github:example/project#1";

/// A target repository whose `origin` is on GitHub and whose publication
/// pushes to a local bare `fixture` remote; fixtures stand in for every agent.
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
        fs::write(target.repo.join("spec.md"), "# Spec\n\n- Works\n").unwrap();
        target.git(&["add", "."]);
        target.git(&["commit", "-qm", "initial"]);
        target.git(&["push", "-q", "fixture", "main"]);
        target.write("kiln.json", json!({
            "build":["git","diff","--check"], "test":["git","diff","--check"], "startup":["git","--version"],
            "acceptance_criteria":["Open issues are delivered after verification"],
            "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]},
            "validation":{"workflows":[
                {"criterion":"github-example-project-1.md#ac-1","command":["git","diff","--check"]}
            ]},
            "publication":{"github_repository":"example/project","target_branch":"main","remote":"fixture"}
        }));
        target.write(
            "issues.json",
            json!({"issues":[{"number":1,"url":"https://github.com/example/project/issues/1",
                "title":"Issue 1","body":"## Acceptance criteria\n- Implement 1\n",
                "labels":[],"comments":[],"assignee":null,"blocked_by":[],"state":"OPEN"}]}),
        );
        target.write(
            "planning.json",
            json!({"tickets":[{"id":TICKET,"title":"Issue 1","description":"Implement the issue",
                "acceptance_criteria":["Implement 1"],"covers":["github-example-project-1.md#ac-1"],"blocked_by":[]}],
                "verification":{"outcome":"verified","findings":[]}}),
        );
        target.scenario(None);
        target.write("github.json", json!({"pull_requests":[]}));
        target.write("repair.json", json!({"corrections":[],"reviews":[]}));
        target
    }
    /// Delivery scenario; `hold` makes implementation wait for `.kiln/release`.
    fn scenario(&self, hold: Option<&str>) {
        let approved = json!({"outcome":"approved","evidence":"Reviewed"});
        let mut ticket = json!({"implementation":{"files":{"first.txt":"done\n"},"outcome":"completed"},
            "review":{"standards":approved,"spec":approved}});
        if let Some(marker) = hold {
            ticket["await_file"] = json!(marker);
        }
        self.write("scenario.json", json!({"tickets":{ TICKET: ticket }}));
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
            .env("PATH", "/usr/bin:/bin");
        command
    }
    fn kiln(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn start_args(extra: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = [
            "start",
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
        ]
        .map(String::from)
        .to_vec();
        args.extend(extra.iter().map(|s| s.to_string()));
        args
    }
    fn start(&self, extra: &[&str]) -> Output {
        let args = Self::start_args(extra);
        self.kiln(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }
    fn spawn(&self, args: &[&str]) -> Child {
        self.command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
    fn runs(&self) -> Vec<String> {
        serde_json::from_str(&success(&self.kiln(&["inspect"]))).unwrap()
    }
    fn journal_path(&self, id: &str) -> PathBuf {
        self.repo.join(".kiln/runs").join(id).join("events.jsonl")
    }
    fn journal(&self, id: &str) -> Vec<Value> {
        fs::read_to_string(self.journal_path(id))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
            .collect()
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// (ticket, stage, status) of every event, in journal order.
fn transitions(events: &[Value]) -> Vec<(Option<String>, String, String)> {
    events
        .iter()
        .map(|e| {
            (
                e["ticket"].as_str().map(String::from),
                e["stage"].as_str().unwrap().to_owned(),
                e["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn expected_delivery() -> Vec<(Option<String>, String, String)> {
    let t = || Some(TICKET.to_owned());
    [
        (None, "run", "prepared"),
        (t(), "plan", "planned"),
        (None, "run", "running"),
        (t(), "implementation", "started"),
        (t(), "review", "passed"),
        (t(), "integration", "integrated"),
        (None, "run", "awaiting_validation"),
        (t(), "pull-request", "opened"),
        (t(), "ci", "pending"),
        (t(), "ci", "passed"),
        (None, "run", "completed"),
    ]
    .into_iter()
    .map(|(ticket, stage, status)| (ticket, stage.into(), status.into()))
    .collect()
}

#[test]
fn a_backlog_run_journals_every_pipeline_transition_in_order() {
    let target = Target::new();

    success(&target.start(&["--json"]));

    let id = &target.runs()[0];
    let raw = fs::read_to_string(target.journal_path(id)).unwrap();
    assert!(raw.ends_with('\n'), "one JSON object per line");
    let events = target.journal(id);
    assert_eq!(transitions(&events), expected_delivery(), "{raw}");
    for event in &events {
        let keys: Vec<_> = event.as_object().unwrap().keys().cloned().collect();
        for key in &keys {
            assert!(
                ["ts", "ticket", "stage", "status", "message"].contains(&key.as_str()),
                "unexpected field {key} in {event}"
            );
        }
        assert!(event["message"].is_string(), "{event}");
        let ts = event["ts"].as_str().expect("ts is a timestamp string");
        assert!(
            ts.len() >= 20 && ts.ends_with('Z') && ts.as_bytes()[10] == b'T',
            "RFC 3339 UTC timestamp: {ts}"
        );
    }
    let timestamps: Vec<_> = events.iter().map(|e| e["ts"].as_str().unwrap()).collect();
    assert!(
        timestamps.windows(2).all(|w| w[0] <= w[1]),
        "{timestamps:?}"
    );
    assert!(events[3]["message"].as_str().unwrap().contains("fixture"));
    let run: Value = serde_json::from_str(&success(&target.kiln(&["inspect", id]))).unwrap();
    let url = run["delivery_groups"][0]["pull_request"]["url"]
        .as_str()
        .unwrap();
    assert!(events[7]["message"].as_str().unwrap().contains(url));
}

#[test]
fn start_prints_one_readable_line_per_event() {
    let target = Target::new();

    let stdout = success(&target.start(&[]));

    let id = &target.runs()[0];
    let events = target.journal(id);
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| is_clock(line.get(..8).unwrap_or_default()))
        .collect();
    assert_eq!(lines.len(), events.len(), "{stdout}");
    for (line, event) in lines.iter().zip(&events) {
        assert!(serde_json::from_str::<Value>(line).is_err(), "{line}");
        for field in ["stage", "status"] {
            let value = event[field].as_str().unwrap();
            assert!(line.contains(value), "{line} lacks {value}");
        }
        if let Some(ticket) = event["ticket"].as_str() {
            assert!(line.contains(ticket), "{line} lacks {ticket}");
        }
    }
    let summary = stdout.find(&format!("Run {id}")).expect("final summary");
    assert!(
        stdout.find(lines.last().unwrap()).unwrap() < summary,
        "events print as they happen, before the outcome summary"
    );
}

fn is_clock(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 8
        && bytes[2] == b':'
        && bytes[5] == b':'
        && [0, 1, 3, 4, 6, 7]
            .iter()
            .all(|&i| bytes[i].is_ascii_digit())
}

#[test]
fn start_json_keeps_stdout_as_json() {
    let target = Target::new();

    let output = target.start(&["--json"]);

    let run: Value = serde_json::from_str(&success(&output)).unwrap();
    assert_eq!(run["status"], "completed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("implementation"), "{stderr}");
}

#[test]
fn logs_prints_the_journal_of_the_latest_or_a_given_run() {
    let target = Target::new();
    let planned = success(&target.kiln(&[
        "plan",
        "--issue-fixture",
        "issues.json",
        "--planning-fixture",
        "planning.json",
        "--json",
    ]));
    let planned: Value = serde_json::from_str(&planned).unwrap();
    let planned_id = planned["id"].as_str().unwrap().to_owned();
    success(&target.start(&["--fresh", "--json"]));
    let latest = target
        .runs()
        .into_iter()
        .find(|id| *id != planned_id)
        .unwrap();

    let all = success(&target.kiln(&["logs"]));
    let given = success(&target.kiln(&["logs", &planned_id]));

    assert_eq!(all.lines().count(), target.journal(&latest).len(), "{all}");
    assert!(all.lines().all(|line| is_clock(&line[..8])), "{all}");
    assert!(
        all.contains("pull-request") && all.contains("completed"),
        "{all}"
    );
    assert_eq!(
        given.lines().count(),
        target.journal(&planned_id).len(),
        "{given}"
    );
    assert!(
        given.contains("planned") && !given.contains("pull-request"),
        "{given}"
    );
    let unknown = target.kiln(&["logs", "run-0-0"]);
    assert!(!unknown.status.success());
}

#[test]
fn logs_follow_prints_new_events_and_exits_when_the_run_finishes() {
    let target = Target::new();
    target.scenario(Some(".kiln/release"));
    let args = Target::start_args(&["--json"]);
    let mut start = target.spawn(&args.iter().map(String::as_str).collect::<Vec<_>>());
    wait_until("implementation to start", || {
        target.runs().first().is_some_and(|id| {
            transitions(&target.journal(id))
                .iter()
                .any(|(_, stage, _)| stage == "implementation")
        })
    });
    let id = target.runs()[0].clone();

    let mut follow = target.spawn(&["logs", "-f"]);
    std::thread::sleep(Duration::from_millis(300));
    assert!(follow.try_wait().unwrap().is_none(), "follows a live run");
    fs::write(target.repo.join(".kiln/release"), "").unwrap();
    assert!(start.wait().unwrap().success());
    wait_until("logs -f to exit", || follow.try_wait().unwrap().is_some());

    assert!(follow.wait().unwrap().success());
    let mut stdout = String::new();
    follow
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert_eq!(
        stdout.lines().count(),
        target.journal(&id).len(),
        "{stdout}"
    );
    for appended in ["review", "pull-request", "completed"] {
        assert!(stdout.contains(appended), "missing {appended}:\n{stdout}");
    }
}

#[test]
fn deleting_or_truncating_the_journal_does_not_affect_resume() {
    for damage in ["delete", "truncate"] {
        let target = Target::new();
        let args = Target::start_args(&["--json"]);
        let crashed = target
            .command(&args.iter().map(String::as_str).collect::<Vec<_>>())
            .env(
                "KILN_FAULT_INJECT",
                format!("integration.after_merge@{TICKET}"),
            )
            .output()
            .unwrap();
        assert_eq!(crashed.status.code(), Some(86), "{damage}");
        let id = target.runs()[0].clone();
        let journal = target.journal_path(&id);
        assert!(journal.exists());
        match damage {
            "delete" => fs::remove_file(&journal).unwrap(),
            _ => fs::write(&journal, "{\"ts\":\"2026-").unwrap(),
        }

        let resumed = success(&target.kiln(&[
            "resume",
            &id,
            "--fixture",
            "scenario.json",
            "--publication-fixture",
            "github.json",
            "--repair-fixture",
            "repair.json",
        ]));

        let run: Value = serde_json::from_str(&resumed).unwrap();
        assert_eq!(run["status"], "completed", "{damage}: {run:#}");
        assert_eq!(run["delivery_groups"][0]["status"], "verified");
        let sessions = run["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1, "{damage}: no work is repeated");
    }
}

#[test]
fn logs_says_when_a_run_has_no_journal() {
    let target = Target::new();
    let none = success(&target.kiln(&["logs"]));
    assert!(none.to_lowercase().contains("no runs"), "{none}");
    let prepared =
        success(&target.kiln(&["prepare", "--config", "kiln.json", "--spec", "spec.md"]));
    let id = serde_json::from_str::<Value>(&prepared).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    for args in [vec!["logs"], vec!["logs", id.as_str()]] {
        let output = success(&target.kiln(&args));
        assert!(
            output.contains("no journal") && output.contains(&id),
            "{args:?}: {output}"
        );
    }
}

#[test]
fn logs_is_listed_in_help() {
    let target = Target::new();
    let help = success(&target.kiln(&["--help"]));
    assert!(
        help.lines()
            .any(|line| line.trim_start().starts_with("logs ")),
        "{help}"
    );
}
