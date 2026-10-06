//! Autonomous decision sessions through the CLI boundary: the decision hierarchy
//! (product objectives > architectural decisions > specs), versioned spec revisions
//! and independent reverification of affected tickets before revised work proceeds.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const SPEC: &str = "# Export\n## Acceptance criteria\n- Users can export their report\n";

struct Repo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let repo = Self { _temp: temp, path };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        fs::write(repo.path.join("export.md"), SPEC).unwrap();
        fs::write(repo.path.join(".gitignore"), ".kiln/\n").unwrap();
        let config = json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],
            "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]},
            "product_objectives":[{"id":"PO-1","statement":"Reports open directly in spreadsheet tools"}],
            "architectural_decisions":[{"id":"ADR-1","statement":"All machine interfaces exchange JSON"}]});
        fs::write(repo.path.join("kiln.json"), config.to_string()).unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(&self.path).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
    fn planned(&self) -> String {
        let prepared = self.cli(&["prepare", "--config", "kiln.json", "--spec", "export.md"]);
        assert!(prepared.status.success(), "{}", String::from_utf8_lossy(&prepared.stderr));
        let id = serde_json::from_slice::<Value>(&prepared.stdout).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        fs::write(
            self.path.join("plan.json"),
            json!({"tickets":[export_ticket(&["export.md#ac-1"])],"verification":verified()}).to_string(),
        )
        .unwrap();
        let planned = self.cli(&["plan", &id, "--fixture", "plan.json"]);
        assert!(planned.status.success(), "{}", String::from_utf8_lossy(&planned.stderr));
        id
    }
    /// Run one decision session; returns the CLI output and the recorded run.
    fn decide(&self, id: &str, ambiguity: Value, agent: Value) -> (Output, Value) {
        fs::write(self.path.join("ambiguity.json"), ambiguity.to_string()).unwrap();
        fs::write(self.path.join("decision.json"), agent.to_string()).unwrap();
        let out = self.cli(&["decide", id, "--ambiguity", "ambiguity.json", "--fixture", "decision.json"]);
        let run = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("no run state: {}", String::from_utf8_lossy(&out.stderr)));
        (out, run)
    }
}
fn verified() -> Value {
    json!({"outcome":"verified","findings":[]})
}
fn export_ticket(covers: &[&str]) -> Value {
    json!({"id":"export","title":"Export","description":"Export the report","acceptance_criteria":["export works"],"covers":covers,"blocked_by":[]})
}
/// The format question, with one conflicting position per hierarchy level.
fn format_conflict() -> Value {
    json!({"id":"export-format","question":"Which format does the export produce?","positions":[
        {"source":"spec","reference":"export.md","statement":"The spec leaves the format open; JSON is implied by the interface"},
        {"source":"architectural_decision","reference":"ADR-1","statement":"Exports are machine interfaces, so JSON"},
        {"source":"product_objective","reference":"PO-1","statement":"Spreadsheet users need CSV"}]})
}
fn proposal(governing: &str) -> Value {
    json!({"governing":governing,"resolution":"Export CSV","rationale":"The product objective outranks the architectural default",
           "evidence":["PO-1 requires spreadsheet compatibility","ADR-1 targets machine interfaces, not user downloads"]})
}
fn decision(run: &Value) -> &Value {
    run["decisions"].as_array().unwrap().last().unwrap()
}

#[test]
fn product_objectives_govern_conflicts_and_resolution_evidence_is_preserved() {
    let repo = Repo::new();
    let id = repo.planned();
    let (out, run) = repo.decide(&id, format_conflict(), json!({"proposal":proposal("PO-1"),"verification":verified()}));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let d = decision(&run);
    assert_eq!(d["outcome"], "resolved");
    assert_eq!(d["question"], "Which format does the export produce?");
    // Positions are presented in hierarchy order with the recorded source statement.
    let ranked: Vec<&str> = d["positions"].as_array().unwrap().iter().map(|p| p["source"].as_str().unwrap()).collect();
    assert_eq!(ranked, ["product_objective", "architectural_decision", "spec"]);
    assert_eq!(d["positions"][0]["source_statement"], "Reports open directly in spreadsheet tools");
    assert_eq!(d["governing"]["reference"], "PO-1");
    assert_eq!(d["resolution"], "Export CSV");
    assert_eq!(d["rationale"], "The product objective outranks the architectural default");
    assert_eq!(d["evidence"].as_array().unwrap().len(), 2);
    assert_ne!(d["context_id"], Value::Null);
    // Persisted, not just printed.
    let inspected: Value = serde_json::from_slice(&repo.cli(&["inspect", &id]).stdout).unwrap();
    assert_eq!(decision(&inspected)["outcome"], "resolved");
}

