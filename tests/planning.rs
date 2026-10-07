use serde_json::{json, Value};
use std::{fs, process::Command};
fn scenario(mutation: Option<&str>) {
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    for name in ["one", "two"] {
        fs::write(
            repo.path().join(format!("{name}.md")),
            format!("# {name}\n## Acceptance criteria\n- {name} works\n"),
        )
        .unwrap();
    }
    fs::write(repo.path().join("kiln.json"),json!({"build":["git"],"test":["git"],"startup":["git"],"acceptance_criteria":["Together"],"isolation":{"network":"none","runtime":"system","commands":[["git"]]}}).to_string()).unwrap();
    let cli = |args: Vec<&str>| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap()
    };
    let prepared = cli(vec![
        "prepare",
        "--config",
        "kiln.json",
        "--spec",
        "one.md",
        "--spec",
        "two.md",
    ]);
    let state: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    let id = state["id"].as_str().unwrap();
    fs::write(repo.path().join("one.md"), "changed").unwrap();
    let ticket = |id: &str, spec: &str, blockers: Vec<&str>| json!({"id":id,"title":format!("Deliver {id}"),"description":"Complete observable feature","acceptance_criteria":["Feature works"],"covers":[format!("{spec}#ac-1")],"blocked_by":blockers});
    let mut fixture = json!({"tickets":[ticket("a","one.md",vec![]),ticket("b","two.md",vec!["a"])],"verification":{"outcome":"verified","findings":[]}});
    match mutation {
        Some("missing_coverage") => {
            fixture["tickets"].as_array_mut().unwrap().pop();
        }
        Some("unknown_blocker") => fixture["tickets"][1]["blocked_by"] = json!(["unknown"]),
        Some("cycle") => fixture["tickets"][0]["blocked_by"] = json!(["b"]),
        Some("granularity") => fixture["tickets"][0]["acceptance_criteria"] = json!([]),
        Some("verification_not_verified") => {
            fixture["verification"]["outcome"] = json!("unable-to-verify")
        }
        _ => {}
    }
    fs::write(repo.path().join("agent.json"), fixture.to_string()).unwrap();
    let result = cli(vec!["plan", id, "--fixture", "agent.json"]);
    if let Some(code) = mutation {
        assert!(!result.status.success());
        let rejected: Value = serde_json::from_slice(&cli(vec!["inspect", id]).stdout).unwrap();
        assert_eq!(rejected["status"], "plan_rejected");
        assert_eq!(rejected["plan"]["executable"], false);
        assert!(rejected["plan"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == code));
        return;
    }
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let planned: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(planned["status"], "planned");
    assert_eq!(planned["plan"]["executable"], true);
    assert_eq!(planned["plan"]["dependency_graph"]["b"], json!(["a"]));
    assert_ne!(
        planned["plan"]["generation_context"],
        planned["plan"]["verification_context"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&cli(vec!["inspect", id]).stdout).unwrap(),
        planned
    );
}

#[test]
fn frozen_multispec_plan_is_independently_verified_and_inspectable() {
    scenario(None);
}
#[test]
fn rejected_plans_keep_inspectable_findings() {
    for code in [
        "missing_coverage",
        "unknown_blocker",
        "cycle",
        "granularity",
        "verification_not_verified",
    ] {
        scenario(Some(code));
    }
}
