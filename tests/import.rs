use serde_json::{json, Value};
use std::{fs, process::Command};
fn scenario(mutation: Option<&str>) {
    let repo = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    fs::write(
        repo.path().join("spec.md"),
        "# Spec\n## Acceptance criteria\n- Feature works\n",
    )
    .unwrap();
    fs::write(
        repo.path().join("kiln.json"),
        json!({"build":["git"],"test":["git"],"startup":["git"],"acceptance_criteria":["Works"],"isolation":{"network":"none","runtime":"system","commands":[["git"]]}})
            .to_string(),
    )
    .unwrap();
    let cli = |args: Vec<&str>| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap()
    };
    let output = cli(vec![
        "prepare",
        "--config",
        "kiln.json",
        "--spec",
        "spec.md",
    ]);
    let run: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = run["id"].as_str().unwrap();
    fs::write(repo.path().join("issues.json"),json!({"issues":[{"number":7,"url":"https://github.com/example/project/issues/7","title":"Feature","body":"## Acceptance criteria\n- Feature works\n## Spec coverage\n- spec.md#ac-1","labels":["feature"],"blocked_by":[]}]}).to_string()).unwrap();
    fs::write(
        repo.path().join("verify.json"),
        json!({"tickets":[],"verification":{"outcome":"verified","findings":[]}}).to_string(),
    )
    .unwrap();
    let mut fixture: Value =
        serde_json::from_str(&fs::read_to_string(repo.path().join("issues.json")).unwrap())
            .unwrap();
    match mutation {
 Some("unknown_blocker") => fixture["issues"][0]["body"] = json!("## Acceptance criteria\n- Feature works\n## Spec coverage\n- spec.md#ac-1\n## Blocked by\n- #9: dependency"),
 Some("cycle") => fixture["issues"][0]["blocked_by"] = json!(["github:example/project#7"]),
 Some("spec_divergence") => fixture["issues"][0]["body"] = json!("## Acceptance criteria\n- Feature does something else\n## Spec coverage\n- spec.md#ac-1"),
 _ => {}
 }
    fs::write(repo.path().join("issues.json"), fixture.to_string()).unwrap();
    fixture["issues"].as_array_mut().unwrap().push(json!({"number":8,"url":"https://github.com/example/project/issues/8","title":"Related feature","body":"## Acceptance criteria\n- Feature works\n## Spec coverage\n- spec.md#ac-1","labels":["feature"],"blocked_by":["github:example/project#7"]}));
    fs::write(repo.path().join("issues.json"), fixture.to_string()).unwrap();
    for _ in 0..2 {
        let output = cli(vec![
            "import",
            id,
            "--github-repo",
            "example/project",
            "--issue",
            "7",
            "--issue",
            "8",
            "--fixture",
            "issues.json",
            "--verification-fixture",
            "verify.json",
        ]);
        if let Some(code) = mutation {
            assert!(!output.status.success());
            let inspected = cli(vec!["inspect", id]);
            let rejected: Value = serde_json::from_slice(&inspected.stdout).unwrap();
            assert_eq!(rejected["plan"]["executable"], false);
            assert!(rejected["plan"]["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["code"] == code));
            assert_eq!(
                rejected["specs"][0]["content"],
                "# Spec\n## Acceptance criteria\n- Feature works\n"
            );
            return;
        }
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let imported: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            imported["plan"]["tickets"][0]["id"],
            "github:example/project#7"
        );
        assert_eq!(imported["imported_issues"][0]["labels"], json!(["feature"]));
        assert_eq!(imported["imported_issues"].as_array().unwrap().len(), 2);
        assert_eq!(imported["status"], "planned");
    }
}

#[test]
fn import_preserves_issue_identity_and_frozen_spec_authority() {
    scenario(None);
}
#[test]
fn imported_missing_blocker_cycles_and_divergence_remain_nonexecutable() {
    for code in ["unknown_blocker", "cycle", "spec_divergence"] {
        scenario(Some(code));
    }
}

