use serde_json::{json, Value};
use std::{fs, process::Command};
fn git(repo: &std::path::Path, args: &[&str]) {
    assert!(Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
}
fn scenario(failure: Option<&str>) {
    let secret = "kiln-private-token-392849";
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    fs::write(
        repo.join("one.md"),
        "# Feature\n## Acceptance criteria\n- Feature works\n",
    )
    .unwrap();
    fs::write(repo.join("original.txt"), "clean").unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    fs::write(repo.join("kiln.json"),json!({"build":["git","status","--porcelain"],"test":if failure == Some("checks") {json!(["git","rev-parse","--verify","absent-ref"])} else {json!(["git","diff","--check"])},"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","secrets":{"KILN_TEST_SECRET":["agent"]},"commands":[["git","status","--porcelain"],["git","rev-parse","--verify","absent-ref"],["git","diff","--check"],["git","--version"]]}}).to_string()).unwrap();
    if failure == Some("policy") {
        let mut config: Value =
            serde_json::from_slice(&fs::read(repo.join("kiln.json")).unwrap()).unwrap();
        let argv = json!(["python3", "-c", "import os,socket; assert 'KILN_TEST_SECRET' not in os.environ; assert not os.path.exists('/home/victor/.codex'); s=socket.socket(); code=s.connect_ex(('1.1.1.1',443)); assert code != 0; print('network denied by isolation')"]);
        config["test"] = argv.clone();
        config["isolation"]["commands"]
            .as_array_mut()
            .unwrap()
            .push(argv);
        fs::write(repo.join("kiln.json"), config.to_string()).unwrap();
    }
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "initial"]);
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(repo)
            .env("KILN_TEST_SECRET", secret)
            .output()
            .unwrap()
    };
    let out = cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = run["id"].as_str().unwrap();
    fs::write(repo.join("plan.json"),json!({"tickets":[{"id":"a","title":"Feature","description":"Deliver feature","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":[]},{"id":"b","title":"Dependent","description":"Deliver dependent","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":["a"]}],"verification":{"outcome":"verified","findings":[]}}).to_string()).unwrap();
    assert!(cli(&["plan", id, "--fixture", "plan.json"])
        .status
        .success());
    fs::write(repo.join("original.txt"), "developer dirty").unwrap();
    fs::write(repo.join("agent.json"),json!({"files":if failure == Some("empty") {json!({})} else {json!({"feature.txt":"implemented\n"})},"outcome":if failure == Some("secret-outcome") {secret} else if failure == Some("agent") {"failed"} else {"completed"},"log":secret}).to_string()).unwrap();
    let out = cli(&["implement", id, "a", "--fixture", "agent.json"]);
    assert_eq!(
        out.status.success(),
        failure.is_none() || failure == Some("policy"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains(secret));
    for entry in fs::read_dir(repo.join(".kiln/runs")).unwrap() {
        assert!(!fs::read_to_string(entry.unwrap().path())
            .unwrap()
            .contains(secret));
    }
    if failure == Some("secret-outcome") {
        use std::io::{BufRead, BufReader, Read, Write};
        let state_file = repo.join(format!(".kiln/runs/{id}.json"));
        let mut state: Value = serde_json::from_slice(&fs::read(&state_file).unwrap()).unwrap();
        state["sessions"][0]["failure"] = json!(secret);
        fs::write(&state_file, serde_json::to_vec(&state).unwrap()).unwrap();
        fs::write(repo.join("kiln.json"), serde_json::to_vec(&json!({"build":["git","status","--porcelain"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","secrets":{"KILN_TEST_SECRET":["agent"]},"commands":[["git","status","--porcelain"],["git","rev-parse","--verify","absent-ref"],["git","diff","--check"],["git","--version"]]}})).unwrap()).unwrap();
        let mut server = Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(["serve", "--bind", "127.0.0.1:0"])
            .current_dir(repo)
            .env("KILN_TEST_SECRET", secret)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        struct Stop(std::process::Child);
        impl Drop for Stop {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let stdout = server.stdout.take().unwrap();
        let _stop = Stop(server);
        let mut address = String::new();
        BufReader::new(stdout).read_line(&mut address).unwrap();
        let mut stream = std::net::TcpStream::connect(
            address
                .trim()
                .strip_prefix("Kiln web view: http://")
                .unwrap(),
        )
        .unwrap();
        write!(
            stream,
            "GET /api/runs/{id} HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
            address
                .trim()
                .strip_prefix("Kiln web view: http://")
                .unwrap()
                .rsplit(':')
                .next()
                .unwrap()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("REDACTED"));
        assert!(!response.contains(secret));
    }
    let inspected = cli(&["inspect", id]);
    assert!(!String::from_utf8_lossy(&inspected.stdout).contains(secret));
    let state: Value = serde_json::from_slice(&out.stdout).unwrap();
    let session = &state["sessions"][0];
    if failure == Some("policy") {
        assert!(session["checks"][1]["stdout"]
            .as_str()
            .unwrap()
            .contains("network denied by isolation"));
    }
    if let Some(reason) = failure.filter(|f| *f != "policy") {
        assert_eq!(session["status"], "failed");
        assert_eq!(session["verification_passed"], false);
        assert!(session["failure"].is_string());
        if reason == "checks" {
            assert_eq!(session["checks"][1]["passed"], false);
            assert_eq!(session["checks"][1]["exit_code"], 128);
            assert!(!session["checks"][1]["stderr"].as_str().unwrap().is_empty());
        }
        assert!(!cli(&["implement", id, "b", "--fixture", "agent.json"])
            .status
            .success());
        return;
    }
    assert_eq!(session["status"], "implemented");
    assert_eq!(session["verification_passed"], true);
    assert!(session["diff"].as_str().unwrap().contains("implemented"));
    assert!(session["commit"].is_string());
    assert_eq!(session["checks"].as_array().unwrap().len(), 2);
    assert_eq!(
        fs::read_to_string(repo.join("original.txt")).unwrap(),
        "developer dirty"
    );
    assert!(!repo.join("feature.txt").exists());
    assert!(!cli(&["implement", id, "b", "--fixture", "agent.json"])
        .status
        .success());
}

#[test]
fn isolated_session_records_real_diff_checks_and_preserves_dirty_checkout() {
    scenario(None);
}
#[test]
fn failed_checks_agent_failures_and_empty_results_never_release_dependents() {
    for reason in ["checks", "empty", "agent"] {
        scenario(Some(reason));
    }
}

#[test]
fn authorized_interpreter_observes_network_denial_private_home_and_role_secrets() {
    scenario(Some("policy"));
}

#[test]
fn provider_outcome_secret_is_absent_from_durable_and_inspected_evidence() {
    scenario(Some("secret-outcome"));
}
