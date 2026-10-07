use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    /// Two approved specs, deterministic checks, optional implementation concurrency cap.
    fn new(concurrency: Option<u64>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let repo = Self { _temp: temp, path };
        repo.git(&["init", "-q"]);
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
        fs::write(repo.path.join(".gitignore"), ".kiln/\n").unwrap();
        let mut config = json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}});
        if let Some(limit) = concurrency {
            config["implementation_concurrency"] = json!(limit);
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
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
    /// Prepares a run with an accepted plan. `tickets` are (id, spec, blocked_by);
    /// tests must cover both specs.
    fn planned(&self, tickets: &[(&str, &str, &[&str])]) -> String {
        let prepared = self.cli(&[
            "prepare",
            "--config",
            "kiln.json",
            "--spec",
            "one.md",
            "--spec",
            "two.md",
        ]);
        assert!(
            prepared.status.success(),
            "{}",
            String::from_utf8_lossy(&prepared.stderr)
        );
        let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
        let id = prepared["id"].as_str().unwrap().to_owned();
        let plan_tickets: Vec<Value> = tickets
            .iter()
            .map(|(name, spec, blockers)| json!({"id":name,"title":name,"description":"Deliver","acceptance_criteria":["works"],"covers":[format!("{spec}#ac-1")],"blocked_by":blockers}))
            .collect();
        fs::write(
            self.path.join("plan.json"),
            json!({"tickets":plan_tickets,"verification":{"outcome":"verified","findings":[]}})
                .to_string(),
        )
        .unwrap();
        let planned = self.cli(&["plan", &id, "--fixture", "plan.json"]);
        assert!(
            planned.status.success(),
            "{}",
            String::from_utf8_lossy(&planned.stderr)
        );
        id
    }
    fn scenario(&self, scenario: Value) {
        fs::write(self.path.join("scenario.json"), scenario.to_string()).unwrap();
    }
    fn run(&self, id: &str) -> (Output, Value) {
        let out = self.cli(&["run", id, "--fixture", "scenario.json"]);
        let run = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("no run state: {}", String::from_utf8_lossy(&out.stderr)));
        (out, run)
    }
}
fn approved() -> Value {
    json!({"outcome":"approved","findings":[],"evidence":"Observed concrete change"})
}
/// A deterministic ticket that writes `file` and passes both review axes.
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
fn sessions_for<'a>(run: &'a Value, id: &str) -> Vec<&'a Value> {
    run["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["ticket_id"] == id)
        .collect()
}
fn integrated_commit(run: &Value, id: &str) -> String {
    run["integrations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["ticket_id"] == id && i["status"] == "integrated")
        .unwrap()["integrated_commit"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn assert_ancestor(repo: &Path, ancestor: &str, descendant: &str) {
    let ok = Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(
        ok.success(),
        "{ancestor} is not an ancestor of {descendant}"
    );
}

#[test]
fn cross_spec_prerequisite_is_integrated_and_verified_before_dependent_starts() {
    let repo = Repo::new(None);
    let id = repo.planned(&[("a", "one.md", &[]), ("b", "two.md", &["a"])]);
    // b can only succeed when a's integrated change is already present in its worktree.
    let mut b = works("b.txt");
    b["require_files"] = json!(["a.txt"]);
    repo.scenario(json!({"tickets":{"a":works("a.txt"),"b":b}}));
    let (out, run) = repo.run(&id);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run["status"], "awaiting_validation");
    assert_eq!(run["scheduler"]["implementation_concurrency"], 3);
    for t in ["a", "b"] {
        assert_eq!(ticket(&run, t)["state"], "integrated");
        assert_eq!(sessions_for(&run, t).len(), 1);
        assert_eq!(sessions_for(&run, t)[0]["status"], "integrated");
    }
    let b_base = sessions_for(&run, "b")[0]["base_commit"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ancestor(&repo.path, &integrated_commit(&run, "a"), &b_base);
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    assert_eq!(tip, integrated_commit(&run, "b"));
    assert_eq!(repo.git(&["show", &format!("{tip}:a.txt")]), "works");
}

#[test]
fn independent_sessions_overlap_up_to_the_default_cap_of_three() {
    let repo = Repo::new(None);
    let id = repo.planned(&[
        ("a", "one.md", &[]),
        ("b", "two.md", &[]),
        ("c", "one.md", &[]),
        ("d", "two.md", &[]),
    ]);
    // a, b and c each wait until all three are running at once: a serial or
    // two-wide scheduler times out. A fourth concurrent session fails its agent.
    let mut tickets = serde_json::Map::new();
    for t in ["a", "b", "c"] {
        let mut v = works(&format!("{t}.txt"));
        v["await_started"] = json!(["a", "b", "c"]);
        tickets.insert(t.into(), v);
    }
    tickets.insert("d".into(), works("d.txt"));
    repo.scenario(json!({"max_active_implementations":3,"tickets":tickets}));
    let (out, run) = repo.run(&id);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run["scheduler"]["implementation_concurrency"], 3);
    assert_eq!(run["scheduler"]["peak_active"], 3);
    let worktrees: std::collections::HashSet<_> = run["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["worktree"].as_str().unwrap())
        .collect();
    let contexts: std::collections::HashSet<_> = run["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["context_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        (worktrees.len(), contexts.len()),
        (4, 4),
        "separate worktrees and fresh contexts"
    );
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    for t in ["a", "b", "c", "d"] {
        assert_eq!(ticket(&run, t)["state"], "integrated");
        assert_eq!(repo.git(&["show", &format!("{tip}:{t}.txt")]), "works");
    }
}

#[test]
fn configured_cap_limits_concurrent_implementation_sessions() {
    let repo = Repo::new(Some(2));
    let id = repo.planned(&[
        ("a", "one.md", &[]),
        ("b", "two.md", &[]),
        ("c", "one.md", &[]),
    ]);
    let mut a = works("a.txt");
    a["await_started"] = json!(["b"]);
    let mut b = works("b.txt");
    b["await_started"] = json!(["a"]);
    repo.scenario(
        json!({"max_active_implementations":2,"tickets":{"a":a,"b":b,"c":works("c.txt")}}),
    );
    let (out, run) = repo.run(&id);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run["scheduler"]["implementation_concurrency"], 2);
    assert_eq!(run["scheduler"]["peak_active"], 2);
    assert!(run["scheduler"]["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["state"] == "integrated"));
}

#[test]
fn invalid_concurrency_cap_is_rejected_before_launch() {
    let repo = Repo::new(Some(0));
    let out = repo.cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("implementation_concurrency"));
}

#[test]
fn blocked_ticket_stops_descendants_while_independent_work_continues() {
    let repo = Repo::new(None);
    let id = repo.planned(&[
        ("a", "one.md", &[]),
        ("b", "two.md", &["a"]),
        ("c", "one.md", &["b"]),
        ("d", "two.md", &[]),
    ]);
    let failing = json!({"implementation":{"files":{"a.txt":"broken"},"outcome":"failed"},"review":{"standards":approved(),"spec":approved()}});
    // d only succeeds if it overlaps a's failing session, so a's failure cannot stop it.
    let mut d = works("d.txt");
    d["await_started"] = json!(["a"]);
    repo.scenario(json!({"tickets":{"a":failing,"b":works("b.txt"),"c":works("c.txt"),"d":d}}));
    let (out, run) = repo.run(&id);
    assert!(
        !out.status.success(),
        "a blocked run must not report success"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("blocked"));
    assert_eq!(run["status"], "blocked");
    assert_eq!(ticket(&run, "a")["state"], "blocked");
    assert!(ticket(&run, "a")["blocker"]
        .as_str()
        .unwrap()
        .contains("agent outcome: failed"));
    for descendant in ["b", "c"] {
        assert_eq!(ticket(&run, descendant)["state"], "waiting");
        assert!(ticket(&run, descendant)["blocker"]
            .as_str()
            .unwrap()
            .contains("prerequisite a is blocked"));
        assert!(
            sessions_for(&run, descendant).is_empty(),
            "{descendant} must never start"
        );
    }
    assert_eq!(ticket(&run, "b")["waiting_on"], json!(["a"]));
    assert_eq!(ticket(&run, "d")["state"], "integrated");
    let tip = repo.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    assert_eq!(tip, integrated_commit(&run, "d"));
    assert!(repo.cli(&["inspect", &id]).status.success());
}

#[test]
fn active_sessions_waiting_prerequisites_and_blockers_are_visible_while_running() {
    let repo = Repo::new(None);
    let id = repo.planned(&[
        ("a", "one.md", &[]),
        ("b", "two.md", &[]),
        ("c", "one.md", &["a"]),
        ("x", "two.md", &[]),
        ("y", "two.md", &["x"]),
    ]);
    let mut tickets = serde_json::Map::new();
    for t in ["a", "b"] {
        let mut v = works(&format!("{t}.txt"));
        v["await_file"] = json!(".kiln/release");
        tickets.insert(t.into(), v);
    }
    tickets.insert("c".into(), works("c.txt"));
    tickets.insert("x".into(), json!({"implementation":{"files":{},"outcome":"failed"},"review":{"standards":approved(),"spec":approved()}}));
    tickets.insert("y".into(), works("y.txt"));
    repo.scenario(json!({"tickets":tickets}));
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(["run", &id, "--fixture", "scenario.json"])
        .current_dir(&repo.path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let observed = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "scheduler state never became visible"
        );
        let out = repo.cli(&["inspect", &id]);
        if let Ok(run) = serde_json::from_slice::<Value>(&out.stdout) {
            if run["scheduler"]["tickets"].is_array()
                && ticket(&run, "x")["state"] == "blocked"
                && ticket(&run, "a")["state"] == "implementing"
                && ticket(&run, "b")["state"] == "implementing"
            {
                break run;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(observed["status"], "running");
    let mut active: Vec<&str> = observed["scheduler"]["active"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    active.sort();
    assert_eq!(active, ["a", "b"]);
    assert_eq!(ticket(&observed, "c")["state"], "waiting");
    assert_eq!(ticket(&observed, "c")["waiting_on"], json!(["a"]));
    assert!(ticket(&observed, "y")["blocker"]
        .as_str()
        .unwrap()
        .contains("prerequisite x is blocked"));
    // The local web view serves the same durable state.
    let mut server = std::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(["serve", "--bind", "127.0.0.1:0"])
        .current_dir(&repo.path)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(server.stdout.take().unwrap()),
        &mut line,
    )
    .unwrap();
    let address = line.trim().rsplit("http://").next().unwrap().to_owned();
    let mut stream = std::net::TcpStream::connect(&address).unwrap();
    std::io::Write::write_all(
        &mut stream,
        format!(
            "GET /api/runs/{id} HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
            address.rsplit(':').next().unwrap()
        )
        .as_bytes(),
    )
    .unwrap();
    let mut body = String::new();
    std::io::Read::read_to_string(&mut stream, &mut body).unwrap();
    server.kill().unwrap();
    let _ = server.wait();
    let web: Value = serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(web["scheduler"]["active"].as_array().unwrap().len(), 2);
    assert_eq!(ticket(&web, "c")["waiting_on"], json!(["a"]));
    fs::write(repo.path.join(".kiln/release"), "go").unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "x is blocked");
    let finished: Value = serde_json::from_slice(&repo.cli(&["inspect", &id]).stdout).unwrap();
    for t in ["a", "b", "c"] {
        assert_eq!(ticket(&finished, t)["state"], "integrated");
    }
    assert!(finished["scheduler"]["active"]
        .as_array()
        .unwrap()
        .is_empty());
}
