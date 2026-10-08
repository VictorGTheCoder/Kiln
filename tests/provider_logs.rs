//! Public CLI coverage for live provider logs: `kiln start` with a fake
//! provider on PATH streams every redacted provider line to
//! `.kiln/runs/<id>/agents/<ticket>-<stage>.log`, and `--verbose` also prints a
//! readable summary of the provider's activity. `kiln dashboard` streams the
//! same lines, with their summaries, over its run event stream.
#[path = "support/sse.rs"]
mod sse;
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

const CODEX_ARGV: &[&str] = &[
    "/codex/codex",
    "exec",
    "--json",
    "--ephemeral",
    "--ignore-user-config",
    "--ignore-rules",
    "--color",
    "never",
    "--sandbox",
    "danger-full-access",
    "-c",
    "approval_policy=\"never\"",
    "-c",
    "features.code_mode=false",
    "-",
];
const CLAUDE_ARGV: &[&str] = &[
    "/claude/claude",
    "-p",
    "--output-format",
    "stream-json",
    "--verbose",
    "--no-session-persistence",
    "--permission-mode",
    "bypassPermissions",
    "--strict-mcp-config",
];
/// Registered secret handed to the provider through `isolation.secrets`.
const SECRET: &str = "registered-secret-4711";
const TICKET_FILE: &str = "github-example-project-1";
/// Marker the implementation prints before it sleeps.
const FIRST_LINE: &str = "first-line-before-sleep";

/// Target repository with one open issue planned by fixture; the provider
/// (implementation and review) is a fake `codex` or `claude` on PATH.
struct Target {
    temp: tempfile::TempDir,
    repo: PathBuf,
    bin: PathBuf,
}
impl Target {
    fn new(provider: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let target = Self { temp, repo, bin };
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
        fs::write(target.repo.join(".gitignore"), ".kiln/\n").unwrap();
        target.git(&["add", "."]);
        target.git(&["commit", "-qm", "initial"]);
        let auth = target.temp.path().join("auth.json");
        fs::write(&auth, r#"{"tokens":{"access_token":"private-auth-value"}}"#).unwrap();
        let credentials = target.temp.path().join("credentials.json");
        fs::write(
            &credentials,
            r#"{"claudeAiOauth":{"accessToken":"private-auth-value"}}"#,
        )
        .unwrap();
        target.write("kiln.json", json!({
            "agent": provider,
            "build":["git","diff","--check"], "test":["git","diff","--check"], "startup":["git","--version"],
            "acceptance_criteria":["Open issues are delivered after verification"],
            "codex":{"auth": auth, "timeout_seconds": 30},
            "claude":{"credentials": credentials, "timeout_seconds": 30},
            "isolation":{"network":"allow-all","runtime":"system",
                "commands":[["git","diff","--check"],["git","--version"], CODEX_ARGV, CLAUDE_ARGV],
                "secrets":{"KILN_TEST_SECRET":["agent"]}}
        }));
        target.write(
            "issues.json",
            json!({"issues":[{
            "number":1,"url":"https://github.com/example/project/issues/1","title":"Issue 1",
            "body":"## Acceptance criteria\n- Implement 1\n","labels":[],"comments":[],
            "assignee":null,"blocked_by":[],"state":"OPEN"}]}),
        );
        target.write("planning.json", json!({
            "tickets":[{"id":"github:example/project#1","title":"Issue 1","description":"Implement the issue",
                "acceptance_criteria":["Implement 1"],"covers":["github-example-project-1.md#ac-1"],"blocked_by":[]}],
            "verification":{"outcome":"verified","findings":[]}}));
        target.provider(provider);
        target
    }
    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }
    fn write(&self, path: &str, value: Value) {
        fs::write(self.repo.join(path), value.to_string()).unwrap();
    }
    /// Fake provider. Reviews approve; the implementation prints a line,
    /// sleeps, then reports a command, a file edit, a message, a registered
    /// secret, a malformed line and an unknown event before completing.
    fn provider(&self, name: &str) {
        let review = json!({"outcome":"approved","findings":[],"evidence":"Reviewed first.txt","log":"read first.txt"}).to_string();
        let (review_events, work_events) = match name {
            "codex" => (
                vec![
                    json!({"type":"thread.started","thread_id":"review"}),
                    json!({"type":"item.completed","item":{"type":"agent_message","text":review}}),
                    json!({"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}),
                ],
                vec![
                    json!({"type":"item.completed","item":{"type":"command_execution","command":"echo done > first.txt","exit_code":0,"status":"completed"}}),
                    json!({"type":"item.completed","item":{"type":"file_change","changes":[{"path":"first.txt","kind":"add"}],"status":"completed"}}),
                    json!({"type":"item.completed","item":{"type":"agent_message","text":"Implemented ticket one"}}),
                    json!({"type":"mystery.event","detail":"unknown to kiln"}),
                    json!({"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}),
                ],
            ),
            _ => (
                vec![
                    json!({"type":"system","subtype":"init","session_id":"review"}),
                    json!({"type":"result","subtype":"success","is_error":false,"result":review,"session_id":"review"}),
                ],
                vec![
                    json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"echo done > first.txt"}}]}}),
                    json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"first.txt","content":"done\n"}}]}}),
                    json!({"type":"assistant","message":{"content":[{"type":"text","text":"Implemented ticket one"}]}}),
                    json!({"type":"mystery.event","detail":"unknown to kiln"}),
                    json!({"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"work"}),
                ],
            ),
        };
        let lines = |events: &[Value]| -> String {
            events
                .iter()
                .map(|e| format!("printf '%s\\n' '{e}'\n"))
                .collect()
        };
        let first = match name {
            "codex" => json!({"type":"thread.started","thread_id":FIRST_LINE}),
            _ => json!({"type":"system","subtype":"init","session_id":FIRST_LINE}),
        };
        let script = format!(
            "#!/bin/sh\nprompt=$(cat)\ncase \"$prompt\" in\n\
             *\"Return only one JSON object with exactly these field types\"*)\n{review}\n;;\n\
             *)\nprintf '%s\\n' '{first}'\nsleep 3\necho done > first.txt\n\
             printf '%s\\n' \"secret $KILN_TEST_SECRET and private-auth-value\"\n\
             echo 'this line is not json {{'\n{work}\n;;\nesac\n",
            review = lines(&review_events),
            work = lines(&work_events),
        );
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln"));
        command
            .args(args)
            .current_dir(&self.repo)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.bin.to_str().unwrap()),
            )
            .env("KILN_TEST_SECRET", SECRET);
        command
    }
    fn start_args(extra: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = [
            "start",
            "--issue-fixture",
            "issues.json",
            "--planning-fixture",
            "planning.json",
        ]
        .map(String::from)
        .to_vec();
        args.extend(extra.iter().map(|s| s.to_string()));
        args
    }
    fn start(&self, extra: &[&str]) -> Output {
        let args = Self::start_args(extra);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.command(&args).output().unwrap()
    }
    fn spawn_start(&self) -> Child {
        let args = Self::start_args(&[]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.command(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
    fn run_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.repo.join(".kiln/runs")) else {
            return Vec::new();
        };
        entries
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_dir())
            .collect()
    }
    /// The only run's `agents/` directory.
    fn agents(&self) -> PathBuf {
        let runs = self.run_dirs();
        assert_eq!(runs.len(), 1, "{runs:?}");
        runs[0].join("agents")
    }
    fn run(&self) -> Value {
        let runs = self.run_dirs();
        let id = runs[0].file_name().unwrap().to_str().unwrap().to_owned();
        let out = self.command(&["inspect", &id]).output().unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}
