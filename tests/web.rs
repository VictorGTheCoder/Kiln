//! Local monitoring interface, verified at the HTTP boundary of `kiln serve`
//! against the durable engine state that `kiln inspect` exposes. Workflows are
//! driven through the CLI with deterministic agent fixtures in temporary repositories.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

const FAULT_EXIT: i32 = 86;

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    /// Two approved specs; `extra` keys are merged into the project configuration.
    fn new(extra: Value) -> Self {
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
        fs::write(
            repo.path.join("two.md"),
            "# Two\n## Acceptance criteria\n- Also works\n",
        )
        .unwrap();
        fs::write(repo.path.join(".gitignore"), ".kiln/\n*.json\n!kiln.json\n").unwrap();
        let mut config = json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}});
        for (k, v) in extra.as_object().unwrap() {
            config[k] = v.clone();
        }
        fs::write(repo.path.join("kiln.json"), config.to_string()).unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiln"));
        command
            .args(args)
            .current_dir(&self.path)
            .env_remove("KILN_FAULT_INJECT");
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
    fn planned(&self, tickets: &[(&str, &str, &[&str])]) -> String {
        let prepared = self.ok(&[
            "prepare",
            "--config",
            "kiln.json",
            "--spec",
            "one.md",
            "--spec",
            "two.md",
        ]);
        let id = prepared["id"].as_str().unwrap().to_owned();
        let plan: Vec<Value> = tickets
            .iter()
            .map(|(name, spec, blockers)| json!({"id":name,"title":format!("Ticket {name}"),"description":"Deliver","acceptance_criteria":["works"],"covers":covers(spec),"blocked_by":blockers}))
            .collect();
        self.write(
            "plan.json",
            json!({"tickets":plan,"verification":{"outcome":"verified","findings":[]}}),
        );
        self.ok(&["plan", &id, "--fixture", "plan.json"]);
        id
    }
    fn inspect(&self, id: &str) -> Value {
        self.ok(&["inspect", id])
    }
    fn replan(&self, id: &str, spec: &str, fixture: Value) -> Value {
        self.write("replan.json", fixture);
        self.ok(&["replan", id, "--spec", spec, "--fixture", "replan.json"])
    }
    /// Start `kiln serve` on an ephemeral loopback port.
    fn serve(&self, env: &[(&str, &str)]) -> Server {
        let mut command = self.command(&["serve", "--bind", "127.0.0.1:0"]);
        for (k, v) in env {
            command.env(k, v);
        }
        let mut child = command.stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line
            .trim()
            .strip_prefix("Kiln web view: http://")
            .expect("web server announces its loopback address")
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
impl Server {
    /// (status code, body)
    fn get(&self, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
            self.address.rsplit(':').next().unwrap()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, body.to_owned())
    }
    fn get_host(&self, path: &str, host: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            body.to_owned(),
        )
    }
    fn page(&self, id: &str) -> String {
        let (status, body) = self.get(&format!("/runs/{id}"));
        assert_eq!(status, 200, "{body}");
        body
    }
}
/// `one.md` covers its first criterion; `one.md#ac-1,two.md#ac-2` lists criteria.
fn covers(spec: &str) -> Vec<String> {
    if spec.contains('#') {
        spec.split(',').map(str::to_owned).collect()
    } else {
        vec![format!("{spec}#ac-1")]
    }
}
fn approved() -> Value {
    json!({"outcome":"approved","findings":[],"evidence":"Observed concrete change"})
}
fn works(file: &str) -> Value {
    json!({"implementation":{"files":{file:"works"},"outcome":"completed"},"review":{"standards":approved(),"spec":approved()}})
}
fn ticket<'a>(run: &'a Value, id: &str) -> &'a Value {
    run["scheduler"]["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}
/// The HTML section of a page that carries `data-ticket="<id>"`, up to the row end.
fn ticket_row<'a>(page: &'a str, id: &str) -> &'a str {
    let start = page
        .find(&format!("data-ticket=\"{id}\""))
        .unwrap_or_else(|| panic!("no row for ticket {id} in {page}"));
    let rest = &page[start..];
    &rest[..rest.find("</tr>").unwrap()]
}
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn running_ticket_dependencies_and_blockers_follow_engine_state() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[
        ("a", "one.md", &[]),
        ("c", "one.md", &["a"]),
        ("x", "two.md", &[]),
        ("y", "two.md", &["x"]),
    ]);
    let mut a = works("a.txt");
    a["await_file"] = json!(".kiln/release");
    let failing = json!({"implementation":{"files":{},"outcome":"failed"},"review":{"standards":approved(),"spec":approved()}});
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":a,"c":works("c.txt"),"x":failing,"y":works("y.txt")}}),
    );
    let mut child = repo
        .command(&["run", &id, "--fixture", "scenario.json"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut state = Value::Null;
    wait_until("ticket a implementing while x is blocked", || {
        state = repo.inspect(&id);
        state["scheduler"]["tickets"].is_array()
            && ticket(&state, "a")["state"] == "implementing"
            && ticket(&state, "x")["state"] == "blocked"
    });
    let server = repo.serve(&[]);
    let page = server.page(&id);

    // Frozen spec version.
    for spec in state["specs"].as_array().unwrap() {
        assert!(page.contains(spec["path"].as_str().unwrap()));
        assert!(page.contains(spec["content_sha256"].as_str().unwrap()));
    }
    // Active session of the running ticket.
    assert_eq!(state["scheduler"]["active"], json!(["a"]));
    assert!(ticket_row(&page, "a").contains("implementing"));
    assert!(page.contains("Active sessions"));
    assert!(page.contains("data-active=\"a\""));
    assert!(page.contains("Live: a scheduler process owns this run"));
    // Dependencies and why waiting or blocked work cannot start.
    assert!(ticket_row(&page, "c").contains("Waiting on: a"));
    assert!(ticket_row(&page, "x").contains("blocked"));
    assert!(ticket_row(&page, "x").contains("agent outcome: failed"));
    assert!(ticket_row(&page, "y").contains("prerequisite x is blocked"));
    assert!(ticket_row(&page, "y").contains("Depends on: x"));
    // A running run keeps refreshing itself.
    assert!(page.contains("http-equiv=\"refresh\""));

    fs::write(repo.path.join(".kiln/release"), "go").unwrap();
    assert!(!child.wait().unwrap().success(), "x is blocked");
    let finished = repo.inspect(&id);
    assert_eq!(finished["status"], "blocked");
    let page = server.page(&id);
    assert!(ticket_row(&page, "a").contains("integrated"));
    assert!(ticket_row(&page, "c").contains("integrated"));
    assert!(!page.contains("http-equiv=\"refresh\""));
    assert!(page.contains("No active sessions"));
    assert!(page.contains("Not live: no scheduler process owns this run"));
}

#[test]
fn failed_review_correction_integration_and_limits_are_inspectable() {
    let repo = Repo::new(json!({"usage_token_limit": 100000}));
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &[])]);
    let hostile = "<script>alert('agent')</script>";
    let rejected = json!({"outcome":"rejected","findings":[{"code":"spec-gap","message":format!("Missing behaviour {hostile}"),"evidence":"a.txt is empty","required":true}],"evidence":"Observed change"});
    let standards = json!({"outcome":"rejected","findings":[{"code":"style","message":"Trailing whitespace","evidence":"line 1","required":true}],"evidence":"Observed change"});
    let log = json!({"usage":{"input_tokens":1234,"output_tokens":0},"cost":null}).to_string();
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":{
        "implementation":{"files":{"a.txt":""},"outcome":"completed","log":log},
        "review":{"standards":standards,"spec":rejected},
        "corrections":{"corrections":[{"files":{"a.txt":"works"},"outcome":"completed"}],
                       "reviews":[{"standards":approved(),"spec":approved()}]}},
        "b":works("b.txt")}}),
    );
    let out = repo.cli(&["run", &id, "--fixture", "scenario.json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state = repo.inspect(&id);
    assert_eq!(ticket(&state, "a")["state"], "integrated");
    let server = repo.serve(&[]);
    let page = server.page(&id);

    // Standards and Spec findings of the failed review, escaped as untrusted text.
    assert!(page.contains("Standards review"));
    assert!(page.contains("Spec review"));
    assert!(page.contains("Trailing whitespace"));
    assert!(page.contains("Missing behaviour &lt;script&gt;alert(&#39;agent&#39;)&lt;/script&gt;"));
    assert!(!page.contains(hostile));
    assert!(page.contains("state-rejected"));
    // Correction history and the integration outcome.
    let correction = &state["corrections"][0];
    assert!(page.contains("Correction history"));
    assert!(page.contains(correction["id"].as_str().unwrap()));
    assert!(page.contains(&format!(
        "state-{}",
        correction["outcome"].as_str().unwrap()
    )));
    let integration = &state["integrations"].as_array().unwrap().last().unwrap();
    assert!(page.contains("Integration outcomes"));
    assert!(page.contains(integration["integrated_commit"].as_str().unwrap()));
    // Limits and usage are the engine's recorded values; unavailable cost is never zero.
    let limits = &state["scheduler"]["limits"];
    assert_eq!(limits["usage"]["tokens"], 1234);
    assert_eq!(limits["usage"]["cost_data"], "unavailable");
    let section = &page[page.find("Limits and usage").unwrap()..];
    assert!(section.contains("100000"));
    assert!(section.contains("1234"));
    assert!(section.contains("Cost: unavailable"));
    assert!(!section.contains("$0"));
    assert!(section.contains(limits["cost_ceiling"].as_str().unwrap()));
    assert!(section.contains("No run-wide limit was exhausted"));
}

#[test]
fn interrupted_run_and_its_recovery_decisions_are_shown() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &["a"])]);
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":works("a.txt"),"b":works("b.txt")}}),
    );
    let out = repo
        .command(&["run", &id, "--fixture", "scenario.json"])
        .env("KILN_FAULT_INJECT", "implementation.after_commit@a")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(FAULT_EXIT),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state = repo.inspect(&id);
    assert_eq!(
        state["status"], "running",
        "the interrupted process recorded no final status"
    );
    let server = repo.serve(&[]);
    let page = server.page(&id);
    assert!(page
        .contains("Interrupted: the run is recorded as running, but no scheduler process owns it"));
    assert!(page.contains(&format!("kiln resume {id}")));
    assert!(page.contains("No recovery has been recorded"));

    let out = repo.cli(&["resume", &id, "--fixture", "scenario.json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let resumed = repo.inspect(&id);
    let page = server.page(&id);
    assert!(!page.contains("Interrupted: the run is recorded"));
    assert!(page.contains("Recovery"));
    let recovery = &resumed["recoveries"][0];
    assert!(page.contains(recovery["id"].as_str().unwrap()));
    for decision in recovery["decisions"].as_array().unwrap() {
        assert!(page.contains(&format!("state-{}", decision["action"].as_str().unwrap())));
        let reason = decision["reason"]
            .as_str()
            .unwrap()
            .replace('\'', "&#39;")
            .replace('"', "&quot;");
        assert!(page.contains(&reason), "missing reason {reason}");
    }
    assert!(ticket_row(&page, "b").contains("integrated"));
}

#[test]
fn run_wide_limit_stop_reason_is_shown() {
    let repo = Repo::new(json!({"usage_token_limit": 10}));
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &["a"])]);
    let mut a = works("a.txt");
    a["implementation"]["log"] =
        json!(json!({"usage":{"input_tokens":50,"output_tokens":0},"cost":0.25}).to_string());
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":a,"b":works("b.txt")}}),
    );
    assert!(!repo
        .cli(&["run", &id, "--fixture", "scenario.json"])
        .status
        .success());
    let state = repo.inspect(&id);
    let exhausted = &state["scheduler"]["limits"]["exhausted"];
    assert_eq!(exhausted["limit"], "usage_tokens");
    let page = repo.serve(&[]).page(&id);
    assert!(page.contains("state-limit_exhausted"));
    let section = &page[page.find("Limits and usage").unwrap()..];
    assert!(section.contains("Stop reason: usage_tokens limit exhausted"));
    let reason = exhausted["reason"]
        .as_str()
        .unwrap()
        .replace('\'', "&#39;")
        .replace('"', "&quot;");
    assert!(section.contains(&reason));
    // Reported cost is shown as the measured value.
    assert_eq!(
        state["scheduler"]["limits"]["usage"]["measured_cost_usd"],
        0.25
    );
    assert!(section.contains("$0.2500 measured"));
}

