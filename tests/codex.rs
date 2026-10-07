use kiln::{
    codex::{CodexAdapter, CodexConfig, CodexPlanningAgent},
    planning::{PlanningAgent, VerificationRequest},
    sandbox::IsolationPolicy,
};
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn subprocess_events_report_usage_and_redact_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("codex");
    fs::write(&cli,"#!/bin/sh\ncat >/dev/null\necho '{\"type\":\"thread.started\",\"thread_id\":\"test-thread\"}'\necho '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"done private-auth-value\"}}'\necho '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":17,\"output_tokens\":3}}'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let auth = dir.path().join("auth.json");
    fs::write(&auth, r#"{"tokens":{"access_token":"private-auth-value"}}"#).unwrap();
    let config = CodexConfig {
        installation: cli,
        auth,
        model: None,
        timeout_seconds: 5,
    };
    let mut policy = IsolationPolicy {
        network: "allow-all".into(),
        runtime: "system".into(),
        commands: vec![],
        secrets: Default::default(),
    };
    policy.commands.push(config.argv());
    let result = CodexAdapter::new(config)
        .invoke(dir.path(), &policy, "test")
        .unwrap();
    assert_eq!(result.thread_id.as_deref(), Some("test-thread"));
    assert_eq!(result.usage.unwrap()["input_tokens"], 17);
    assert!(result.cost.is_none());
    assert!(!result.log.contains("private-auth-value"));
}

fn fake(script: &str, seconds: u64) -> (tempfile::TempDir, CodexAdapter, IsolationPolicy) {
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("codex");
    fs::write(&cli, format!("#!/bin/sh\ncat >/dev/null\n{script}\n")).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let auth = dir.path().join("auth.json");
    fs::write(&auth, r#"{"tokens":{"access_token":"private-auth-value"}}"#).unwrap();
    let config = CodexConfig {
        installation: cli,
        auth,
        model: None,
        timeout_seconds: seconds,
    };
    let policy = IsolationPolicy {
        network: "allow-all".into(),
        runtime: "system".into(),
        commands: vec![config.argv()],
        secrets: Default::default(),
    };
    (dir, CodexAdapter::new(config), policy)
}

#[test]
fn verified_planning_prompt_requires_findings_to_be_empty() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("codex");
    fs::write(
        &cli,
        r##"#!/bin/sh
prompt=$(cat)
printf '%s\n' '{"type":"thread.started","thread_id":"verify-contract"}'
case "$prompt" in
  *"A verified outcome must have an empty findings array"*)
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"outcome\":\"verified\",\"findings\":[]}"}}'
    ;;
  *)
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"outcome\":\"verified\",\"findings\":[{\"code\":\"COVERAGE_CONFIRMED\",\"message\":\"All requirements are covered\"}]}"}}'
    ;;
esac
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3}}'
"##,
    )
    .unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let auth = dir.path().join("auth.json");
    fs::write(&auth, r#"{"tokens":{"access_token":"private-auth-value"}}"#).unwrap();
    let config = CodexConfig {
        installation: cli,
        auth,
        model: None,
        timeout_seconds: 5,
    };
    let isolation = IsolationPolicy {
        network: "allow-all".into(),
        runtime: "system".into(),
        commands: vec![config.argv()],
        secrets: Default::default(),
    };
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@localhost"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
    }
    fs::write(
        repo.join("one.md"),
        "# Feature\n## Acceptance criteria\n- Works\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-qm", "initial"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let agent = CodexPlanningAgent {
        adapter: CodexAdapter::new(config),
        repository: repo,
        isolation,
    };
    let verification = agent
        .verify(&VerificationRequest {
            context_id: "verify-contract".into(),
            instructions: "Compare proposed tickets with requirements.".into(),
            specs: vec![],
            requirements: vec![],
            tickets: vec![],
        })
        .unwrap();
    assert_eq!(verification.outcome, "verified");
    assert!(verification.findings.is_empty());
}

