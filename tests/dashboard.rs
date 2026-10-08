//! `kiln dashboard`: the embedded dashboard page and its JSON endpoints,
//! verified over real HTTP against runs recorded through the CLI.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::Duration,
};

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    /// A repository with one approved spec and a project configuration.
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let repo = Self { _temp: temp, path };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        fs::write(
            repo.path.join("one.md"),
            "# One\n## Acceptance criteria\n- Works\n",
        )
        .unwrap();
        fs::write(repo.path.join(".gitignore"), ".kiln/\n*.json\n!kiln.json\n").unwrap();
        let config = json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}});
        fs::write(repo.path.join("kiln.json"), config.to_string()).unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }
    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln"));
        command.args(args).current_dir(&self.path);
        command
    }
    fn cli(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn write(&self, file: &str, value: Value) {
        fs::write(self.path.join(file), value.to_string()).unwrap();
    }
    /// A run planned with tickets `a` and `b` (b depends on a), not started.
    fn planned(&self) -> String {
        let prepared = self.ok(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
        let id = prepared["id"].as_str().unwrap().to_owned();
        let ticket = |name: &str, blockers: &[&str]| json!({"id":name,"title":format!("Ticket {name}"),"description":"Deliver","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":blockers});
        self.write(
            "plan.json",
            json!({"tickets":[ticket("a", &[]), ticket("b", &["a"])],"verification":{"outcome":"verified","findings":[]}}),
        );
        self.ok(&["plan", &id, "--fixture", "plan.json"]);
        id
    }
    /// A planned run whose ticket `a` is integrated and `b` is blocked in review.
    fn executed(&self) -> String {
        let id = self.planned();
        let approved = json!({"outcome":"approved","evidence":"Reviewed"});
        let rejected = json!({"outcome":"rejected","evidence":"Not acceptable"});
        self.write(
            "scenario.json",
            json!({"tickets":{
                "a":{"implementation":{"files":{"a.txt":"a\n"},"outcome":"completed"},"review":{"standards":approved,"spec":approved}},
                "b":{"implementation":{"files":{"b.txt":"b\n"},"outcome":"completed"},"review":{"standards":rejected,"spec":approved},"corrections":[]}
            }}),
        );
        let out = self.cli(&["run", &id, "--fixture", "scenario.json"]);
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("panicked"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        id
    }
    /// Start `kiln dashboard` with `args` and read its announced address.
    fn dashboard(&self, args: &[&str]) -> Server {
        let mut command = self.command(&["dashboard"]);
        command.args(args);
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
    head: String,
    body: String,
}
impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (field, value) = line.split_once(':')?;
            field.eq_ignore_ascii_case(name).then_some(value.trim())
        })
    }
    fn json(&self) -> Value {
        assert_eq!(self.status, 200, "{}", self.body);
        serde_json::from_str(&self.body).unwrap()
    }
}
impl Server {
    fn port(&self) -> u16 {
        self.address.rsplit(':').next().unwrap().parse().unwrap()
    }
    fn get_host(&self, path: &str, host: &str) -> Reply {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        Reply {
            status: head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            head: head.to_owned(),
            body: body.to_owned(),
        }
    }
    fn get(&self, path: &str) -> Reply {
        self.get_host(path, &format!("localhost:{}", self.port()))
    }
}

#[test]
fn dashboard_serves_its_page_and_own_script_without_a_front_end_build() {
    let repo = Repo::new();
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);

    let page = server.get("/");
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(page
        .header("Content-Type")
        .unwrap()
        .starts_with("text/html"));
    assert!(
        page.body.contains("<script src=\"/dashboard.js\""),
        "{}",
        page.body
    );
    let csp = page.header("Content-Security-Policy").unwrap();
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert!(csp.contains("connect-src 'self'"), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(!csp.contains("unsafe-eval"), "{csp}");

    let script = server.get("/dashboard.js");
    assert_eq!(script.status, 200);
    assert!(script
        .header("Content-Type")
        .unwrap()
        .starts_with("text/javascript"));
    assert!(
        script.body.contains("/api/dashboard/runs"),
        "{}",
        script.body
    );
}

#[test]
fn dashboard_moves_to_the_next_free_port_when_its_port_is_taken() {
    let repo = Repo::new();
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();

    let server = repo.dashboard(&["--bind", &format!("127.0.0.1:{port}")]);

    assert!(
        server.address.starts_with("127.0.0.1:"),
        "{}",
        server.address
    );
    assert!(server.port() > port, "{} after taken {port}", server.port());
    assert_eq!(server.get("/").status, 200);
}

