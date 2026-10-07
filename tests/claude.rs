use kiln::{
    claude::{ClaudeAdapter, ClaudeConfig},
    sandbox::IsolationPolicy,
};
use std::{fs, os::unix::fs::PermissionsExt};

const CREDENTIALS: &str = r#"{"claudeAiOauth":{"accessToken":"private-auth-value","refreshToken":"private-refresh-value","expiresAt":1},"mcpOAuth":{"other|1":{"accessToken":"unrelated-mcp-secret"}}}"#;

fn fake(script: &str, seconds: u64) -> (tempfile::TempDir, ClaudeAdapter, IsolationPolicy) {
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("claude");
    fs::write(&cli, format!("#!/bin/sh\nprompt=$(cat)\n{script}\n")).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let credentials = dir.path().join(".credentials.json");
    fs::write(&credentials, CREDENTIALS).unwrap();
    let config = ClaudeConfig {
        installation: cli,
        credentials,
        model: None,
        timeout_seconds: seconds,
    };
    let policy = IsolationPolicy {
        network: "allow-all".into(),
        runtime: "system".into(),
        commands: vec![config.argv()],
        secrets: Default::default(),
    };
    (dir, ClaudeAdapter::new(config), policy)
}

const RESULT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"test-session"}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done private-auth-value","session_id":"test-session","total_cost_usd":0.0123,"usage":{"input_tokens":17,"output_tokens":3,"cache_read_input_tokens":40}}'"#;

#[test]
fn stream_events_report_session_usage_estimate_and_redact_credentials() {
    let (dir, adapter, policy) = fake(RESULT, 5);
    let result = adapter.invoke(dir.path(), &policy, "test").unwrap();
    assert_eq!(result.thread_id.as_deref(), Some("test-session"));
    assert_eq!(result.message, "done [REDACTED]");
    assert_eq!(result.usage.as_ref().unwrap()["input_tokens"], 17);
    assert_eq!(result.usage.as_ref().unwrap()["output_tokens"], 3);
    // Subscription cost is never exact: the CLI figure is only an estimate.
    assert!(result.cost.is_none());
    assert_eq!(result.cost_estimate, Some(0.0123));
    assert!(!result.log.contains("private-auth-value"));
}

#[test]
fn recorded_estimate_is_accounted_as_estimate_not_measured_cost() {
    let (dir, adapter, policy) = fake(RESULT, 5);
    let observation = adapter.invoke(dir.path(), &policy, "test").unwrap();
    let log: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&observation).unwrap()).unwrap();
    assert!(log["cost"].is_null());
    assert_eq!(log["cost_estimate"], 0.0123);
}

#[test]
fn session_sees_prompt_on_stdin_and_only_a_private_credentials_copy() {
    let (dir, adapter, policy) = fake(
        r#"creds="$HOME/.claude/.credentials.json"
test "$prompt" = "hello prompt" || { echo "{\"type\":\"result\",\"is_error\":true,\"result\":\"prompt missing\"}"; exit 0; }
test -f "$creds" || { echo "{\"type\":\"result\",\"is_error\":true,\"result\":\"credentials missing\"}"; exit 0; }
grep -q unrelated-mcp-secret "$creds" && { echo "{\"type\":\"result\",\"is_error\":true,\"result\":\"unrelated secrets exposed\"}"; exit 0; }
test "$(stat -c %a "$creds")" = 600 || { echo "{\"type\":\"result\",\"is_error\":true,\"result\":\"credentials not private\"}"; exit 0; }
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"ok","usage":{"input_tokens":1,"output_tokens":1}}'"#,
        5,
    );
    let result = adapter.invoke(dir.path(), &policy, "hello prompt").unwrap();
    assert_eq!(result.message, "ok");
}

#[test]
fn model_is_passed_before_reading_the_prompt() {
    let (_dir, adapter, _) = fake("", 5);
    let mut config = adapter.config.clone();
    config.model = Some("claude-test-model".into());
    let argv = config.argv();
    assert_eq!(argv[0], "/claude/claude");
    for flag in ["-p", "stream-json", "--verbose", "--no-session-persistence"] {
        assert!(argv.iter().any(|a| a == flag), "missing {flag}");
    }
    let model = argv.iter().position(|a| a == "--model").unwrap();
    assert_eq!(argv[model + 1], "claude-test-model");
}

#[test]
fn error_result_with_exit_zero_is_failure_with_message_intact() {
    let (dir, adapter, policy) = fake(
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"Claude AI usage limit reached|1760000000 private-auth-value"}'"#,
        3,
    );
    let failure = adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string();
    assert!(failure.contains("Claude Code provider failure"));
    assert!(failure.contains("Claude AI usage limit reached|1760000000"));
    assert!(!failure.contains("private-auth-value"));
}

#[test]
fn malformed_structured_response_is_preserved_and_redacted() {
    let (dir, adapter, policy) = fake(
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"not-json private-auth-value","usage":{"input_tokens":1,"output_tokens":1}}'"#,
        3,
    );
    let failure = adapter
        .structured_at::<serde_json::Value>(dir.path(), &policy, "review")
        .unwrap_err()
        .to_string();
    assert!(failure.contains("Claude Code final response must be the requested JSON value"));
    assert!(failure.contains("not-json [REDACTED]"));
    assert!(!failure.contains("private-auth-value"));
}

#[test]
fn malformed_events_and_missing_result_are_actionable() {
    for script in [
        "echo not-json",
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"s"}'"#,
    ] {
        let (dir, adapter, policy) = fake(script, 3);
        let failure = adapter
            .invoke(dir.path(), &policy, "test")
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains("malformed") || failure.contains("without a completed turn"),
            "{failure}"
        );
    }
}