#[test]
fn exit_zero_with_provider_error_is_failure() {
    let (dir, adapter, policy) = fake(
        "echo '{\"type\":\"error\",\"message\":\"authentication unavailable private-auth-value\"}'",
        3,
    );
    let failure = adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string();
    assert!(failure.contains("authentication unavailable"));
    assert!(!failure.contains("private-auth-value"));
}
#[test]
fn malformed_structured_response_is_preserved_and_redacted() {
    let (dir, adapter, policy) = fake(
        "echo '{\"type\":\"thread.started\",\"thread_id\":\"review-thread\"}'\necho '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"not-json private-auth-value\"}}'\necho '{\"type\":\"turn.completed\"}'",
        3,
    );
    let failure = adapter
        .structured_at::<serde_json::Value>(dir.path(), &policy, "review")
        .unwrap_err()
        .to_string();
    assert!(failure.contains("Codex final response must be the requested JSON value"));
    assert!(failure.contains("not-json [REDACTED]"));
    assert!(!failure.contains("private-auth-value"));
}
fn review_via_fake(response: &str) -> kiln::review::ReviewResult {
    use kiln::{
        execution::CheckResult,
        planning::Ticket,
        review::{ReviewAgent, ReviewRequest},
        FrozenSpec,
    };
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("codex");
    fs::write(
        &cli,
        format!(
            r##"#!/bin/sh
prompt=$(cat)
{response}
printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":9,"output_tokens":4}}}}'"##
        ),
    )
    .unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let auth = dir.path().join("auth.json");
    fs::write(&auth, r#"{"tokens":{"access_token":"private-auth-value"}}"#).unwrap();
    let config = CodexConfig {
        installation: cli,
        auth,
        model: None,
        timeout_seconds: 5,
    };
    let isolation = IsolationPolicy {
        network: "allow-all".into(),
        runtime: "system".into(),
        commands: vec![config.argv()],
        secrets: Default::default(),
    };
    let adapter = CodexAdapter::new(config);
    let request = ReviewRequest {
        context_id: "review-contract".into(),
        axis: "spec".into(),
        author_context_id: "implementation".into(),
        instructions: "Review the supplied change independently.".into(),
        isolation,
        ticket: Ticket {
            id: "pilot-1".into(),
            title: "Selector".into(),
            description: "Extract selector behavior".into(),
            acceptance_criteria: vec!["Filters are combined".into()],
            covers: vec!["spec.md#ac-1".into()],
            blocked_by: vec![],
        },
        specs: vec![FrozenSpec {
            path: "spec.md".into(),
            content: "# Acceptance criteria\n- Filters are combined\n".into(),
            content_sha256: "hash".into(),
            source_revision: None,
        }],
        repository_standards: "Review concrete behavior".into(),
        commit: "abc123".into(),
        diff: "diff --git a/file b/file".into(),
        implementation_checks: vec![CheckResult {
            name: "test".into(),
            command: vec!["npm".into(), "test".into()],
            exit_code: Some(0),
            stdout: "passed".into(),
            stderr: String::new(),
            passed: true,
        }],
        worktree: dir.path().to_owned(),
    };
    adapter.review(&request).unwrap()
}
#[test]
fn review_process_contract_requires_scalar_evidence_and_log() {
    let result = review_via_fake(
        r#"printf '%s\n' '{"type":"thread.started","thread_id":"review-contract"}'
case "$prompt" in
  *"evidence MUST be a nonempty string"*)
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"outcome\":\"approved\",\"findings\":[],\"evidence\":\"Read ui/web/src/components/ChampionCatalog.tsx and verified filter behavior\",\"log\":\"No project commands launched\",\"acceptance_checks\":[]}"}}'
    ;;
  *)
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"outcome\":\"approved\",\"findings\":[],\"evidence\":\"fallback branch\",\"log\":\"checked\",\"acceptance_checks\":[]}"}}'
    ;;