/// The run reached delivery: its only ticket was implemented, reviewed and
/// integrated. Publication is deliberately not configured here.
fn assert_ticket_integrated(target: &Target, output: &Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("publication is not configured"),
        "run must only stop at delivery; stderr: {stderr}"
    );
    let run = target.run();
    assert_eq!(
        run["scheduler"]["tickets"][0]["state"], "integrated",
        "{run:#}"
    );
}

#[test]
fn provider_lines_reach_the_session_log_while_the_provider_runs() {
    let target = Target::new("codex");
    let mut child = target.spawn_start();
    let deadline = Instant::now() + Duration::from_secs(30);
    let seen_live = loop {
        let log = target
            .run_dirs()
            .into_iter()
            .map(|dir| read(&dir.join(format!("agents/{TICKET_FILE}-implementation.log"))))
            .collect::<String>();
        let running = child.try_wait().unwrap().is_none();
        if log.contains(FIRST_LINE) {
            assert!(
                !log.contains("Implemented ticket one"),
                "the log must hold the first line before the provider finished: {log}"
            );
            break running;
        }
        if !running || Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = child.wait();
    assert!(
        seen_live,
        "the implementation log did not contain the first provider line while kiln ran"
    );
}

#[test]
fn each_ticket_stage_has_one_redacted_log_that_keeps_unknown_and_malformed_lines() {
    let target = Target::new("codex");

    let output = target.start(&[]);

    assert_ticket_integrated(&target, &output);
    let agents = target.agents();
    let mut names: Vec<String> = fs::read_dir(&agents)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            format!("{TICKET_FILE}-implementation.log"),
            format!("{TICKET_FILE}-review.log"),
        ]
    );
    let implementation = read(&agents.join(format!("{TICKET_FILE}-implementation.log")));
    for kept in [
        FIRST_LINE,
        "this line is not json {",
        "mystery.event",
        "Implemented ticket one",
    ] {
        assert!(
            implementation.contains(kept),
            "missing {kept:?} in:\n{implementation}"
        );
    }
    assert!(
        implementation.contains("secret [REDACTED] and [REDACTED]"),
        "{implementation}"
    );
    let review = read(&agents.join(format!("{TICKET_FILE}-review.log")));
    assert!(review.contains("Reviewed first.txt"), "{review}");
    for log in [&implementation, &review] {
        assert!(
            !log.contains(SECRET) && !log.contains("private-auth-value"),
            "{log}"
        );
    }
}

