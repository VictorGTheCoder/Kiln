use serde_json::{json, Value};
use std::{
    fs,
    process::{Command, Output},
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
};
use tempfile::TempDir;

fn cli(repo: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kiln"))
        .current_dir(repo.path())
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn local_web_view_exposes_the_same_durable_run_as_inspect() {
    let repo = fixture();
    let state: Value = serde_json::from_slice(&prepare(&repo).stdout).unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .current_dir(repo.path())
        .args(["serve", "--bind", "127.0.0.1:0"])
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
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    let address = line
        .trim()
        .strip_prefix("Kiln web view: http://")
        .expect("web server announces its loopback address");
    let get = |path: &str| {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response.split_once("\r\n\r\n").unwrap().1.to_string()
    };
    assert_eq!(
        serde_json::from_str::<Value>(&get(&format!(
            "/api/runs/{}",
            state["id"].as_str().unwrap()
        )))
        .unwrap(),
        state
    );
    let page = get(&format!("/runs/{}", state["id"].as_str().unwrap()));
    assert!(page.contains("prepared"));
    assert!(page.contains("Approved feature one"));
}
fn fixture() -> TempDir {
    let repo = TempDir::new().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    fs::write(repo.path().join("one.md"), "# One\nApproved feature one\n").unwrap();
    fs::write(repo.path().join("two.md"), "# Two\nApproved feature two\n").unwrap();
    fs::write(
        repo.path().join("kiln.json"),
        serde_json::to_vec(&json!({
            "build": ["git", "status"], "test": ["git", "status"], "startup": ["git", "status"],
            "acceptance_criteria": ["Both features work together"]
        }))
        .unwrap(),
    )
    .unwrap();
    repo
}
fn prepare(repo: &TempDir) -> Output {
    cli(
        repo,
        &[
            "prepare",
            "--config",
            "kiln.json",
            "--spec",
            "one.md",
            "--spec",
            "two.md",
        ],
    )
}
#[test]
fn prepared_run_survives_restart_and_freezes_multiple_specs() {
    let repo = fixture();
    let result = prepare(&repo);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let prepared: Value = serde_json::from_slice(&result.stdout).unwrap();
    fs::write(repo.path().join("one.md"), "Changed externally").unwrap();
    let inspected = cli(&repo, &["inspect", prepared["id"].as_str().unwrap()]);
    assert!(inspected.status.success());
    let state: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(state, prepared);
    assert_eq!(state["status"], "prepared");
    assert_eq!(
        state["specs"][0]["content"],
        "# One\nApproved feature one\n"
    );
    assert_eq!(state["specs"].as_array().unwrap().len(), 2);
    assert_eq!(
        state["specs"][0]["content_sha256"].as_str().unwrap().len(),
        64
    );
}
#[test]
fn invalid_preparation_explains_missing_commands_and_criteria_without_creating_runs() {
    let repo = fixture();
    for (config, expected) in [
        (
            json!({"build": [], "test": ["git"], "startup": ["git"], "acceptance_criteria": ["Works"]}),
            "build",
        ),
        (
            json!({"build": ["kiln-command-does-not-exist"], "test": ["git"], "startup": ["git"], "acceptance_criteria": ["Works"]}),
            "executable",
        ),
        (
            json!({"build": ["git"], "test": ["git"], "startup": ["git"], "acceptance_criteria": []}),
            "acceptance_criteria",
        ),
    ] {
        fs::write(
            repo.path().join("kiln.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let result = prepare(&repo);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
        assert_eq!(
            serde_json::from_slice::<Value>(&cli(&repo, &["inspect"]).stdout).unwrap(),
            json!([])
        );
    }
}
#[cfg(unix)]
#[test]
fn project_relative_commands_are_resolved_against_the_repository() {
    use std::os::unix::fs::PermissionsExt;
    let repo = fixture();
    fs::create_dir(repo.path().join("config")).unwrap();
    fs::write(repo.path().join("check"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(repo.path().join("check"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(repo.path().join("config/kiln.json"), serde_json::to_vec(&json!({
        "build": ["./check"], "test": ["./check"], "startup": ["./check"], "acceptance_criteria": ["Works"]
    })).unwrap()).unwrap();
    let result = cli(
        &repo,
        &[
            "prepare",
            "--config",
            "config/kiln.json",
            "--spec",
            "one.md",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