esac"#,
    );
    assert_eq!(result.outcome, "approved");
    assert!(result.evidence.contains("verified filter behavior"));
    assert!(result.log.contains("review-contract"));
    assert!(result.acceptance_checks.is_empty());
}
#[test]
fn review_accepts_list_shaped_evidence_from_codex() {
    let result = review_via_fake(
        r#"printf '%s\n' '{"type":"thread.started","thread_id":"review-contract"}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"outcome\":\"rejected\",\"findings\":[{\"code\":\"CLEAR_RESETS_SORT\",\"message\":\"Clear resets sort\",\"evidence\":[\"ChampionCatalog.tsx:42\",\"sort reset to cost\"],\"required\":true}],\"evidence\":[\"Read ChampionCatalog.tsx\",\"verified filter behavior\"],\"log\":[\"git diff\"],\"acceptance_checks\":[]}"}}'"#,
    );
    assert_eq!(result.outcome, "rejected");
    assert_eq!(
        result.evidence,
        "Read ChampionCatalog.tsx\nverified filter behavior"
    );
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].code, "CLEAR_RESETS_SORT");
    assert_eq!(
        result.findings[0].evidence,
        "ChampionCatalog.tsx:42\nsort reset to cost"
    );
}
#[test]
fn malformed_events_and_missing_completion_are_actionable() {
    for script in ["echo not-json", "echo '{\"type\":\"thread.started\"}'"] {
        let (dir, adapter, policy) = fake(script, 3);
        let failure = adapter
            .invoke(dir.path(), &policy, "test")
            .unwrap_err()
            .to_string();
        assert!(failure.contains("malformed") || failure.contains("without a completed turn"));
    }
}
#[test]
fn timeout_stops_descendants_and_returns_promptly() {
    let (dir, adapter, policy) = fake("sleep 60 &\nwait", 1);
    let start = std::time::Instant::now();
    assert!(adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert!(start.elapsed() < std::time::Duration::from_secs(4));
}
#[test]
fn explicit_stop_cancels_running_session() {
    let start = std::time::Instant::now();
    let (dir, adapter, policy) = fake("sleep 60 &\nwait", 60);
    let stop = adapter.stop_handle();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    assert!(adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string()
        .contains("stopped"));
    assert!(start.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn stop_before_invocation_does_not_start_provider() {
    let (dir, adapter, policy) = fake("touch started; sleep 60", 60);
    adapter.stop();
    fs::remove_file(&adapter.config.installation).unwrap();
    let start = std::time::Instant::now();
    assert!(adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string()
        .contains("stopped"));
    assert!(start.elapsed() < std::time::Duration::from_secs(4));
    assert!(!dir.path().join("started").exists());
}

#[test]
fn usage_limit_failure_is_reported_as_a_provider_limit() {
    for event in [
        r#"{"type":"error","message":"You've hit your usage limit. Upgrade to Pro (https://openai.com/chatgpt/pricing) or try again at 10:05 AM."}"#,
        r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Upgrade to Pro (https://openai.com/chatgpt/pricing) or try again at 10:05 AM."}}"#,
    ] {
        let (dir, adapter, policy) = fake(&format!("cat <<'EVENT'\n{event}\nEVENT"), 3);
        let error = adapter.invoke(dir.path(), &policy, "test").unwrap_err();
        let limit = kiln::limits::ProviderLimit::in_error(&error)
            .unwrap_or_else(|| panic!("not classified as a provider limit: {error:#}"));
        assert_eq!(limit.provider, "codex");
        assert_eq!(limit.reset_at.as_deref(), Some("10:05 AM"));
        assert!(limit.message.starts_with("You've hit your usage limit"));
    }
    // Other provider errors are not limits, even when the transcript mentions one.
    let (dir, adapter, policy) = fake(
        "echo '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"added rate limit middleware\"}}'\necho '{\"type\":\"error\",\"message\":\"authentication unavailable\"}'",
        3,
    );
    let error = adapter.invoke(dir.path(), &policy, "test").unwrap_err();
    assert!(
        kiln::limits::ProviderLimit::in_error(&error).is_none(),
        "{error:#}"
    );
}

#[test]
fn cli_rejects_successful_provider_without_a_git_change() {
    use serde_json::{json, Value};
    use std::process::Command;
    let (dir, adapter, policy) = fake("echo '{\"type\":\"turn.completed\"}'", 3);
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@localhost"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
    }
    fs::write(
        repo.join("one.md"),
        "# Feature\n## Acceptance criteria\n- Works\n",
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    let mut policy = policy;
    policy.commands.extend([
        vec!["git".into(), "diff".into(), "--check".into()],
        vec!["git".into(), "--version".into()],
    ]);
    fs::write(repo.join("kiln.json"),json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["Works"],"codex":adapter.config,"isolation":policy}).to_string()).unwrap();
    for args in [vec!["add", "."], vec!["commit", "-qm", "initial"]] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
    }
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
    };
    let prepared = cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let run: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    let id = run["id"].as_str().unwrap();
    fs::write(repo.join("plan.json"),json!({"tickets":[{"id":"a","title":"Feature","description":"Deliver feature","acceptance_criteria":["Works"],"covers":["one.md#ac-1"]}],"verification":{"outcome":"verified","findings":[]}}).to_string()).unwrap();
    assert!(cli(&["plan", id, "--fixture", "plan.json"])
        .status
        .success());
    let implemented = cli(&[
        "implement",
        id,
        "a",
        "--codex",
        adapter.config.installation.to_str().unwrap(),
    ]);
    assert!(!implemented.status.success());
    let run: Value = serde_json::from_slice(&implemented.stdout).unwrap();
    assert_eq!(run["sessions"][0]["status"], "failed");
    assert_eq!(
        run["sessions"][0]["failure"],
        "agent produced no usable Git change"
    );
}

#[test]
fn stop_tears_down_descendants_that_escape_the_process_group() {
    let (dir, adapter, policy) = fake(
        "setsid sh -c 'touch ready; sleep 2; touch escaped' & wait",
        60,
    );
    let root = dir.path().to_owned();
    let stop = adapter.stop_handle();
    let trigger = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !root.join("ready").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "provider never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let start = std::time::Instant::now();
    assert!(adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string()
        .contains("stopped"));
    trigger.join().unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(!dir.path().join("escaped").exists());
}

#[test]
fn stop_during_sandbox_startup_returns_promptly() {
    for _ in 0..20 {
        let (dir, adapter, policy) = fake("sleep 5 & wait", 60);
        let stop = adapter.stop_handle();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1));
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let start = std::time::Instant::now();
        assert!(adapter
            .invoke(dir.path(), &policy, "test")
            .unwrap_err()
            .to_string()
            .contains("stopped"));
        trigger.join().unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