#[test]
fn dashboard_defaults_to_loopback_port_3000_or_the_next_free_one() {
    let repo = Repo::new();

    let server = repo.dashboard(&[]);

    assert!(
        server.address.starts_with("127.0.0.1:"),
        "{}",
        server.address
    );
    assert!((3000..3100).contains(&server.port()), "{}", server.address);
}

#[test]
fn dashboard_stays_loopback_only_and_rejects_foreign_hosts() {
    let repo = Repo::new();
    let out = repo.cli(&["dashboard", "--bind", "0.0.0.0:0"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("loopback"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);
    for path in ["/", "/dashboard.js", "/api/dashboard/runs"] {
        let reply = server.get_host(path, "evil.example:80");
        assert_eq!(reply.status, 400, "{path}: {}", reply.body);
    }
}

#[test]
fn runs_list_endpoint_lists_every_run_newest_first_with_its_status() {
    let repo = Repo::new();
    let planned = repo.planned();
    let executed = repo.executed();
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);

    let runs = server.get("/api/dashboard/runs").json();

    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0]["id"], executed.as_str());
    assert_eq!(runs[1]["id"], planned.as_str());
    assert_eq!(runs[1]["status"], "planned");
    assert_eq!(runs[1]["tickets"], 2);
    assert_ne!(runs[0]["status"], "planned");
}

/// Ticket ids per stage, in pipeline order.
fn stages(board: &Value) -> Vec<(String, Vec<String>)> {
    board["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["stage"].as_str().unwrap().to_owned(),
                c["tickets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t["id"].as_str().unwrap().to_owned())
                    .collect(),
            )
        })
        .collect()
}
fn stage_of(board: &Value, ticket: &str) -> String {
    stages(board)
        .into_iter()
        .find(|(_, ids)| ids.iter().any(|id| id == ticket))
        .unwrap_or_else(|| panic!("{ticket} not on board {board:#}"))
        .0
}

#[test]
fn run_board_endpoint_places_tickets_by_stage_from_recorded_state() {
    let repo = Repo::new();
    let planned = repo.planned();
    let executed = repo.executed();
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);

    let board = server.get(&format!("/api/dashboard/runs/{planned}")).json();
    let names: Vec<String> = stages(&board).into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        names,
        [
            "planned",
            "implementing",
            "review",
            "integrating",
            "pr",
            "ci"
        ]
    );
    assert_eq!(board["id"], planned.as_str());
    assert_eq!(stage_of(&board, "a"), "planned");
    assert_eq!(stage_of(&board, "b"), "planned");
    let b = &board["stages"][0]["tickets"][1];
    assert_eq!(b["title"], "Ticket b");
    assert_eq!(b["blocked_by"], json!(["a"]));

    let board = server
        .get(&format!("/api/dashboard/runs/{executed}"))
        .json();
    assert_eq!(stage_of(&board, "a"), "integrating", "{board:#}");
    assert_eq!(stage_of(&board, "b"), "review", "{board:#}");

    assert_eq!(server.get("/api/dashboard/runs/missing-run").status, 404);
    assert_eq!(server.get("/api/dashboard/runs/..%2Fx").status, 404);
}

/// A run state file written the way earlier Kiln versions recorded it: a
/// backlog run whose only ticket has an open pull request and a CI result.
fn record_earlier_run(repo: &Repo, id: &str) {
    let dir = repo.path.join(".kiln/runs");
    fs::create_dir_all(&dir).unwrap();
    let ticket = "github:example/project#7";
    let run = json!({
        "schema_version": 1, "id": id, "repository": repo.path, "created_unix_ms": 1_700_000_000_000u64,
        "status": "completed",
        "config": {"build":["true"],"test":["true"],"startup":["true"],"acceptance_criteria":[],
                   "isolation":{"network":"none","runtime":"system","commands":[]}},
        "specs": [],
        "plan": {"requirements":[],"findings":[],"dependency_graph":{},"executable":true,
                 "generation_context":"g","verification_context":"v",
                 "verification":{"outcome":"verified","findings":[]},"tickets":[{"id":ticket,"title":"Issue 7","description":"",
                 "acceptance_criteria":[],"covers":[],"blocked_by":[]}]},
        "scheduler": {"status":"completed","implementation_concurrency":1,"active":[],"peak_active":1,
                      "tickets":[{"id":ticket,"blocked_by":[],"state":"integrated","waiting_on":[],"blocker":null,"session_id":null}]},
        "delivery_groups": [{"id":"group-1","tickets":[ticket],"dependency_edges":[],"prerequisites":[],
            "status":"verified","pull_request":{"number":12,"url":"https://github.com/example/project/pull/12","head":"kiln/x","base":"main"},
            "ci_attempts":[{"id":"ci-1","pull_request":12,"commit":"abc","observed_unix_ms":1,"status":"passed","checks":[]}],
            "repair_attempts":0}]
    });
    fs::write(dir.join(format!("{id}.json")), run.to_string()).unwrap();
}