#[test]
fn timeout_stops_descendants_and_returns_promptly() {
    let (dir, adapter, policy) = fake("sleep 60 &\nwait", 1);
    let start = std::time::Instant::now();
    let failure = adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string();
    assert!(
        failure.contains("Claude Code session timed out"),
        "{failure}"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn explicit_stop_cancels_running_session() {
    let (dir, adapter, policy) = fake("sleep 60 &\nwait", 60);
    let stop = adapter.stop_handle();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let start = std::time::Instant::now();
    assert!(adapter
        .invoke(dir.path(), &policy, "test")
        .unwrap_err()
        .to_string()
        .contains("stopped"));
    assert!(start.elapsed() < std::time::Duration::from_secs(4));
}

fn git(repo: &std::path::Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap()
        .success());
}

/// Prepared project whose `claude` section points at the fake; returns (repo, run id).
fn prepared_project(
    dir: &std::path::Path,
    adapter: &ClaudeAdapter,
    policy: &IsolationPolicy,
) -> (std::path::PathBuf, String) {
    use serde_json::json;
    let repo = dir.join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@localhost"]);
    fs::write(
        repo.join("one.md"),
        "# Feature\n## Acceptance criteria\n- Works\n",
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    let mut policy = policy.clone();
    policy.commands.extend([
        vec!["git".into(), "diff".into(), "--check".into()],
        vec!["git".into(), "--version".into()],
    ]);
    fs::write(repo.join("kiln.json"),json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["Works"],"claude":adapter.config,"isolation":policy}).to_string()).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "initial"]);
    let prepared = kiln_cli(
        &repo,
        &["prepare", "--config", "kiln.json", "--spec", "one.md"],
    );
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let run: serde_json::Value = serde_json::from_slice(&prepared.stdout).unwrap();
    (repo, run["id"].as_str().unwrap().to_owned())
}

fn kiln_cli(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap()
}

#[test]
fn cli_plans_and_implements_through_claude_and_records_estimated_usage() {
    let (dir, adapter, policy) = fake(
        r#"case "$prompt" in
  *"Return only a JSON array of tickets"*)
    printf '%s\n' '{"type":"result","is_error":false,"result":"[{\"id\":\"a\",\"title\":\"Feature\",\"description\":\"Deliver feature\",\"acceptance_criteria\":[\"Works\"],\"covers\":[\"one.md#ac-1\"],\"blocked_by\":[]}]","usage":{"input_tokens":5,"output_tokens":5}}' ;;
  *"A verified outcome must have an empty findings array"*)
    printf '%s\n' '{"type":"result","is_error":false,"result":"{\"outcome\":\"verified\",\"findings\":[]}","usage":{"input_tokens":5,"output_tokens":5}}' ;;
  *)
    echo hello > greeting.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"impl-session"}'
    printf '%s\n' '{"type":"result","is_error":false,"result":"done","total_cost_usd":0.5,"usage":{"input_tokens":11,"output_tokens":7}}' ;;
esac"#,
        5,
    );
    let (repo, id) = prepared_project(dir.path(), &adapter, &policy);
    let claude = adapter.config.installation.to_str().unwrap();
    let planned = kiln_cli(&repo, &["plan", &id, "--claude", claude]);
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let implemented = kiln_cli(&repo, &["implement", &id, "a", "--claude", claude]);
    assert!(
        implemented.status.success(),
        "{}",
        String::from_utf8_lossy(&implemented.stderr)
    );
    let run: serde_json::Value = serde_json::from_slice(&implemented.stdout).unwrap();
    assert_eq!(run["sessions"][0]["status"], "implemented");
    let log: serde_json::Value =
        serde_json::from_str(run["sessions"][0]["agent_log"].as_str().unwrap()).unwrap();
    assert_eq!(log["thread_id"], "impl-session");
    assert_eq!(log["usage"]["input_tokens"], 11);
    assert!(log["cost"].is_null());
    assert_eq!(log["cost_estimate"], 0.5);
    let stored = fs::read_to_string(repo.join(".kiln").join("runs").join(&id).join("run.json"))
        .unwrap_or_default();
    assert!(!stored.contains("private-auth-value"));
    assert!(!String::from_utf8_lossy(&implemented.stdout).contains("private-auth-value"));
}

#[test]
fn cli_rejects_codex_and_claude_together() {
    let (dir, adapter, policy) = fake(RESULT, 5);
    let (repo, id) = prepared_project(dir.path(), &adapter, &policy);
    let claude = adapter.config.installation.to_str().unwrap();
    let output = kiln_cli(
        &repo,
        &["implement", &id, "a", "--claude", claude, "--codex", claude],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn review_tolerates_list_shaped_evidence() {
    use kiln::{
        execution::CheckResult,
        planning::Ticket,
        review::{ReviewAgent, ReviewRequest},
        FrozenSpec,
    };
    let (dir, adapter, isolation) = fake(
        r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"review-contract"}'
printf '%s\n' '{"type":"result","is_error":false,"result":"```json\n{\"outcome\":\"rejected\",\"findings\":[{\"code\":\"X\",\"message\":\"m\",\"evidence\":[\"a.rs:1\",\"b\"],\"required\":true}],\"evidence\":[\"Read a.rs\",\"checked\"],\"log\":[\"git diff\"],\"acceptance_checks\":[]}\n```","usage":{"input_tokens":9,"output_tokens":4}}'"#,
        5,
    );
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
    let result = adapter.review(&request).unwrap();
    assert_eq!(result.outcome, "rejected");
    assert_eq!(result.evidence, "Read a.rs\nchecked");
    assert_eq!(result.findings[0].evidence, "a.rs:1\nb");
    assert!(result.log.contains("review-contract"));
}