#[test]
fn verbose_prints_readable_codex_and_claude_activity_and_is_quiet_without_it() {
    for provider in ["codex", "claude"] {
        let target = Target::new(provider);
        let verbose = target.start(&["--verbose"]);
        assert_ticket_integrated(&target, &verbose);
        let stdout = String::from_utf8_lossy(&verbose.stdout);
        for summary in [
            "github:example/project#1  implementation | ran: echo done > first.txt",
            "github:example/project#1  implementation | edited: first.txt",
            "github:example/project#1  implementation | message: Implemented ticket one",
        ] {
            assert!(
                stdout.contains(summary),
                "{provider}: missing {summary:?} in:\n{stdout}"
            );
        }
        assert!(!stdout.contains(SECRET) && !stdout.contains("private-auth-value"));

        let target = Target::new(provider);
        let quiet = target.start(&[]);
        assert_ticket_integrated(&target, &quiet);
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&quiet.stdout),
            String::from_utf8_lossy(&quiet.stderr)
        );
        for summary in ["ran: ", "edited: ", "message: Implemented"] {
            assert!(
                !all.contains(summary),
                "{provider}: unexpected {summary:?} in:\n{all}"
            );
        }
    }
}

/// `kiln dashboard` serving the target repository.
struct Dashboard {
    child: Child,
    address: String,
}
impl Drop for Dashboard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Dashboard {
    fn start(target: &Target) -> Self {
        let mut child = target
            .command(&["dashboard", "--bind", "127.0.0.1:0"])
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
        Self { child, address }
    }
    fn events(&self, id: &str, last_event_id: Option<&str>) -> sse::Stream {
        let port = self.address.rsplit(':').next().unwrap();
        sse::connect(
            &self.address,
            id,
            last_event_id,
            &format!("localhost:{port}"),
        )
    }
}

fn provider_events(frames: &[sse::Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.event == "provider")
        .map(sse::Frame::json)
        .collect()
}

#[test]
fn dashboard_streams_the_active_tickets_provider_activity_live_with_summaries() {
    for provider in ["codex", "claude"] {
        let target = Target::new(provider);
        let mut child = target.spawn_start();
        let deadline = Instant::now() + Duration::from_secs(30);
        let run_dir = loop {
            let found = target.run_dirs().into_iter().find(|dir| {
                read(&dir.join(format!("agents/{TICKET_FILE}-implementation.log")))
                    .contains(FIRST_LINE)
            });
            if let Some(dir) = found {
                break dir;
            }
            assert!(Instant::now() < deadline, "{provider}: no provider line");
            std::thread::sleep(Duration::from_millis(50));
        };
        let id = run_dir.file_name().unwrap().to_str().unwrap().to_owned();
        let dashboard = Dashboard::start(&target);

        let mut stream = dashboard.events(&id, None);
        assert_eq!(stream.status, 200, "{}", stream.head);
        let mut frames = stream.until(|f| f.event == "provider" && f.data.contains(FIRST_LINE));
        assert!(
            child.try_wait().unwrap().is_none(),
            "{provider}: the first provider line is streamed while the provider runs"
        );
        let first = provider_events(&frames).pop().unwrap();
        assert_eq!(first["ticket"], "github:example/project#1", "{first:#}");
        assert_eq!(first["stage"], "implementation", "{first:#}");
        frames.extend(stream.until(|f| f.event == "end"));
        let _ = child.wait();

        let events = provider_events(&frames);
        let implementation: Vec<&Value> = events
            .iter()
            .filter(|e| e["stage"] == "implementation")
            .collect();
        let summaries: Vec<&str> = implementation
            .iter()
            .filter_map(|e| e["summary"].as_str())
            .collect();
        for summary in [
            "ran: echo done > first.txt",
            "edited: first.txt",
            "message: Implemented ticket one",
        ] {
            assert!(
                summaries.contains(&summary),
                "{provider}: missing {summary:?} in {summaries:#?}"
            );
        }
        let raw: Vec<&str> = implementation
            .iter()
            .map(|e| e["line"].as_str().unwrap())
            .collect();
        assert!(raw.contains(&"this line is not json {"), "{raw:#?}");
        assert!(
            raw.contains(&"secret [REDACTED] and [REDACTED]"),
            "{provider}: raw lines are the redacted provider events: {raw:#?}"
        );
        assert!(
            events.iter().any(|e| e["stage"] == "review"),
            "{provider}: {events:#?}"
        );
        for frame in &frames {
            assert!(
                !frame.data.contains(SECRET) && !frame.data.contains("private-auth-value"),
                "{provider}: {frame:#?}"
            );
        }

        // A reconnecting client is not sent the provider lines it saw again.
        let seen = frames.last().unwrap().id.clone().unwrap();
        let again = dashboard
            .events(&id, Some(&seen))
            .until(|f| f.event == "end");
        assert!(provider_events(&again).is_empty(), "{again:#?}");
    }
}