#[test]
fn runs_recorded_before_the_dashboard_appear_with_a_kanban_from_their_state() {
    let repo = Repo::new();
    record_earlier_run(&repo, "run-earlier");
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);

    let runs = server.get("/api/dashboard/runs").json();
    assert_eq!(runs[0]["id"], "run-earlier", "{runs:#}");
    assert_eq!(runs[0]["status"], "completed");

    let board = server.get("/api/dashboard/runs/run-earlier").json();
    let ticket = "github:example/project#7";
    assert_eq!(stage_of(&board, ticket), "ci", "{board:#}");
    let card = &board["stages"][5]["tickets"][0];
    assert_eq!(card["pull_request"]["number"], 12);
    assert_eq!(
        card["pull_request"]["url"],
        "https://github.com/example/project/pull/12"
    );
    assert_eq!(card["ci"], "passed");
}

#[test]
fn status_shows_the_url_of_a_running_dashboard_only_while_it_runs() {
    let repo = Repo::new();
    repo.planned();
    let status = || repo.ok(&["status", "--json"]);
    assert_eq!(status()["dashboard_url"], Value::Null);

    let mut server = repo.dashboard(&["--bind", "127.0.0.1:0"]);
    let url = format!("http://{}", server.address);
    assert_eq!(status()["dashboard_url"], url.as_str());
    let text = repo.cli(&["status"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains(&url));

    // SIGTERM stops the server cleanly and removes its record.
    // SAFETY: a plain signal to our own child process.
    unsafe { libc::kill(server.child.id() as libc::pid_t, libc::SIGTERM) };
    let exit = server.child.wait().unwrap();
    assert!(exit.success(), "{exit:?}");
    assert_eq!(status()["dashboard_url"], Value::Null);
    assert!(!repo.path.join(".kiln/dashboard.json").exists());

    // A killed server cannot remove its record; it is not reported either.
    let mut killed = repo.dashboard(&["--bind", "127.0.0.1:0"]);
    killed.child.kill().unwrap();
    killed.child.wait().unwrap();
    assert_eq!(status()["dashboard_url"], Value::Null);
}

#[test]
fn dashboard_is_listed_in_help() {
    let repo = Repo::new();
    let help = String::from_utf8_lossy(&repo.cli(&["--help"]).stdout).into_owned();
    assert!(
        help.lines()
            .any(|l| l.trim_start().starts_with("dashboard")),
        "{help}"
    );
}

/// One Server-Sent Events frame.
#[derive(Debug, Clone)]
struct Frame {
    id: Option<String>,
    event: String,
    data: String,
}
impl Frame {
    fn json(&self) -> Value {
        serde_json::from_str(&self.data).unwrap_or_else(|e| panic!("{e}: {}", self.data))
    }
    fn is(&self, ticket: Option<&str>, stage: &str, status: &str) -> bool {
        if self.event != "journal" {
            return false;
        }
        let event = self.json();
        event["ticket"].as_str() == ticket && event["stage"] == stage && event["status"] == status
    }
}
/// The body of a chunked HTTP/1.1 response; ends at its last chunk.
struct Dechunk {
    inner: BufReader<TcpStream>,
    left: usize,
    done: bool,
}
impl Read for Dechunk {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.left == 0 {
            let mut size = String::new();
            self.inner.read_line(&mut size)?;
            self.left = usize::from_str_radix(size.trim(), 16)
                .unwrap_or_else(|_| panic!("chunk size line {size:?}"));
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let wanted = buf.len().min(self.left);
        let n = self.inner.read(&mut buf[..wanted])?;
        self.left -= n;
        if self.left == 0 {
            let mut crlf = String::new();
            self.inner.read_line(&mut crlf)?;
        }
        Ok(n)
    }
}
/// A live connection to an event stream.
struct Stream {
    status: u16,
    head: String,
    reader: BufReader<Dechunk>,
}
impl Stream {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (field, value) = line.split_once(':')?;
            field.eq_ignore_ascii_case(name).then_some(value.trim())
        })
    }
    /// The next frame carrying data; `None` once the server closes the stream.
    fn next(&mut self) -> Option<Frame> {
        let (mut id, mut event, mut data) = (None, "message".to_owned(), Vec::new());
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line).unwrap() == 0 {
                return None;
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !data.is_empty() {
                    return Some(Frame {
                        id,
                        event,
                        data: data.join("\n"),
                    });
                }
                continue;
            }
            match line.split_once(':') {
                Some(("", _)) => {} // comment, e.g. a keep-alive
                Some(("id", v)) => id = Some(v.trim_start().to_owned()),
                Some(("event", v)) => event = v.trim_start().to_owned(),
                Some(("data", v)) => data.push(v.strip_prefix(' ').unwrap_or(v).to_owned()),
                _ => {}
            }
        }
    }
    /// Frames up to and including the first matching one.
    fn until(&mut self, mut done: impl FnMut(&Frame) -> bool) -> Vec<Frame> {
        let mut frames = Vec::new();
        while let Some(frame) = self.next() {
            let stop = done(&frame);
            frames.push(frame);
            if stop {
                return frames;
            }
        }
        panic!("stream closed before the expected frame: {frames:#?}");
    }
}
impl Server {
    fn events(&self, id: &str, last_event_id: Option<&str>) -> Stream {
        self.events_host(id, last_event_id, &format!("localhost:{}", self.port()))
    }
    fn events_host(&self, id: &str, last_event_id: Option<&str>, host: &str) -> Stream {
        let stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let resume = last_event_id
            .map(|id| format!("Last-Event-ID: {id}\r\n"))
            .unwrap_or_default();
        write!(
            &stream,
            "GET /api/dashboard/runs/{id}/events HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\n{resume}\r\n"
        )
        .unwrap();
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            head.push_str(&line);
        }
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let chunked = head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked");
        Stream {
            status,
            head,
            reader: BufReader::new(Dechunk {
                inner: reader,
                left: 0,
                done: status != 200 || !chunked,
            }),
        }
    }
}