#[test]
fn provider_usage_limit_is_shown_distinctly_from_ticket_failures() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &["a"])]);
    let mut a = works("a.txt");
    a["implementation"]["provider_failure"] =
        json!("You've hit your usage limit. Upgrade to Pro or try again at 10:05 AM.");
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":a,"b":works("b.txt")}}),
    );
    assert!(!repo
        .cli(&["run", &id, "--fixture", "scenario.json"])
        .status
        .success());
    let page = repo.serve(&[]).page(&id);
    let section = &page[page.find("Limits and usage").unwrap()..];
    assert!(section.contains("data-provider-limit"), "{section}");
    assert!(section.contains("Stop reason: provider_usage limit exhausted"));
    assert!(section.contains("Provider fixture"));
    assert!(section.contains("resets: 10:05 AM"));
    let row = ticket_row(&page, "a");
    assert!(row.contains("stopped"), "{row}");
    assert!(!row.contains("Blocker"), "{row}");
}

#[test]
fn global_validation_outcomes_and_evidence_per_criterion_are_distinguishable() {
    const FLOW: [&str; 3] = [
        "sh",
        "-c",
        "test -f .ready && test \"$(cat one.txt)\" = \"$(cat two.txt)\"",
    ];
    const STARTUP: [&str; 3] = ["sh", "-c", "touch .ready; exec sleep 30"];
    const PROBE: [&str; 3] = ["sh", "-c", "test -f .ready"];
    const PRESENT: [&str; 3] = ["sh", "-c", "test -f two.txt"];
    let repo = Repo::new(json!({
        "build": PRESENT, "test": PRESENT, "startup": STARTUP,
        "isolation": {"runtime":"system","network":"none","commands":[PRESENT, STARTUP, PROBE, FLOW]},
        "validation": {"startup_probe": PROBE, "timeout_ms": 3000, "workflows": [
            {"criterion":"one.md#ac-1","command":FLOW},
            {"criterion":"two.md#ac-1","command":PRESENT}
        ]}
    }));
    fs::write(
        repo.path.join("two.md"),
        "# Two\n## Acceptance criteria\n- Also works\n- Keeps <b>markup</b> inert\n",
    )
    .unwrap();
    fs::write(repo.path.join("one.txt"), "v1\n").unwrap();
    fs::write(repo.path.join("two.txt"), "v1\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "fixtures"]);
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md#ac-1,two.md#ac-2", &[])]);
    let a = json!({"implementation":{"files":{"one.txt":"v2\n"},"outcome":"completed"},"review":{"standards":approved(),"spec":approved()}});
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":a,"b":works("b.txt")}}),
    );
    repo.ok(&["run", &id, "--fixture", "scenario.json"]);
    assert!(!repo.cli(&["validate", &id]).status.success());
    let state = repo.inspect(&id);
    let report = state["validation_reports"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(report["outcome"], "failed");
    let outcome = |c: &str| {
        report["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == c)
            .unwrap()["outcome"]
            .clone()
    };
    assert_eq!(outcome("one.md#ac-1"), "failed");
    assert_eq!(outcome("two.md#ac-1"), "verified");
    assert_eq!(outcome("two.md#ac-2"), "unable-to-verify");

    let page = repo.serve(&[]).page(&id);
    let section = &page[page.find("Global validation").unwrap()..];
    assert!(section.contains(report["id"].as_str().unwrap()));
    assert!(section.contains(report["integrated_commit"].as_str().unwrap()));
    let row = |c: &str| {
        let start = section
            .find(&format!("data-criterion=\"{c}\""))
            .unwrap_or_else(|| panic!("no {c}"));
        &section[start..start + section[start..].find("</tr>").unwrap()]
    };
    // Each outcome has its own visual class and its own text.
    assert!(row("one.md#ac-1").contains("class=\"badge bad state-failed\">Failed<"));
    assert!(row("two.md#ac-1").contains("class=\"badge good state-verified\">Verified<"));
    assert!(row("two.md#ac-2")
        .contains("class=\"badge unknown state-unable-to-verify\">Unable to verify<"));
    // Evidence per criterion: the configured workflow command and its result.
    assert!(row("one.md#ac-1").contains("configured"));
    assert!(row("one.md#ac-1").contains("test -f .ready"));
    assert!(row("two.md#ac-1").contains("test -f two.txt"));
    assert!(row("two.md#ac-2").contains("No evidence"));
    // Spec text is untrusted input and stays inert.
    assert!(row("two.md#ac-2").contains("Keeps &lt;b&gt;markup&lt;/b&gt; inert"));
}

#[test]
fn configured_secret_values_never_reach_the_interface() {
    const SECRET: &str = "kiln-web-secret-value-90210";
    let repo = Repo::new(
        json!({"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]],"secrets":{"KILN_WEB_TOKEN":["build"]}}}),
    );
    let out = repo
        .command(&["prepare", "--config", "kiln.json", "--spec", "one.md"])
        .env("KILN_WEB_TOKEN", SECRET)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = serde_json::from_slice::<Value>(&out.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // State recorded before the value was registered may still carry it.
    let file = repo.path.join(format!(".kiln/runs/{id}.json"));
    let mut state: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    state["specs"][0]["path"] = json!(format!("leaked-{SECRET}.md"));
    fs::write(&file, state.to_string()).unwrap();

    let server = repo.serve(&[("KILN_WEB_TOKEN", SECRET)]);
    for path in [
        format!("/runs/{id}"),
        format!("/api/runs/{id}"),
        "/".to_owned(),
    ] {
        let (status, body) = server.get(&path);
        assert_eq!(status, 200, "{path}: {body}");
        assert!(!body.contains(SECRET), "{path} exposes the secret");
    }
    let (status, body) = server.get_host(&format!("/api/runs/{id}"), "evil.example:80");
    assert_eq!(status, 400);
    assert_eq!(body, "Invalid Host header");
    let page = server.page(&id);
    assert!(page.contains("leaked-[REDACTED].md"));
    assert!(page.contains("<html lang=\"en\">"));
}

#[test]
fn escaped_html_secret_is_redacted_from_run_page_and_json() {
    const SECRET: &str = "token&<value>\"'";
    let repo = Repo::new(
        json!({"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]],"secrets":{"KILN_WEB_TOKEN":["build"]}}}),
    );
    let out = repo
        .command(&["prepare", "--config", "kiln.json", "--spec", "one.md"])
        .env("KILN_WEB_TOKEN", SECRET)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = serde_json::from_slice::<Value>(&out.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let file = repo.path.join(format!(".kiln/runs/{id}.json"));
    let mut state: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    state["specs"][0]["path"] = json!(format!("leaked-{SECRET}.md"));
    fs::write(&file, state.to_string()).unwrap();
    let server = repo.serve(&[("KILN_WEB_TOKEN", SECRET)]);
    let page = server.page(&id);
    assert!(!page.contains(SECRET));
    assert!(!page.contains("token&amp;&lt;value&gt;&quot;&#39;"));
    assert!(page.contains("leaked-[REDACTED].md"));
    let (status, json) = server.get(&format!("/api/runs/{id}"));
    assert_eq!(status, 200);
    assert!(!json.contains(SECRET));
    assert!(json.contains("[REDACTED]"));
}

#[test]
fn replanning_decisions_and_spec_revisions_explain_the_effective_work() {
    let repo = Repo::new(
        json!({"product_objectives":[{"id":"PO-1","statement":"Reports open in spreadsheets"}]}),
    );
    let id = repo.planned(&[("a", "one.md", &[]), ("c", "two.md", &["a"])]);
    // A decision revises one.md; the frozen version stays recorded beside it.
    let revised = "# One\n## Acceptance criteria\n- Works as CSV\n";
    repo.write("ambiguity.json", json!({"id":"format","question":"Which <format>?","positions":[
        {"source":"spec","reference":"one.md","statement":"Format is open"},
        {"source":"product_objective","reference":"PO-1","statement":"Spreadsheet users need CSV"}]}));
    repo.write("decision.json", json!({"proposal":{"governing":"PO-1","resolution":"Export CSV","rationale":"Objectives outrank specs",
        "evidence":["PO-1 requires spreadsheets"],"spec_revision":{"path":"one.md","content":revised}},
        "verification":{"outcome":"verified","findings":[]}}));
    repo.ok(&[
        "decide",
        &id,
        "--ambiguity",
        "ambiguity.json",
        "--fixture",
        "decision.json",
    ]);
    // Ticket a exhausts its corrections, is replanned, and still fails.
    let rejected = json!({"outcome":"rejected","findings":[{"code":"wrong","message":"Still incorrect","evidence":"x","required":true}],"evidence":"Observed change"});
    let corrections: Vec<Value> = (0..3)
        .map(|n| json!({"files":{"bad.txt":format!("attempt {n}")},"outcome":"completed"}))
        .collect();
    let reviews: Vec<Value> = (0..3)
        .map(|_| json!({"standards":approved(),"spec":rejected}))
        .collect();
    let a = json!({"implementation":{"files":{"bad.txt":"wrong"},"outcome":"completed"},
        "review":{"standards":approved(),"spec":rejected},
        "corrections":{"corrections":corrections,"reviews":reviews},
        "replanning":{"ticket":{"id":"a","title":"a","description":"Smaller approach","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":[]},
            "verification":{"outcome":"verified","findings":[]},
            "implementation":{"files":{"still.txt":"works"},"outcome":"completed"},
            "review":{"standards":approved(),"spec":rejected}}});
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":a,"c":works("c.txt")}}),
    );
    assert!(!repo
        .cli(&["run", &id, "--fixture", "scenario.json"])
        .status
        .success());
    let state = repo.inspect(&id);
    assert_eq!(ticket(&state, "a")["exhaustion"], "replanning");

    let page = repo.serve(&[]).page(&id);
    // Frozen spec plus the verified revision in effect.
    let revision = &state["spec_revisions"][0];
    let specs = &page[page.find("Frozen specs").unwrap()..page.find("Active sessions").unwrap()];
    assert!(specs.contains(state["specs"][0]["content_sha256"].as_str().unwrap()));
    assert!(specs.contains("Revision 1"));
    assert!(specs.contains(revision["content_sha256"].as_str().unwrap()));
    assert!(specs.contains("state-verified"));
    assert!(specs.contains("Works as CSV"));
    // Why a is blocked: the replanning attempt and its result.
    assert!(ticket_row(&page, "a").contains("Exhausted: replanning"));
    let replans = &page[page.find("Replanning").unwrap()..];
    let attempt = &state["replans"][0];
    assert!(replans.contains(attempt["id"].as_str().unwrap()));
    assert!(replans.contains(&format!("state-{}", attempt["outcome"].as_str().unwrap())));
    assert!(replans.contains("Smaller approach"));
    assert!(replans.contains("Result: <span class=\"badge bad state-blocked\""));
    // Decision history with escaped question text.
    let decisions = &page[page.find("Decisions").unwrap()..];
    assert!(decisions.contains("Which &lt;format&gt;?"));
    assert!(decisions.contains("Export CSV"));
    assert!(decisions.contains("Objectives outrank specs"));
    assert!(decisions.contains("PO-1"));
    // Superseded sessions are labelled as such in the session history.
    assert!(page.contains("state-superseded"));
}

#[test]
fn approved_spec_replans_and_work_input_versions_are_visible() {
    let repo = Repo::new(json!({}));
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &[])]);
    repo.write(
        "scenario.json",
        json!({"tickets":{"a":works("a.txt"),"b":works("b.txt")}}),
    );
    let out = repo.cli(&["run", &id, "--fixture", "scenario.json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    fs::write(
        repo.path.join("one.md"),
        "# One\n## Acceptance criteria\n- Works in French\n",
    )
    .unwrap();
    let replan = repo.replan(
        &id,
        "one.md",
        json!({
            "tickets":[{"id":"a","title":"Ticket a","description":"Work in French","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":[]}],
            "verification":{"outcome":"verified","findings":[]}
        }),
    );
    assert_eq!(replan["spec_replans"][0]["input_version"], 1);
    let first_session = replan["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["ticket_id"] == "a")
        .unwrap();
    assert_eq!(first_session["input_version"], 0);

    let page = repo.serve(&[]).page(&id);
    assert!(page.contains("Current input version: <strong>1</strong>"));
    assert!(page.contains("Ticket input version"));
    assert!(page.contains("Input version 0"));
    assert!(page.contains("Spec replan history"));
    assert!(page.contains(replan["spec_replans"][0]["id"].as_str().unwrap()));
    assert!(page.contains("Recorded by spec replan"));
    assert!(page.contains("Previous input version 0 → 1"));
    assert!(page.contains("<th>Affected tickets</th>"));
    assert!(page.contains("<td>a</td>"));
}
