use serde_json::{json, Value};
use std::{fs, process::Command};
fn scenario_with_ticket(stalled: bool, exhausted: bool, ticket: &str) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
            .status
            .success())
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    fs::write(
        repo.join("one.md"),
        "# Feature\n## Acceptance criteria\n- Feature works\n",
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    fs::write(repo.join("kiln.json"),json!({"build":["git","diff","--cached","--check"],"test":["git","diff","--cached","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--cached","--check"],["git","--version"]]}}).to_string()).unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "initial"]);
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
    };
    let out = cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = run["id"].as_str().unwrap();
    fs::write(repo.join("plan.json"),json!({"tickets":[{"id":ticket,"title":"Feature","description":"Deliver feature","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":[]}],"verification":{"outcome":"verified","findings":[]}}).to_string()).unwrap();
    assert!(cli(&["plan", id, "--fixture", "plan.json"])
        .status
        .success());
    fs::write(
        repo.join("bad.json"),
        json!({"files":{"feature.txt":"bad trailing space \n"},"outcome":"completed"}).to_string(),
    )
    .unwrap();
    assert!(!cli(&["implement", id, ticket, "--fixture", "bad.json"])
        .status
        .success());
    let approved = json!({"outcome":"approved","findings":[],"evidence":"Read corrected feature"});
    let rejected = json!({"outcome":"rejected","findings":[{"code":"wrong","message":"Still incorrect","evidence":"feature.txt","required":true}],"evidence":"Observed feature"});
    let fixture = if exhausted {
        json!({"corrections":[{"files":{"feature.txt":"first\n"},"outcome":"completed"},{"files":{"feature.txt":"second\n"},"outcome":"completed"},{"files":{"feature.txt":"third\n"},"outcome":"completed"}],"reviews":[{"standards":approved,"spec":rejected},{"standards":approved,"spec":rejected},{"standards":approved,"spec":rejected}]})
    } else {
        json!({"corrections":[{"files":{"feature.txt":if stalled {"bad trailing space \n"} else {"correct\n"}},"outcome":"completed"}],"reviews":[{"standards":approved,"spec":approved}]})
    };
    fs::write(repo.join("correct.json"), fixture.to_string()).unwrap();
    let out = cli(&["correct", id, ticket, "--fixture", "correct.json"]);
    assert_eq!(
        out.status.success(),
        !stalled && !exhausted,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state: Value = serde_json::from_slice(&out.stdout).unwrap();
    let context_id = state["corrections"][0]["id"].as_str().unwrap();
    assert!(!context_id.contains('/'));
    let context = repo
        .join(".kiln/contexts")
        .join(format!("{context_id}.json"));
    let recorded: Value = serde_json::from_slice(&fs::read(&context).unwrap()).unwrap();
    assert_eq!(recorded["ticket"]["id"], ticket);
    assert_eq!(
        fs::canonicalize(context.parent().unwrap()).unwrap(),
        fs::canonicalize(repo.join(".kiln/contexts")).unwrap()
    );

    if exhausted {
        assert_eq!(state["corrections"].as_array().unwrap().len(), 3);
        assert_eq!(state["corrections"][2]["outcome"], "exhausted");
        assert_eq!(state["reviews"][2]["passed"], false);
        return;
    }
    if stalled {
        assert_eq!(state["corrections"][0]["outcome"], "no-progress");
        assert!(state["reviews"].as_array().unwrap().is_empty());
        return;
    }
    assert_eq!(state["corrections"][0]["outcome"], "approved");
    assert_eq!(state["sessions"][0]["status"], "implemented");
    assert_eq!(state["reviews"][0]["passed"], true);
}

#[test]
fn failing_implementation_is_corrected_and_freshly_reviewed() {
    scenario(false, false);
}
#[test]
fn stalled_correction_stops_without_approval() {
    scenario(true, false);
}

#[test]
fn correction_allowance_exhausts_without_false_success() {
    scenario(false, true);
}

fn scenario(stalled: bool, exhausted: bool) {
    scenario_with_ticket(stalled, exhausted, "a");
}
#[test]
fn imported_issue_identity_is_safe_for_correction_contexts() {
    scenario_with_ticket(false, false, "github:owner/repository#15");
}
#[test]
fn hostile_ticket_identity_cannot_escape_context_directory() {
    scenario_with_ticket(false, false, "../../escape");
}
#[test]
fn newer_review_findings_reopen_a_stalled_correction() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
            .status
            .success())
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    fs::write(
        repo.join("one.md"),
        "# Feature\n## Acceptance criteria\n- Feature works\n",
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), ".kiln/\n").unwrap();
    fs::write(repo.join("kiln.json"),json!({"build":["git","diff","--cached","--check"],"test":["git","diff","--cached","--check"],"startup":["git","--version"],"acceptance_criteria":["works"],"isolation":{"network":"none","runtime":"system","commands":[["git","diff","--cached","--check"],["git","--version"]]}}).to_string()).unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "initial"]);
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
    };
    let write = |name: &str, value: Value| fs::write(repo.join(name), value.to_string()).unwrap();
    let out = cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = run["id"].as_str().unwrap();
    write(
        "plan.json",
        json!({"tickets":[{"id":"a","title":"Feature","description":"Deliver feature","acceptance_criteria":["works"],"covers":["one.md#ac-1"],"blocked_by":[]}],"verification":{"outcome":"verified","findings":[]}}),
    );
    assert!(cli(&["plan", id, "--fixture", "plan.json"])
        .status
        .success());
    write(
        "impl.json",
        json!({"files":{"feature.txt":"draft\n"},"outcome":"completed"}),
    );
    assert!(cli(&["implement", id, "a", "--fixture", "impl.json"])
        .status
        .success());
    let unable = json!({"outcome":"unable-to-verify","findings":[],"evidence":""});
    let approved = json!({"outcome":"approved","findings":[],"evidence":"Read corrected feature"});
    let rejected = json!({"outcome":"rejected","findings":[{"code":"wrong","message":"Draft text","evidence":"feature.txt","required":true}],"evidence":"Observed feature"});
    write("unable.json", json!({"standards":unable,"spec":unable}));
    assert!(!cli(&["review", id, "a", "--fixture", "unable.json"])
        .status
        .success());
    write(
        "stall.json",
        json!({"corrections":[{"files":{"feature.txt":"draft\n"},"outcome":"completed"}],"reviews":[{"standards":unable,"spec":unable}]}),
    );
    let out = cli(&["correct", id, "a", "--fixture", "stall.json"]);
    assert!(!out.status.success());
    let state: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(state["corrections"][0]["outcome"], "no-progress");

    write(
        "rejected.json",
        json!({"standards":approved,"spec":rejected}),
    );
    assert!(!cli(&["review", id, "a", "--fixture", "rejected.json"])
        .status
        .success());
    write(
        "fix.json",
        json!({"corrections":[{"files":{"feature.txt":"final\n"},"outcome":"completed"}],"reviews":[{"standards":approved,"spec":approved}]}),
    );
    let out = cli(&["correct", id, "a", "--fixture", "fix.json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state: Value = serde_json::from_slice(&out.stdout).unwrap();
    let corrections = state["corrections"].as_array().unwrap();
    assert_eq!(corrections.len(), 2);
    assert_eq!(corrections[0]["outcome"], "no-progress");
    assert_eq!(corrections[1]["outcome"], "approved");
    assert!(corrections[1]["findings"]
        .to_string()
        .contains("Draft text"));
}
