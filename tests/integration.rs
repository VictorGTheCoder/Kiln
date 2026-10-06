use serde_json::{json, Value};
use std::{fs, process::Command};
fn scenario(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    fs::write(
        repo.join("one.md"),
        "# Feature\n## Acceptance criteria\n- Feature works\n",
    )
    .unwrap();
    fs::write(repo.join("feature.txt"), "initial\n").unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    if mode == "combination" {
        fs::write(repo.join("check.sh"), "#!/bin/sh\nif test -f other.txt && test \"$(cat feature.txt)\" = works; then exit 1; fi\n").unwrap();
    }
    fs::write(repo.join("kiln.json"), json!({"build":["git","diff","--check"],"test":["git","diff","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}}).to_string()).unwrap();
    if mode == "combination" {
        let mut config: Value =
            serde_json::from_slice(&fs::read(repo.join("kiln.json")).unwrap()).unwrap();
        config["test"] = json!(["sh", "check.sh"]);
        config["isolation"]["commands"]
            .as_array_mut()
            .unwrap()
            .push(json!(["sh", "check.sh"]));
        fs::write(repo.join("kiln.json"), config.to_string()).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-qm", "initial"]);
    let primary = git(&["rev-parse", "HEAD"]);
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
    };
    let prepared: Value = serde_json::from_slice(
        &cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]).stdout,
    )
    .unwrap();
    let id = prepared["id"].as_str().unwrap();
    fs::write(repo.join("plan.json"), json!({"tickets":[{"id":"a","title":"Feature","description":"Deliver","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":[]},{"id":"b","title":"Dependent","description":"Deliver dependent","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":["a"]}],"verification":{"outcome":"verified","findings":[]}}).to_string()).unwrap();
    assert!(cli(&["plan", id, "--fixture", "plan.json"])
        .status
        .success());
    fs::write(
        repo.join("implement.json"),
        json!({"files":{"feature.txt":"works\n"},"outcome":"completed"}).to_string(),
    )
    .unwrap();
    assert!(cli(&["implement", id, "a", "--fixture", "implement.json"])
        .status
        .success());
    let approved = json!({"outcome":"approved","findings":[],"evidence":"Observed feature"});
    fs::write(
        repo.join("review.json"),
        json!({"standards":approved,"spec":approved}).to_string(),
    )
    .unwrap();
    assert!(cli(&["review", id, "a", "--fixture", "review.json"])
        .status
        .success());
    let state: Value = serde_json::from_slice(&cli(&["inspect", id]).stdout).unwrap();
    let branch = state["integration_branch"].as_str().unwrap();
    let base = git(&["rev-parse", branch]);
    if mode == "stale" {
        let tree = state["sessions"][0]["worktree"].as_str().unwrap();
        fs::write(std::path::Path::new(tree).join("feature.txt"), "unreviewed").unwrap();
    }
    if ["conflict", "corrected", "unresolved", "combination"].contains(&mode) {
        let other = temp.path().join("other");
        git(&[
            "worktree",
            "add",
            "--detach",
            other.to_str().unwrap(),
            branch,
        ]);
        fs::write(
            other.join(if mode == "combination" {
                "other.txt"
            } else {
                "feature.txt"
            }),
            "parallel change\n",
        )
        .unwrap();
        let run_git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&other)
                .status()
                .unwrap()
                .success());
        };
        run_git(&["add", "."]);
        run_git(&["commit", "-qm", "parallel"]);
        let tip = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&other)
            .output()
            .unwrap();
        git(&[
            "update-ref",
            &format!("refs/heads/{branch}"),
            String::from_utf8_lossy(&tip.stdout).trim(),
            &base,
        ]);
    }
    let before = git(&["rev-parse", branch]);
    if ["corrected", "unresolved", "combination"].contains(&mode) {
        let files = if mode == "unresolved" {
            json!({})
        } else {
            json!({"feature.txt":"combined\n"})
        };
        fs::write(repo.join("correct.json"),json!({"corrections":[{"files":files,"outcome":"completed"}],"reviews":[{"standards":approved,"spec":approved}]}).to_string()).unwrap();
    }
    fs::write(repo.join("one.md"), "dirty primary").unwrap();
    let out = if ["corrected", "unresolved", "combination"].contains(&mode) {
        cli(&["integrate", id, "a", "--fixture", "correct.json"])
    } else {
        cli(&["integrate", id, "a"])
    };
    if ["conflict", "stale", "unresolved", "combination"].contains(&mode) {
        assert!(!out.status.success());
        assert_eq!(git(&["rev-parse", branch]), before);
        if mode == "combination" {
            let failed: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(failed["integrations"][0]["status"], "failed");
            assert_eq!(failed["integrations"][0]["checks"][1]["passed"], false);
            assert_eq!(failed["sessions"][0]["status"], "implemented");
            assert!(failed["integrations"][0]["integrated_commit"].is_null());
            let dependent = cli(&["implement", id, "b", "--fixture", "implement.json"]);
            assert!(!dependent.status.success());
            assert!(String::from_utf8_lossy(&dependent.stderr)
                .contains("prerequisite a is not integrated"));
        }
        return;
    }
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(run["sessions"][0]["status"], "integrated");
    assert_eq!(run["integrations"][0]["status"], "integrated");
    assert_eq!(
        run["integrations"][0]["checks"].as_array().unwrap().len(),
        2
    );
    assert_eq!(git(&["rev-parse", "HEAD"]), primary);
    assert_eq!(
        fs::read_to_string(repo.join("one.md")).unwrap(),
        "dirty primary"
    );
    let branch = run["integration_branch"].as_str().unwrap();
    assert_eq!(
        git(&["show", &format!("{branch}:feature.txt")]),
        if mode == "corrected" {
            "combined"
        } else {
            "works"
        }
    );
}

#[test]
fn reviewed_change_integrates_without_touching_dirty_primary() {
    scenario("success");
}
#[test]
fn conflicts_do_not_publish_without_correction() {
    scenario("conflict");
}
#[test]
fn conflict_resolution_is_checked_and_freshly_reviewed() {
    scenario("corrected");
}
#[test]
fn raw_conflict_markers_cannot_pass_fixture_approval() {
    scenario("unresolved");
}
#[test]
fn edits_after_review_require_renewed_validation() {
    scenario("stale");
}

#[test]
fn combined_failure_does_not_publish_passing_ticket() {
    scenario("combination");
}
