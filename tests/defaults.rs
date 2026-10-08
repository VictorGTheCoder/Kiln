//! Public CLI coverage for running user-facing commands with inferred defaults:
//! repository = current directory, config = ./kiln.json, GitHub repository from
//! the `origin` remote, provider from the `agent` field or PATH.
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
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

/// A temporary target repository with a fake `gh` (serving a two-issue open
/// graph for example/project) and an optional fake `codex`/`claude` on PATH.
struct Target {
    temp: tempfile::TempDir,
    repo: PathBuf,
    bin: PathBuf,
}
impl Target {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("target");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let git = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        std::os::unix::fs::symlink(git.trim(), bin.join("git")).unwrap();
        let target = Self { temp, repo, bin };
        target.git(&["init", "-q"]);
        fs::write(
            temp_path(&target, "auth.json"),
            r#"{"tokens":{"access_token":"private-auth-value"}}"#,
        )
        .unwrap();
        fs::write(
            temp_path(&target, "credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"private-auth-value"}}"#,
        )
        .unwrap();
        target.fake_gh();
        target
    }
    fn git(&self, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
    fn origin(&self, url: &str) -> &Self {
        self.git(&["remote", "add", "origin", url]);
        self
    }
    /// Project configuration usable with either provider; `agent` is optional.
    fn config_value(&self, agent: Option<&str>) -> Value {
        let mut config = json!({
            "build":["git"], "test":["git"], "startup":["git"],
            "acceptance_criteria":["Backlog plans are verified before execution"],
            "codex":{"auth": temp_path(self, "auth.json"), "timeout_seconds": 30},
            "claude":{"credentials": temp_path(self, "credentials.json"), "timeout_seconds": 30},
            "isolation":{"network":"allow-all","runtime":"system","commands":[["git"], CODEX_ARGV, CLAUDE_ARGV]}
        });
        if let Some(agent) = agent {
            config["agent"] = json!(agent);
        }
        config
    }
    fn config(&self, agent: Option<&str>) -> &Self {
        fs::write(
            self.repo.join("kiln.json"),
            self.config_value(agent).to_string(),
        )
        .unwrap();
        self
    }
    fn fake_gh(&self) {
        let issue = |number: u64, title: &str, body: &str| {
            json!({"number":number,"html_url":format!("https://github.com/example/project/issues/{number}"),
                "title":title,"body":body,"labels":[],"assignee":null,"state":"open"})
        };
        let one = issue(1, "Base", "## Acceptance criteria\n- Create base\n");
        let two = issue(
            2,
            "Dependent",
            "## Acceptance criteria\n- Use base\n\n## Blocked by\n- #1\n",
        );
        let script = format!(
            "#!/bin/sh\ncase \"$4\" in\n\
             repos/example/project/issues\\?state=open*) printf '%s\\n' '{list}' ;;\n\
             repos/example/project/issues/1) printf '%s\\n' '{one}' ;;\n\
             repos/example/project/issues/2) printf '%s\\n' '{two}' ;;\n\
             repos/example/project/issues/*) printf '%s\\n' '[]' ;;\n\
             *) printf 'unexpected gh endpoint %s\\n' \"$4\" >&2; exit 1 ;;\nesac\n",
            list = json!([one, two]),
        );
        executable(&self.bin.join("gh"), &script);
    }
    /// Fake provider that plans the two-issue graph and verifies the plan.
    fn provider(&self, name: &str) -> PathBuf {
        let tickets = json!([
            {"id":"github:example/project#1","title":"Base","description":"Create the base behavior","acceptance_criteria":["Create base"],"covers":["github-example-project-1.md#ac-1"],"blocked_by":[]},
            {"id":"github:example/project#2","title":"Dependent","description":"Use the base behavior","acceptance_criteria":["Use base"],"covers":["github-example-project-2.md#ac-1"],"blocked_by":["github:example/project#1"]}
        ]);
        let verified = json!({"outcome":"verified","findings":[]});
        let inferred = json!({"acceptance_criteria":[]});
        let events = |answer: &Value| -> String {
            let text = answer.to_string();
            match name {
                "codex" => [
                    json!({"type":"thread.started","thread_id":"fake"}),
                    json!({"type":"item.completed","item":{"type":"agent_message","text":text}}),
                    json!({"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}),
                ],
                _ => [
                    json!({"type":"system","subtype":"init","session_id":"fake"}),
                    json!({"type":"assistant","message":{}}),
                    json!({"type":"result","subtype":"success","is_error":false,"result":text,"session_id":"fake"}),
                ],
            }
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
        };
        let script = format!(
            "#!/bin/sh\nprompt=$(cat)\ncase \"$prompt\" in\n\
             *\"Return only a JSON array of tickets\"*) cat <<'EOF'\n{}\nEOF\n;;\n\
             *\"A verified outcome must have an empty findings array\"*) cat <<'EOF'\n{}\nEOF\n;;\n\
             *) cat <<'EOF'\n{}\nEOF\n;;\nesac\n",
            events(&tickets),
            events(&verified),
            events(&inferred)
        );
        let path = self.bin.join(name);
        executable(&path, &script);
        path
    }
    /// Provider executable that must never be selected.
    fn broken_provider(&self, name: &str) {
        executable(
            &self.bin.join(name),
            "#!/bin/sh\necho 'wrong provider selected' >&2\nexit 9\n",
        );
    }
    fn kiln_in(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(dir)
            .env("PATH", &self.bin)
            .output()
            .unwrap()
    }
    fn kiln(&self, args: &[&str]) -> Output {
        self.kiln_in(&self.repo, args)
    }
}
fn temp_path(target: &Target, name: &str) -> PathBuf {
    target.temp.path().join(name)
}
fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn failure(output: &Output) -> String {
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn planned_json(output: &Output) -> Value {
    serde_json::from_str(&success(output)).expect("--json prints the run as JSON")
}

#[test]
fn plan_with_no_flags_plans_the_open_issue_graph_and_stops_before_delivery() {
    let target = Target::new();
    target
        .config(None)
        .origin("git@github.com:example/project.git");
    target.provider("codex");

    let stdout = success(&target.kiln(&["plan"]));

    assert!(
        serde_json::from_str::<Value>(&stdout).is_err(),
        "default output is human-readable: {stdout}"
    );
    for expected in [
        "example/project",
        "codex",
        "Base",
        "Dependent",
        "kiln start",
    ] {
        assert!(
            stdout.contains(expected),
            "missing {expected:?} in:\n{stdout}"
        );
    }
    let runs: Value = serde_json::from_str(&success(&target.kiln(&["inspect"]))).unwrap();
    let id = runs[0].as_str().unwrap();
    let run: Value = serde_json::from_str(&success(&target.kiln(&["inspect", id]))).unwrap();
    assert_eq!(run["plan"]["executable"], true);
    assert_eq!(run["plan"]["tickets"].as_array().unwrap().len(), 2);
    assert!(run["sessions"].as_array().unwrap().is_empty());
    assert!(run["integrations"].as_array().unwrap().is_empty());
    assert!(run["publication"].is_null());
}

#[test]
fn github_repository_is_inferred_from_https_and_ssh_origin_urls() {
    for url in [
        "https://github.com/example/project.git",
        "https://github.com/example/project",
        "https://token@github.com/example/project.git/",
        "git@github.com:example/project.git",
        "git@github.com:example/project",
        "ssh://git@github.com/example/project.git",
        "ssh://git@github.com:22/example/project",
    ] {
        let target = Target::new();
        target.config(None).origin(url);
        target.provider("codex");

        let run = planned_json(&target.kiln(&["plan", "--json"]));

        assert_eq!(
            run["backlog"]["github_repository"], "example/project",
            "origin {url}"
        );
        assert_eq!(run["plan"]["executable"], true, "origin {url}");
    }
}

#[test]
fn agent_field_selects_the_provider_found_on_path() {
    let target = Target::new();
    target
        .config(Some("claude"))
        .origin("https://github.com/example/project.git");
    target.broken_provider("codex");
    target.provider("claude");

    let stdout = success(&target.kiln(&["plan"]));

    assert!(stdout.contains("claude"), "{stdout}");
}

#[test]
fn without_agent_field_codex_is_preferred_then_claude() {
    // Both on PATH: codex wins.
    let target = Target::new();
    target
        .config(None)
        .origin("https://github.com/example/project.git");
    target.provider("codex");
    target.broken_provider("claude");
    success(&target.kiln(&["plan"]));

    // Only claude on PATH: claude is used, and the config without `agent` stays valid.
    let target = Target::new();
    target
        .config(None)
        .origin("https://github.com/example/project.git");
    target.provider("claude");
    let stdout = success(&target.kiln(&["plan"]));
    assert!(stdout.contains("claude"), "{stdout}");
}

#[test]
fn invalid_agent_field_is_rejected() {
    let target = Target::new();
    target
        .config(Some("gpt"))
        .origin("https://github.com/example/project.git");
    target.provider("codex");

    let stderr = failure(&target.kiln(&["plan"]));

    assert!(
        stderr.contains("agent must be \"codex\" or \"claude\""),
        "{stderr}"
    );
}

#[test]
fn missing_inputs_give_errors_naming_what_is_missing_and_how_to_fix_it() {
    // No kiln.json.
    let target = Target::new();
    target.origin("https://github.com/example/project.git");
    target.provider("codex");
    let stderr = failure(&target.kiln(&["plan"]));
    assert!(stderr.contains("kiln.json"), "{stderr}");
    assert!(stderr.contains("kiln init"), "{stderr}");

    // No origin remote.
    let target = Target::new();
    target.config(None);
    target.provider("codex");
    let stderr = failure(&target.kiln(&["plan"]));
    assert!(stderr.contains("origin"), "{stderr}");
    assert!(stderr.contains("git remote add origin"), "{stderr}");
    assert!(stderr.contains("--github-repo"), "{stderr}");

    // Origin that is not on GitHub.
    let target = Target::new();
    target
        .config(None)
        .origin("https://gitlab.com/example/project.git");
    target.provider("codex");
    let stderr = failure(&target.kiln(&["plan"]));
    assert!(stderr.contains("gitlab.com"), "{stderr}");
    assert!(stderr.contains("--github-repo"), "{stderr}");

    // No provider on PATH.
    let target = Target::new();
    target
        .config(None)
        .origin("https://github.com/example/project.git");
    let stderr = failure(&target.kiln(&["plan"]));
    assert!(stderr.contains("no provider found"), "{stderr}");
    for hint in ["codex", "claude", "PATH", "\"agent\"", "--codex"] {
        assert!(stderr.contains(hint), "missing {hint:?} in {stderr}");
    }

    // Configured agent missing from PATH.
    let target = Target::new();
    target
        .config(Some("claude"))
        .origin("https://github.com/example/project.git");
    target.provider("codex");
    let stderr = failure(&target.kiln(&["plan"]));
    assert!(stderr.contains("`claude`"), "{stderr}");
    assert!(stderr.contains("--claude"), "{stderr}");
    assert!(
        fs::read_dir(target.repo.join(".kiln/runs")).is_err(),
        "no run is recorded when inference fails"
    );
}

#[test]
fn explicit_flags_override_every_inferred_value() {
    let target = Target::new();
    // No kiln.json, no origin and no provider on PATH: nothing can be inferred.
    fs::write(
        target.repo.join("custom.json"),
        target.config_value(Some("claude")).to_string(),
    )
    .unwrap();
    let codex = target.provider("codex");
    let hidden = target.temp.path().join("providers");
    fs::create_dir_all(&hidden).unwrap();
    fs::rename(&codex, hidden.join("codex")).unwrap();
    let outside = target.temp.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();

    let run = planned_json(&target.kiln_in(
        &outside,
        &[
            "plan",
            "--json",
            "--repo",
            target.repo.to_str().unwrap(),
            "--config",
            "custom.json",
            "--github-repo",
            "example/project",
            "--codex",
            hidden.join("codex").to_str().unwrap(),
        ],
    ));

    assert_eq!(run["backlog"]["github_repository"], "example/project");
    assert_eq!(run["plan"]["executable"], true);
    // Success proves --codex won over the config's "agent": "claude" (absent from PATH).

    // An explicit GitHub repository wins over the origin remote.
    let target = Target::new();
    target
        .config(None)
        .origin("https://github.com/someone/else.git");
    target.provider("codex");
    let run = planned_json(&target.kiln(&["plan", "--json", "--github-repo", "example/project"]));
    assert_eq!(run["backlog"]["github_repository"], "example/project");
}

#[test]
fn help_lists_only_user_facing_commands_and_hidden_commands_still_run() {
    let target = Target::new();
    let help = success(&target.kiln(&["--help"]));
    let commands: Vec<&str> = help
        .split("Commands:")
        .nth(1)
        .expect("help lists commands")
        .split("Options:")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .collect();
    let user_facing = [
        "start",
        "plan",
        "status",
        "logs",
        "dashboard",
        "pause",
        "resume",
        "cancel",
    ];
    assert!(
        commands.iter().all(|name| user_facing.contains(name)),
        "{help}"
    );
    for name in ["plan", "pause", "resume", "cancel"] {
        assert!(commands.contains(&name), "missing {name} in {help}");
    }
    // Hidden internal commands keep working with JSON output.
    let runs: Value = serde_json::from_str(&success(&target.kiln(&["inspect"]))).unwrap();
    assert!(runs.as_array().unwrap().is_empty());
    let stderr = failure(&target.kiln(&["start-backlog", "--help-not-a-flag"]));
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}
