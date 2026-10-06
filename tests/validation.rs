//! Global validation through the CLI boundary in a temporary Git repository with
//! deterministic agent fixtures. Tickets pass in isolation; their combination
//! must still be validated against acceptance workflows derived from the specs.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Project {
    _temp: tempfile::TempDir,
    repo: PathBuf,
}
impl Project {
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(args)
            .current_dir(&self.repo)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn write(&self, path: &str, value: Value) {
        fs::write(self.repo.join(path), value.to_string()).unwrap();
    }
    fn deliver(&self, id: &str, ticket: &str, files: Value) {
        self.write(
            "implement.json",
            json!({"files":files,"outcome":"completed"}),
        );
        self.ok(&["implement", id, ticket, "--fixture", "implement.json"]);
        self.ok(&["review", id, ticket, "--fixture", "review.json"]);
        self.ok(&["integrate", id, ticket]);
    }
}

const FLOW: [&str; 2] = ["sh", "flow.sh"];
const STARTUP: [&str; 3] = ["sh", "-c", "touch .ready; exec sleep 30"];
const PROBE: [&str; 3] = ["sh", "-c", "test -f .ready"];
const NEVER_READY: [&str; 3] = ["sh", "-c", "test -f .never"];
const CHECK: [&str; 3] = ["sh", "-c", "test -f one.txt"];
const VERIFIER_TEST: [&str; 3] = ["sh", "-c", "test -f two.txt"];

fn project(probe: [&str; 3]) -> (Project, String, Value) {
    let temp = tempfile::tempdir().unwrap();
    let p = Project {
        repo: temp.path().to_path_buf(),
        _temp: temp,
    };
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["config", "user.email", "test@example.com"]);
    p.git(&["config", "user.name", "Test"]);
    let file = |name: &str, content: &str| fs::write(p.repo.join(name), content).unwrap();
    file(".gitignore", ".kiln/\n*.json\n!kiln.json\n");
    file(
        "one.md",
        "# Sender\n## Acceptance criteria\n- Messages sent by one are understood by two\n",
    );
    file(
        "two.md",
        "# Receiver\n## Acceptance criteria\n- Two accepts the protocol sent by one\n- Two keeps its own file\n",
    );
    file("one.txt", "v1\n");
    file("two.txt", "v1\n");
    // Cross-spec workflow against the started application: sender and receiver
    // must agree on a protocol version.
    file(
        "flow.sh",
        "test -f .ready && test \"$(cat one.txt)\" = \"$(cat two.txt)\"\n",
    );
    p.write(
        "kiln.json",
        json!({
            "build": CHECK, "test": CHECK, "startup": STARTUP,
            "acceptance_criteria": ["Sender and receiver work together"],
            "isolation": {"runtime":"system","network":"none",
                "commands":[CHECK, STARTUP, PROBE, NEVER_READY, FLOW, VERIFIER_TEST]},
            "validation": {"startup_probe": probe, "timeout_ms": 3000, "workflows": [
                {"criterion":"one.md#ac-1","command":FLOW},
                {"criterion":"two.md#ac-1","command":FLOW}
            ]}
        }),
    );
    p.git(&["add", "."]);
    p.git(&["commit", "-qm", "initial"]);
    let prepared = p.ok(&[
        "prepare",
        "--config",
        "kiln.json",
        "--spec",
        "one.md",
        "--spec",
        "two.md",
    ]);
    let id = prepared["id"].as_str().unwrap().to_owned();
    p.write("plan.json", json!({"tickets":[
        {"id":"a","title":"Sender v2","description":"Send v2","acceptance_criteria":["one sends v2"],"covers":["one.md#ac-1"],"blocked_by":[]},
        {"id":"b","title":"Receiver","description":"Receive","acceptance_criteria":["two keeps file"],"covers":["two.md#ac-1","two.md#ac-2"],"blocked_by":[]},
        {"id":"c","title":"Agree","description":"Receiver adopts v2","acceptance_criteria":["combined flow works"],"covers":["one.md#ac-1","two.md#ac-1"],"blocked_by":["a","b"]}
    ],"verification":{"outcome":"verified","findings":[]}}));
    p.ok(&["plan", &id, "--fixture", "plan.json"]);
    let approved = json!({"outcome":"approved","findings":[],"evidence":"Observed change"});
    p.write("review.json", json!({"standards":approved,"spec":approved}));
    (p, id, prepared)
}

fn criterion<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["criteria"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("missing criterion {id} in {report:#}"))
}