/// Wait until the run's journal mentions `needle`.
fn wait_for_journal(repo: &Repo, id: &str, needle: &str) {
    let journal = repo.path.join(".kiln/runs").join(id).join("events.jsonl");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if fs::read_to_string(&journal).is_ok_and(|j| j.contains(needle)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("journal of {id} never mentioned {needle}");
}

#[test]
fn event_stream_replays_missed_journal_events_then_streams_live_ones_until_the_run_ends() {
    let repo = Repo::new();
    let id = repo.planned();
    let approved = json!({"outcome":"approved","evidence":"Reviewed"});
    repo.write(
        "scenario.json",
        json!({"tickets":{
            "a":{"implementation":{"files":{"a.txt":"a\n"},"outcome":"completed"},"review":{"standards":approved,"spec":approved},"await_file":".kiln/release"},
            "b":{"implementation":{"files":{"b.txt":"b\n"},"outcome":"completed"},"review":{"standards":approved,"spec":approved}}
        }}),
    );
    let mut run = repo
        .command(&["run", &id, "--fixture", "scenario.json"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_journal(&repo, &id, "\"implementation\"");
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);

    // Connecting mid-run first replays what was already journaled.
    let mut stream = server.events(&id, None);
    assert_eq!(stream.status, 200, "{}", stream.head);
    assert!(stream
        .header("Content-Type")
        .unwrap()
        .starts_with("text/event-stream"));
    assert_eq!(stream.header("Cache-Control"), Some("no-store"));
    let replayed = stream.until(|f| f.is(Some("a"), "implementation", "started"));
    assert!(
        replayed[0].is(None, "run", "running"),
        "replay starts at the beginning: {replayed:#?}"
    );
    assert!(replayed
        .iter()
        .all(|f| f.event == "journal" && f.id.is_some()));

    // Then events appended after the client connected arrive live.
    fs::write(repo.path.join(".kiln/release"), "").unwrap();
    let live = stream.until(|f| f.event == "end");
    assert!(
        live.iter().any(|f| f.is(Some("a"), "review", "passed")),
        "{live:#?}"
    );
    assert!(
        live.iter()
            .any(|f| f.is(Some("b"), "integration", "integrated")),
        "{live:#?}"
    );
    assert!(
        live[live.len() - 2].is(None, "run", "awaiting_validation"),
        "{live:#?}"
    );
    assert_eq!(live[live.len() - 1].json()["status"], "awaiting_validation");
    assert!(stream.next().is_none(), "the stream closes after its end");
    assert!(run.wait().unwrap().success());

    // A reconnecting client resumes after the last event it saw.
    let seen = replayed.last().unwrap().id.clone().unwrap();
    let journal = |frames: &[Frame]| -> Vec<String> {
        frames
            .iter()
            .filter(|f| f.event == "journal")
            .map(|f| f.data.clone())
            .collect()
    };
    let rest = server.events(&id, Some(&seen)).until(|f| f.event == "end");
    assert_eq!(journal(&rest), journal(&live));
}

#[test]
fn event_stream_of_a_run_without_a_journal_ends_at_once() {
    let repo = Repo::new();
    record_earlier_run(&repo, "run-earlier");
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);

    let mut stream = server.events("run-earlier", None);
    assert_eq!(stream.status, 200, "{}", stream.head);
    let frames = stream.until(|f| f.event == "end");
    assert_eq!(frames.len(), 1, "{frames:#?}");
    assert_eq!(frames[0].json()["status"], "completed");
    assert!(stream.next().is_none());

    assert_eq!(
        server.events("missing-run", None).status,
        404,
        "unknown runs have no stream"
    );
    assert_eq!(server.events("..%2Fx", None).status, 404);
    assert_eq!(
        server
            .events_host("run-earlier", None, "evil.example:80")
            .status,
        400
    );
}

#[test]
fn dashboard_script_follows_the_run_event_stream_and_shows_attention_items() {
    let repo = Repo::new();
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);
    let script = server.get("/dashboard.js").body;
    assert!(script.contains("new EventSource("), "{script}");
    assert!(script.contains("/events"), "{script}");
    assert!(script.contains("\"journal\""), "{script}");
    assert!(script.contains("\"end\""), "{script}");
    assert!(script.contains(".attention"), "{script}");
    let page = server.get("/").body;
    assert!(page.contains("id=\"activity\""), "{page}");
}