#[test]
fn a_proposal_not_governed_by_the_highest_ranked_source_is_rejected() {
    let repo = Repo::new();
    let id = repo.planned();
    let (out, run) = repo.decide(&id, format_conflict(), json!({"proposal":proposal("ADR-1"),"verification":verified()}));
    assert!(!out.status.success(), "a hierarchy violation must not be adopted");
    let d = decision(&run);
    assert_eq!(d["outcome"], "rejected");
    assert!(d["findings"].as_array().unwrap().iter().any(|f| f["code"] == "hierarchy_violation"));
}

#[test]
fn architectural_decisions_govern_specs_when_no_objective_applies() {
    let repo = Repo::new();
    let id = repo.planned();
    let conflict = json!({"id":"format","question":"Which format?","positions":[
        {"source":"spec","reference":"export.md#ac-1","statement":"Any downloadable report"},
        {"source":"architectural_decision","reference":"ADR-1","statement":"JSON"}]});
    let (out, run) = repo.decide(&id, conflict.clone(), json!({"proposal":proposal("ADR-1"),"verification":verified()}));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(decision(&run)["governing"]["source"], "architectural_decision");
    assert_eq!(decision(&run)["positions"][1]["source_statement"], "Users can export their report");
    let (out, run) = repo.decide(&id, conflict, json!({"proposal":proposal("export.md#ac-1"),"verification":verified()}));
    assert!(!out.status.success());
    assert_eq!(decision(&run)["outcome"], "rejected");
    assert_eq!(run["decisions"].as_array().unwrap().len(), 2, "rejected decisions stay visible");
}

