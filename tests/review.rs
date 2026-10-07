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
fn scenario(outcome: &str, axis: &str) {
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
    fs::write(
        repo.join("AGENTS.md"),
        format!("Use English labels\n{}", PROVIDER_TOKEN),
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    fs::write(repo.join("kiln.json"),json!({"build":["git","status","--porcelain"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","secrets":{"KILN_TEST_SECRET":["agent"]},"commands":[["git","status","--porcelain"],["git","rev-parse","--verify","absent-ref"],["git","diff","--check"],["git","--version"]]}}).to_string()).unwrap();
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
    fs::write(
        repo.join("agent.json"),
        json!({"files":{"feature.txt":"implemented\n"},"outcome":"completed","log":secret})
            .to_string(),
    )
    .unwrap();
    assert!(cli(&["implement", id, "a", "--fixture", "agent.json"])
        .status
        .success());
    let passed = json!({"outcome":"approved","findings":[],"evidence":"Observed feature.txt satisfies frozen acceptance criteria", "log":secret});
    let mut rejected = json!({"outcome":outcome,"findings":if outcome == "rejected" {json!([{"code":"deviation","message":"feature.txt violates the requested behavior","evidence":"feature.txt diff", "required":true}])} else {json!([])},"evidence":"Observed concrete diff", "log":""});
    if outcome == "missing-evidence" {
        rejected["outcome"] = json!("approved");
        rejected["evidence"] = json!("");
    }
    if outcome == "failed-check" {
        rejected["outcome"] = json!("approved");
        rejected["acceptance_checks"] = json!([["git", "rev-parse", "--verify", "absent-ref"]]);
    }
    fs::write(repo.join("review.json"), json!({"standards":if axis == "standards" {&rejected} else {&passed},"spec":if axis == "spec" {&rejected} else {&passed}}).to_string()).unwrap();
    let out = cli(&["review", id, "a", "--fixture", "review.json"]);
    assert_eq!(
        out.status.success(),
        outcome == "approved",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains(secret));
    let state: Value = serde_json::from_slice(&out.stdout).unwrap();
    let review = &state["reviews"][0];
    assert_eq!(review["passed"], outcome == "approved");
    assert_ne!(
        review["standards"]["context_id"],
        review["spec"]["context_id"]
    );
    assert_ne!(
        review["standards"]["context_id"],
        state["sessions"][0]["context_id"]
    );
    assert!(review["standards"]["checks"][0]["passed"]
        .as_bool()
        .unwrap());
    let context: Value = serde_json::from_slice(
        &fs::read(repo.join(".kiln/contexts").join(format!(
            "{}.json",
            review["spec"]["context_id"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    assert!(context["diff"].as_str().unwrap().contains("implemented"));
    assert_eq!(context["commit"], state["sessions"][0]["commit"]);
    assert!(!context["specs"].as_array().unwrap().is_empty());
    assert!(context["repository_standards"]
        .as_str()
        .unwrap()
        .contains("Use English labels"));
    let engine = kiln::Engine::open(repo).unwrap();
    let run = engine.inspect(id).unwrap();
    assert_eq!(
        engine.review_gate(&run, &run.sessions[0]).unwrap(),
        outcome == "approved"
    );
    if outcome == "approved" {
        let auth = repo.join("provider-auth.json");
        fs::write(
            &auth,
            json!({"tokens":{"access_token":PROVIDER_TOKEN}}).to_string(),
        )
        .unwrap();
        for fail in [true, false] {
            let provider = CredentialReviewer {
                repository: repo.into(),
                fail,
                adapter: kiln::codex::CodexAdapter::new(kiln::codex::CodexConfig {
                    installation: "/unavailable-codex".into(),
                    auth: auth.clone(),
                    model: None,
                    reasoning_effort: None,
                    timeout_seconds: 1,
                }),
            };
            let redacted_run = engine.review_ticket(id, "a", &provider).unwrap();
            let recorded = serde_json::to_string(redacted_run.reviews.last().unwrap()).unwrap();
            assert!(!recorded.contains(PROVIDER_TOKEN));
            if fail {
                for axis in [
                    &redacted_run.reviews.last().unwrap().standards,
                    &redacted_run.reviews.last().unwrap().spec,
                ] {
                    let failure = axis.failure.as_deref().unwrap();
                    assert!(failure.contains("provider failed with [REDACTED]"));
                    assert!(failure.contains("missing observed review evidence"));
                    assert!(axis.result.log.contains("provider failed with [REDACTED]"));
                }
            }
            assert!(
                !fs::read_to_string(repo.join(".kiln/runs").join(format!("{id}.json")))
                    .unwrap()
                    .contains(PROVIDER_TOKEN)
            );
        }
        let snapshot_run = engine
            .review_ticket(id, "a", &SnapshotReviewer { mutate: false })
            .unwrap();
        assert!(snapshot_run.reviews.last().unwrap().passed);
        let changed_run = engine
            .review_ticket(id, "a", &SnapshotReviewer { mutate: true })
            .unwrap();
        assert!(!changed_run.reviews.last().unwrap().passed);
        assert!(!engine
            .review_gate(&changed_run, &changed_run.sessions[0])
            .unwrap());
        let worktree = std::path::Path::new(&run.sessions[0].worktree);
        fs::write(worktree.join("feature.txt"), "unreviewed change").unwrap();
        assert!(!engine.review_gate(&run, &run.sessions[0]).unwrap());
        git(worktree, &["add", "feature.txt"]);
        git(worktree, &["commit", "-qm", "unreviewed"]);
        assert!(!engine.review_gate(&run, &run.sessions[0]).unwrap());
    }
}
#[test]
fn compliant_change_passes_independent_reviews() {
    scenario("approved", "spec");
}
#[test]
fn standards_violation_blocks_gate() {
    scenario("rejected", "standards");
}
#[test]
fn spec_deviation_blocks_gate() {
    scenario("rejected", "spec");
}
#[test]
fn unable_to_verify_blocks_gate() {
    scenario("unable-to-verify", "spec");
}

#[test]
fn agent_approval_cannot_override_missing_evidence() {
    scenario("missing-evidence", "spec");
}
#[test]
fn agent_approval_cannot_override_failed_acceptance_check() {
    scenario("failed-check", "spec");
}

struct SnapshotReviewer {
    mutate: bool,
}
impl kiln::review::ReviewAgent for SnapshotReviewer {
    fn review(
        &self,
        request: &kiln::review::ReviewRequest,
    ) -> anyhow::Result<kiln::review::ReviewResult> {
        assert_eq!(
            fs::read_to_string(request.worktree.join("feature.txt"))?,
            "implemented\n"
        );
        assert_eq!(
            kiln::execution::git(&request.worktree, &["rev-parse", "HEAD"])?,
            request.commit
        );
        assert_ne!(request.context_id, request.author_context_id);
        if self.mutate {
            fs::write(
                request.worktree.join("feature.txt"),
                "verifier altered feature",
            )?;
        }
        Ok(kiln::review::ReviewResult {
            outcome: "approved".into(),
            findings: vec![],
            evidence: "Read feature.txt at supplied implementation snapshot".into(),
            log: String::new(),
            acceptance_checks: vec![],
        })
    }
}

const PROVIDER_TOKEN: &str = "unregistered-provider-auth-value-9234234";
struct CredentialReviewer {
    repository: std::path::PathBuf,
    fail: bool,
    adapter: kiln::codex::CodexAdapter,
}
impl kiln::review::ReviewAgent for CredentialReviewer {
    fn prepare_redaction(&self) -> anyhow::Result<()> {
        self.adapter.prepare_redaction()
    }
    fn redact_output(&self, text: &str) -> String {
        kiln::execution::ImplementationAgent::redact_output(&self.adapter, text)
    }
    fn review(
        &self,
        request: &kiln::review::ReviewRequest,
    ) -> anyhow::Result<kiln::review::ReviewResult> {
        // Observe the durable artifact while invocation is running, before its return/failure.
        let context = fs::read_to_string(
            self.repository
                .join(".kiln/contexts")
                .join(format!("{}.json", request.context_id)),
        )?;
        assert!(!context.contains(PROVIDER_TOKEN));
        assert!(context.contains("[REDACTED]"));
        if self.fail {
            anyhow::bail!("provider failed with {PROVIDER_TOKEN}");
        }
        Ok(kiln::review::ReviewResult {
            outcome: "approved".into(),
            findings: vec![kiln::review::ReviewFinding {
                code: PROVIDER_TOKEN.into(),
                message: PROVIDER_TOKEN.into(),
                evidence: PROVIDER_TOKEN.into(),
                required: true,
            }],
            evidence: PROVIDER_TOKEN.into(),
            log: PROVIDER_TOKEN.into(),
            acceptance_checks: vec![vec!["git".into(), "show".into(), PROVIDER_TOKEN.into()]],
        })
    }
}