#[cfg(unix)]
#[test]
fn import_can_independently_verify_github_tickets_with_codex() {
    use std::os::unix::fs::PermissionsExt;

    let repo = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    fs::write(
        repo.path().join("spec.md"),
        "# Spec\n## Acceptance criteria\n- Feature works\n",
    )
    .unwrap();

    let codex = repo.path().join("codex");
    fs::write(
        &codex,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"import-verifier\"}'\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"outcome\\\":\\\"verified\\\",\\\"findings\\\":[]}\"}}'\nprintf '%s\\n' '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":4}}'\n",
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();
    let auth = repo.path().join("auth.json");
    fs::write(
        &auth,
        r#"{"tokens":{"access_token":"private-import-auth"}}"#,
    )
    .unwrap();
    let codex_config = kiln::codex::CodexConfig {
        installation: codex.clone(),
        auth: auth.clone(),
        model: None,
        timeout_seconds: 5,
    };
    let mut commands = vec![vec!["git".into()]];
    commands.push(codex_config.argv());
    fs::write(
        repo.path().join("kiln.json"),
        json!({
            "build":["git"],
            "test":["git"],
            "startup":["git"],
            "acceptance_criteria":["Feature works"],
            "codex":codex_config,
            "isolation":{"network":"allow-all","runtime":"system","commands":commands}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        repo.path().join("issues.json"),
        json!({"issues":[{
            "number":7,
            "url":"https://github.com/example/project/issues/7",
            "title":"Feature",
            "body":"## Acceptance criteria\n- Feature works\n## Spec coverage\n- spec.md#ac-1",
            "labels":["ready-for-agent"],
            "blocked_by":[]
        }]})
        .to_string(),
    )
    .unwrap();

    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap()
    };
    let prepared = cli(&["prepare", "--config", "kiln.json", "--spec", "spec.md"]);
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let run: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    let id = run["id"].as_str().unwrap();
    let imported = cli(&[
        "import",
        id,
        "--github-repo",
        "example/project",
        "--issue",
        "7",
        "--fixture",
        "issues.json",
        "--codex",
        codex.to_str().unwrap(),
    ]);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let run: Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert_eq!(run["status"], "planned");
    assert_eq!(run["plan"]["executable"], true);
    assert_eq!(run["plan"]["verification"]["outcome"], "verified");
    assert_eq!(
        run["plan"]["verification_context"],
        format!("{id}-import-verification")
    );
    assert_eq!(run["plan"]["tickets"][0]["id"], "github:example/project#7");
}

#[test]
fn import_requires_exactly_one_verification_provider() {
    let invoke = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args([
                "import",
                "run-id",
                "--github-repo",
                "example/project",
                "--issue",
                "7",
            ])
            .args(extra)
            .output()
            .unwrap()
    };

    let missing = invoke(&[]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("required"));

    let both = invoke(&[
        "--verification-fixture",
        "verify.json",
        "--codex",
        "/tmp/codex",
    ]);
    assert!(!both.status.success());
    assert!(String::from_utf8_lossy(&both.stderr).contains("cannot be used with"));
}

#[cfg(unix)]
#[test]
fn github_rest_contract_preserves_native_blocker_and_reports_unselected_prerequisite() {
    use std::os::unix::fs::PermissionsExt;
    let repo = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    fs::write(
        repo.path().join("spec.md"),
        "## Acceptance criteria\n- Approved requirement\n",
    )
    .unwrap();
    fs::write(
        repo.path().join("kiln.json"),
        json!({"build":["git"],"test":["git"],"startup":["git"],"acceptance_criteria":["Works"],"isolation":{"network":"none","runtime":"system","commands":[["git"]]}})
            .to_string(),
    )
    .unwrap();
    fs::write(
        repo.path().join("verify.json"),
        json!({"tickets":[],"verification":{"outcome":"verified","findings":[]}}).to_string(),
    )
    .unwrap();
    let bin = repo.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("issue.json"),
        include_str!("fixtures/github/issue-15.json"),
    )
    .unwrap();
    fs::write(
        bin.join("blockers.json"),
        include_str!("fixtures/github/blockers-15.json"),
    )
    .unwrap();
    fs::write(bin.join("gh"), "#!/bin/sh\ncase \"$4\" in\n repos/VictorGTheCoder/kiln/issues/15) cat \"$(dirname \"$0\")/issue.json\";;\n repos/VictorGTheCoder/kiln/issues/15/dependencies/blocked_by?*) cat \"$(dirname \"$0\")/blockers.json\";;\n *) exit 1;;\nesac\n").unwrap();
    fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .current_dir(repo.path())
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .args(args)
            .output()
            .unwrap()
    };
    let prepared = cli(&["prepare", "--config", "kiln.json", "--spec", "spec.md"]);
    let run: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    let output = cli(&[
        "import",
        run["id"].as_str().unwrap(),
        "--github-repo",
        "VictorGTheCoder/kiln",
        "--issue",
        "15",
        "--verification-fixture",
        "verify.json",
    ]);
    assert!(!output.status.success());
    let imported: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        imported["imported_issues"][0]["blocked_by"],
        json!(["github:VictorGTheCoder/kiln#2"])
    );
    assert_eq!(
        imported["imported_issues"][0]["labels"],
        json!(["ready-for-agent"])
    );
    assert!(imported["plan"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == "unknown_blocker"));
}