#[test]
fn positions_must_reference_recorded_sources() {
    let repo = Repo::new();
    let id = repo.planned();
    fs::write(repo.path.join("ambiguity.json"), json!({"id":"x","question":"?","positions":[{"source":"product_objective","reference":"PO-9","statement":"invented"}]}).to_string()).unwrap();
    fs::write(repo.path.join("decision.json"), json!({"proposal":proposal("PO-9"),"verification":verified()}).to_string()).unwrap();
    let out = repo.cli(&["decide", &id, "--ambiguity", "ambiguity.json", "--fixture", "decision.json"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("PO-9"));
}

const REVISED: &str = "# Export\n## Acceptance criteria\n- Users can export their report\n- Exports are CSV files\n";

/// The PO-1 resolution revising the spec, with the export ticket covering `covers`.
fn revising(content: &str, covers: &[&str], verification: Value) -> Value {
    let mut p = proposal("PO-1");
    p["spec_revision"] = json!({"path":"export.md","content":content});
    p["tickets"] = json!([export_ticket(covers)]);
    json!({"proposal":p,"verification":verification})
}

#[test]
fn spec_revisions_are_versioned_run_artifacts_not_overwrites_of_frozen_inputs() {
    let repo = Repo::new();
    let id = repo.planned();
    let (out, run) = repo.decide(&id, format_conflict(), revising(REVISED, &["export.md#ac-1", "export.md#ac-2"], verified()));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Frozen inputs and the developer's file are untouched.
    assert_eq!(run["specs"][0]["content"], SPEC);
    assert_eq!(fs::read_to_string(repo.path.join("export.md")).unwrap(), SPEC);
    let frozen_sha = run["specs"][0]["content_sha256"].clone();
    let r1 = &run["spec_revisions"][0];
    assert_eq!(r1["version"], 1);
    assert_eq!(r1["path"], "export.md");
    assert_eq!(r1["base_sha256"], frozen_sha);
    assert_eq!(r1["content"], REVISED);
    assert_ne!(r1["content_sha256"], frozen_sha);
    assert_eq!(r1["decision_id"], decision(&run)["id"]);
    assert_eq!(r1["status"], "verified");
    assert_eq!(decision(&run)["spec_revision"], 1);

    let again = format!("{REVISED}- Exports name their columns\n");
    let (out, run) = repo.decide(&id, format_conflict(), revising(&again, &["export.md#ac-1", "export.md#ac-2", "export.md#ac-3"], verified()));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r2 = &run["spec_revisions"][1];
    assert_eq!(r2["version"], 2);
    assert_eq!(r2["base_sha256"], run["spec_revisions"][0]["content_sha256"], "revisions chain from the latest verified version");
    assert_eq!(run["specs"][0]["content"], SPEC);
}

#[test]
fn revised_tickets_must_pass_independent_reverification_before_adoption() {
    let repo = Repo::new();
    let id = repo.planned();
    let before: Value = serde_json::from_slice(&repo.cli(&["inspect", &id]).stdout).unwrap();
    // Coverage: the revised criterion is left uncovered.
    let (out, run) = repo.decide(&id, format_conflict(), revising(REVISED, &["export.md#ac-1"], verified()));
    assert!(!out.status.success(), "unverified revisions must not proceed");
    let d = decision(&run);
    assert_eq!(d["outcome"], "rejected");
    assert!(d["findings"].as_array().unwrap().iter().any(|f| f["code"] == "missing_coverage"));
    assert_ne!(d["reverification"]["verification_context"], d["context_id"], "verification is independent");
    assert_eq!(run["spec_revisions"][0]["status"], "rejected", "the revision stays visible");
    assert_eq!(run["plan"], before["plan"], "the accepted plan is unchanged");
    // Dependencies: a revised ticket may not depend on an unknown ticket.
    let mut agent = revising(REVISED, &["export.md#ac-1", "export.md#ac-2"], verified());
    agent["proposal"]["tickets"][0]["blocked_by"] = json!(["ghost"]);
    let (_, run) = repo.decide(&id, format_conflict(), agent);
    assert!(decision(&run)["findings"].as_array().unwrap().iter().any(|f| f["code"] == "unknown_blocker"));
    // Acceptance checks: the independent verifier's failure cannot be overridden.
    let failed = json!({"outcome":"failed","findings":[{"code":"untestable","message":"CSV criterion has no observable check"}]});
    let (_, run) = repo.decide(&id, format_conflict(), revising(REVISED, &["export.md#ac-1", "export.md#ac-2"], failed));
    let codes: Vec<&str> = decision(&run)["findings"].as_array().unwrap().iter().map(|f| f["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"untestable") && codes.contains(&"verification_not_verified"), "{codes:?}");
    assert_eq!(run["plan"], before["plan"]);
    assert!(run["spec_revisions"].as_array().unwrap().iter().all(|r| r["status"] == "rejected"));
}

#[test]
fn revised_work_proceeds_from_the_verified_revision() {
    let repo = Repo::new();
    let id = repo.planned();
    let (out, run) = repo.decide(&id, format_conflict(), revising(REVISED, &["export.md#ac-1", "export.md#ac-2"], verified()));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let ids: Vec<&str> = run["plan"]["requirements"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["export.md#ac-1", "export.md#ac-2"]);
    assert!(run["plan"]["executable"].as_bool().unwrap());
    fs::write(repo.path.join("impl.json"), json!({"files":{"export.csv":"a,b"},"outcome":"completed"}).to_string()).unwrap();
    let out = repo.cli(&["implement", &id, "export", "--fixture", "impl.json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    let session = run["sessions"][0]["id"].as_str().unwrap();
    let context: Value = serde_json::from_str(&fs::read_to_string(repo.path.join(".kiln/contexts").join(format!("{session}.json"))).unwrap()).unwrap();
    assert_eq!(context["specs"][0]["content"], REVISED, "sessions receive the verified revision");
}