#[test]
fn board_lists_failures_divergences_and_autonomous_decisions_as_attention_items() {
    let repo = Repo::new();
    let planned = repo.planned();
    let executed = repo.executed();
    record_earlier_run(&repo, "run-earlier");
    let state = repo.path.join(".kiln/runs/run-earlier.json");
    let mut earlier: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    let ticket = json!({"id":"t","title":"T","description":"","acceptance_criteria":[],"covers":[],"blocked_by":[]});
    let position = json!({"rank":1,"source":"spec","reference":"one.md#ac-1","statement":"Use JSON","source_statement":"Use JSON"});
    earlier["decisions"] = json!([{"id":"decision-1","ambiguity_id":"amb-1","context_id":"c","question":"Which format?",
        "positions":[position],"governing":position,"resolution":"Use JSON","rationale":"The spec says so","evidence":[],
        "outcome":"resolved","findings":[]}]);
    earlier["replans"] = json!([{"id":"replan-1","ticket_id":"github:example/project#7","attempt":1,"context_id":"c",
        "failures":["tests failed"],"previous":ticket,"revised":ticket,"verification":null,"findings":[],"outcome":"replanned"}]);
    earlier["delivery_groups"][0]["status"] = json!("ci-failed");
    fs::write(&state, earlier.to_string()).unwrap();
    let server = repo.dashboard(&["--bind", "127.0.0.1:0"]);
    let attention = |id: &str| -> Vec<Value> {
        let board = server.get(&format!("/api/dashboard/runs/{id}")).json();
        board["attention"]
            .as_array()
            .unwrap_or_else(|| panic!("no attention list: {board:#}"))
            .clone()
    };

    assert_eq!(attention(&planned), Vec::<Value>::new());

    let items = attention(&executed);
    let failure = items
        .iter()
        .find(|i| i["kind"] == "failure" && i["ticket"] == "b")
        .unwrap_or_else(|| panic!("blocked ticket b needs attention: {items:#?}"));
    assert!(!failure["message"].as_str().unwrap().is_empty());

    let items = attention("run-earlier");
    let find = |kind: &str| {
        items
            .iter()
            .find(|i| i["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind}: {items:#?}"))
    };
    assert!(find("decision")["message"]
        .as_str()
        .unwrap()
        .contains("Use JSON"));
    assert_eq!(find("divergence")["ticket"], "github:example/project#7");
    assert!(find("failure")["message"].as_str().unwrap().contains("CI"));
}