fn latest(run: &Value) -> &Value {
    run["validation_reports"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
}

#[test]
fn individually_passing_tickets_fail_combined_workflow_then_verify_after_fix() {
    let (p, id, prepared) = project(PROBE);
    p.deliver(&id, "a", json!({"one.txt":"v2\n"}));
    p.deliver(&id, "b", json!({"two.extra":"kept\n"}));
    let branch = format!("kiln/{id}/integration");
    let failing_commit = p.git(&["rev-parse", &branch]);
    let primary = p.git(&["rev-parse", "HEAD"]);
    fs::write(p.repo.join("one.md"), "dirty primary").unwrap();

    let out = p.cli(&["validate", &id]);
    assert!(!out.status.success(), "combined failure must not succeed");
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    let failed = latest(&run);
    assert_eq!(failed["outcome"], "failed");
    assert_eq!(failed["integrated_commit"], failing_commit);
    for name in ["build", "test", "startup"] {
        let check = failed["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("missing {name} check"));
        assert_eq!(
            check["passed"], true,
            "{name} passes; only the workflow fails"
        );
    }
    let one = criterion(failed, "one.md#ac-1");
    assert_eq!(one["outcome"], "failed");
    assert_eq!(one["spec_path"], "one.md");
    assert_eq!(
        one["content_sha256"],
        prepared["specs"][0]["content_sha256"]
    );
    assert_eq!(one["evidence"][0]["source"], "configured");
    assert_eq!(one["evidence"][0]["check"]["command"], json!(FLOW));
    assert_eq!(one["evidence"][0]["check"]["passed"], false);
    // No workflow covers this criterion yet: missing evidence is never a pass.
    let keep = criterion(failed, "two.md#ac-2");
    assert_eq!(keep["outcome"], "unable-to-verify");
    assert!(keep["evidence"].as_array().unwrap().is_empty());

    p.deliver(&id, "c", json!({"two.txt":"v2\n"}));
    let fixed_commit = p.git(&["rev-parse", &branch]);
    // An independent verifier contributes a test derived from the spec.
    p.write(
        "verifier.json",
        json!({"acceptance_checks":[{"criterion":"two.md#ac-2","command":VERIFIER_TEST}]}),
    );
    let run = p.ok(&["validate", &id, "--verifier", "verifier.json"]);
    let verified = latest(&run);
    assert_eq!(verified["outcome"], "verified");
    assert_eq!(verified["integrated_commit"], fixed_commit);
    let keep = criterion(verified, "two.md#ac-2");
    assert_eq!(keep["outcome"], "verified");
    assert_eq!(keep["evidence"][0]["source"], "verifier");
    assert_eq!(
        keep["content_sha256"],
        prepared["specs"][1]["content_sha256"]
    );
    assert_eq!(criterion(verified, "one.md#ac-1")["outcome"], "verified");
    assert_eq!(run["validation_reports"][0]["outcome"], "failed");

    assert_eq!(p.git(&["rev-parse", "HEAD"]), primary);
    assert_eq!(
        fs::read_to_string(p.repo.join("one.md")).unwrap(),
        "dirty primary"
    );

    let report = p.ok(&["report", &id]);
    assert_eq!(report["outcome"], "verified");
    assert_eq!(report["integrated_commit"], fixed_commit);
    assert_eq!(report["verified"].as_array().unwrap().len(), 3);
    assert!(report["failed"].as_array().unwrap().is_empty());
    assert!(report["unable_to_verify"].as_array().unwrap().is_empty());
    assert_eq!(
        report["verified"][0]["evidence"][0]["check"]["passed"],
        true
    );
}

#[test]
fn unauthorized_verifier_test_is_unable_to_verify_not_passing() {
    let (p, id, _) = project(PROBE);
    p.deliver(&id, "a", json!({"one.txt":"v2\n"}));
    p.deliver(&id, "b", json!({"two.txt":"v2\n"}));
    p.write(
        "verifier.json",
        json!({"acceptance_checks":[{"criterion":"two.md#ac-2","command":["sh","-c","exit 0"]}]}),
    );
    let out = p.cli(&["validate", &id, "--verifier", "verifier.json"]);
    assert!(!out.status.success());
    let report = p.ok(&["report", &id]);
    assert_eq!(report["outcome"], "unable-to-verify");
    assert_eq!(report["verified"].as_array().unwrap().len(), 2);
    let unable = &report["unable_to_verify"][0];
    assert_eq!(unable["id"], "two.md#ac-2");
    assert!(unable["evidence"][0]["check"]["stderr"]
        .as_str()
        .unwrap()
        .contains("unauthorized"));
}

#[test]
fn verifier_test_for_unknown_criterion_is_rejected() {
    let (p, id, _) = project(PROBE);
    p.write(
        "verifier.json",
        json!({"acceptance_checks":[{"criterion":"two.md#ac-9","command":VERIFIER_TEST}]}),
    );
    let out = p.cli(&["validate", &id, "--verifier", "verifier.json"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("two.md#ac-9"));
}

#[test]
fn startup_that_never_becomes_ready_fails_and_leaves_workflows_unverified() {
    let (p, id, _) = project(NEVER_READY);
    p.deliver(&id, "a", json!({"one.txt":"v2\n"}));
    let out = p.cli(&["validate", &id]);
    assert!(!out.status.success());
    let report = p.ok(&["report", &id]);
    assert_eq!(report["outcome"], "failed");
    assert_eq!(report["unable_to_verify"].as_array().unwrap().len(), 3);
}

#[test]
fn validation_without_integrated_revision_is_unable_to_verify() {
    let (p, id, _) = project(PROBE);
    let out = p.cli(&["validate", &id]);
    assert!(!out.status.success());
    let report = p.ok(&["report", &id]);
    assert_eq!(report["outcome"], "unable-to-verify");
    assert!(report["integrated_commit"].is_null());
    assert_eq!(report["unable_to_verify"].as_array().unwrap().len(), 3);
}

#[test]
fn report_without_validation_is_an_error() {
    let (p, id, _) = project(PROBE);
    let out = p.cli(&["report", &id]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("validate"));
}
