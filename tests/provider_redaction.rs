use kiln::{
    codex::{CodexAdapter, CodexConfig},
    execution::{
        AgentResult, FixtureImplementationAgent, ImplementationAgent, ImplementationRequest,
    },
};
use serde_json::json;
use std::{fs, path::PathBuf};
const TOKEN: &str = "private-codex-auth-test-value-713534";
struct Provider {
    adapter: CodexAdapter,
    repository: PathBuf,
    fixture: FixtureImplementationAgent,
    fail: bool,
}
impl ImplementationAgent for Provider {
    fn prepare_redaction(&self) -> anyhow::Result<()> {
        self.adapter.prepare_redaction()
    }
    fn redact_output(&self, text: &str) -> String {
        ImplementationAgent::redact_output(&self.adapter, text)
    }
    fn implement(&self, request: &ImplementationRequest) -> anyhow::Result<AgentResult> {
        assert!(request.repository_instructions.contains(TOKEN));
        let recorded = fs::read_to_string(
            self.repository
                .join(".kiln/contexts")
                .join(format!("{}.json", request.context_id)),
        )?;
        assert!(!recorded.contains(TOKEN));
        assert!(recorded.contains("[REDACTED]"));
        if self.fail {
            anyhow::bail!("invocation failed: {TOKEN}");
        }
        self.fixture.implement(request)
    }
}
#[test]
fn implementation_context_is_redacted_before_provider_runs_or_fails() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    kiln::execution::git(repo, &["init", "-q"]).unwrap();
    fs::write(
        repo.join("AGENTS.md"),
        format!("Repository standard {TOKEN}"),
    )
    .unwrap();
    fs::write(
        repo.join("feature.md"),
        "# Feature\n## Acceptance criteria\n- Feature works\n",
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    fs::write(repo.join("kiln.json"),json!({"build":["git","status","--porcelain"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","status","--porcelain"],["git","diff","--check"],["git","--version"]],"secrets":{}}}).to_string()).unwrap();
    kiln::execution::git(repo, &["add", "."]).unwrap();
    kiln::execution::git(
        repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "initial",
        ],
    )
    .unwrap();
    let engine = kiln::Engine::open(repo).unwrap();
    let run = engine
        .prepare(std::path::Path::new("kiln.json"), &["feature.md".into()])
        .unwrap();
    fs::write(repo.join("plan.json"),json!({"tickets":[{"id":"a","title":"Feature","description":"Deliver feature","acceptance_criteria":["works"],"covers":["feature.md#ac-1"],"blocked_by":[]}],"verification":{"outcome":"verified","findings":[]}}).to_string()).unwrap();
    engine
        .plan(
            &run.id,
            &kiln::planning::FixtureAgent::load(&repo.join("plan.json")).unwrap(),
        )
        .unwrap();
    fs::write(
        repo.join("auth.json"),
        json!({"tokens":{"access_token":TOKEN}}).to_string(),
    )
    .unwrap();
    fs::write(
        repo.join("implementation.json"),
        json!({"files":{"feature.txt":"works"},"outcome":"completed","log":TOKEN}).to_string(),
    )
    .unwrap();
    for fail in [true, false] {
        let provider = Provider {
            adapter: CodexAdapter::new(CodexConfig {
                installation: "/unused".into(),
                auth: repo.join("auth.json"),
                model: None,
                reasoning_effort: None,
                timeout_seconds: 1,
            }),
            repository: repo.into(),
            fixture: FixtureImplementationAgent::load(&repo.join("implementation.json")).unwrap(),
            fail,
        };
        let result = engine.implement_ticket(&run.id, "a", &provider).unwrap();
        assert_eq!(
            result.sessions.last().unwrap().status,
            if fail { "failed" } else { "implemented" }
        );
        assert!(!serde_json::to_string(&result).unwrap().contains(TOKEN));
        assert!(
            !fs::read_to_string(repo.join(".kiln/runs").join(format!("{}.json", run.id)))
                .unwrap()
                .contains(TOKEN)
        );
    }
}
