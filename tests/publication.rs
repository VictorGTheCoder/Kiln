//! Pull request publication through the CLI boundary. The Git remote is a local
//! bare repository and GitHub is a durable fixture (or a fake `gh` program for the
//! contract checks), so no test touches the network or real GitHub state.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Project {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    remote: PathBuf,
}
impl Project {
    fn git_in(&self, dir: &PathBuf, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.repo, args)
    }
    fn remote_ref(&self, branch: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
            .current_dir(&self.remote)
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
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
    fn read(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.repo.join(path)).unwrap()).unwrap()
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
    fn publish(&self, id: &str) -> Output {
        self.cli(&["publish", id, "--fixture", "github.json"])
    }
    fn pull_requests(&self) -> Vec<Value> {
        self.read("github.json")["pull_requests"]
            .as_array()
            .unwrap()
            .clone()
    }
}

const CHECK: [&str; 3] = ["sh", "-c", "test -f one.txt"];
const STARTUP: [&str; 3] = ["sh", "-c", "touch .ready; exec sleep 30"];
const PROBE: [&str; 3] = ["sh", "-c", "test -f .ready"];
const FLOW: [&str; 3] = ["sh", "-c", "grep -q v2 one.txt"];

fn project(publication: Value) -> (Project, String) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    let p = Project {
        repo,
        remote,
        _temp: temp,
    };
    p.git_in(
        &temp_parent(&p),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["config", "user.email", "test@example.com"]);
    p.git(&["config", "user.name", "Test"]);
    p.git(&["remote", "add", "origin", p.remote.to_str().unwrap()]);
    let file = |name: &str, content: &str| fs::write(p.repo.join(name), content).unwrap();
    file(".gitignore", ".kiln/\n*.json\n!kiln.json\n");
    file(
        "one.md",
        "# Sender\n## Acceptance criteria\n- Messages are sent as protocol v2\n",
    );
    file("one.txt", "v1\n");
    let mut config = json!({
        "build": CHECK, "test": CHECK, "startup": STARTUP,
        "acceptance_criteria": ["Sender speaks v2"],
        "isolation": {"runtime":"system","network":"none",
            "commands":[CHECK, STARTUP, PROBE, FLOW]},
        "validation": {"startup_probe": PROBE, "timeout_ms": 3000, "workflows": [
            {"criterion":"one.md#ac-1","command":FLOW}
        ]}
    });
    if !publication.is_null() {
        config["publication"] = publication;
    }
    p.write("kiln.json", config);
    p.git(&["add", "."]);
    p.git(&["commit", "-qm", "initial"]);
    p.git(&["push", "-q", "origin", "main"]);
    let prepared = p.ok(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    let id = prepared["id"].as_str().unwrap().to_owned();
    p.write("plan.json", json!({"tickets":[
        {"id":"a","title":"Send protocol v2","description":"Send v2","acceptance_criteria":["one sends v2"],"covers":["one.md#ac-1"],"blocked_by":[]}
    ],"verification":{"outcome":"verified","findings":[]}}));
    p.ok(&["plan", &id, "--fixture", "plan.json"]);
    let approved = json!({"outcome":"approved","findings":[],"evidence":"Observed change"});
    p.write("review.json", json!({"standards":approved,"spec":approved}));
    p.write("github.json", json!({"pull_requests":[]}));
    (p, id)
}
fn temp_parent(p: &Project) -> PathBuf {
    p.repo.parent().unwrap().to_path_buf()
}
fn default_policy() -> Value {
    json!({"github_repository":"acme/widgets","target_branch":"main"})
}
fn verified(publication: Value) -> (Project, String, String) {
    let (p, id) = project(publication);
    p.deliver(&id, "a", json!({"one.txt":"v2\n"}));
    p.ok(&["validate", &id]);
    let commit = p.git(&["rev-parse", &format!("kiln/{id}/integration")]);
    (p, id, commit)
}

#[test]
fn verified_run_pushes_integration_branch_and_opens_described_pull_request() {
    let (p, id, commit) = verified(default_policy());
    let branch = format!("kiln/{id}/integration");
    let primary = p.git(&["rev-parse", "HEAD"]);

    let run = p.ok(&["publish", &id, "--fixture", "github.json"]);

    assert_eq!(p.remote_ref(&branch).as_deref(), Some(commit.as_str()));
    assert_eq!(p.remote_ref("main").as_deref(), Some(primary.as_str()));
    let prs = p.pull_requests();
    assert_eq!(prs.len(), 1);
    let pr = &prs[0];
    assert_eq!(pr["head"], branch);
    assert_eq!(pr["base"], "main");
    assert_eq!(pr["repository"], "acme/widgets");
    let body = pr["body"].as_str().unwrap();
    for expected in [
        "## Delivered behavior",
        "Send protocol v2",
        "## Specifications",
        "one.md",
        "## Validation evidence",
        "Messages are sent as protocol v2",
        "verified",
        "grep -q v2 one.txt",
        "## Limitations",
        commit.as_str(),
    ] {
        assert!(body.contains(expected), "missing {expected:?} in\n{body}");
    }

    let publication = &run["publication"];
    assert_eq!(publication["status"], "published");
    assert_eq!(publication["remote"], "origin");
    assert_eq!(publication["branch"], branch);
    assert_eq!(publication["target_branch"], "main");
    assert_eq!(publication["published_commit"], commit);
    assert_eq!(publication["pull_request"]["number"], pr["number"]);
    assert_eq!(publication["pull_request"]["url"], pr["url"]);
    assert_eq!(
        publication["validation_report"],
        run["validation_reports"][0]["id"]
    );
    // PR creation never implies merging or deployment authority.
    assert_eq!(publication["merge_authorized"], false);
    assert_eq!(publication["deploy_authorized"], false);
    assert!(body.contains("not authorized"));
    // Durable: inspect returns the same recorded identity.
    assert_eq!(p.ok(&["inspect", &id])["publication"], *publication);
}

#[test]
fn failed_or_unverified_delivery_is_never_published() {
    let (p, id) = project(default_policy());
    let branch = format!("kiln/{id}/integration");
    // Without any validation report.
    let out = p.publish(&id);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("verified"));

    // Integrated but the acceptance workflow fails: one.txt still says v1.
    p.deliver(&id, "a", json!({"other.txt":"x\n"}));
    assert!(!p.cli(&["validate", &id]).status.success());
    let out = p.publish(&id);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("failed"));

    assert_eq!(p.remote_ref(&branch), None);
    assert!(p.pull_requests().is_empty());
    assert!(p.ok(&["inspect", &id])["publication"].is_null());
}

#[test]
fn integration_advanced_after_verification_requires_revalidation() {
    let (p, id, _) = verified(default_policy());
    // Advance the branch directly to simulate a later, unvalidated integration.
    let branch = format!("kiln/{id}/integration");
    p.git(&["checkout", "-q", &branch]);
    fs::write(p.repo.join("late.txt"), "late\n").unwrap();
    p.git(&["add", "late.txt"]);
    p.git(&["commit", "-qm", "late unverified change"]);
    p.git(&["checkout", "-q", "main"]);

    let out = p.publish(&id);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("validate"));
    assert_eq!(p.remote_ref(&branch), None);
    assert!(p.pull_requests().is_empty());
}

#[test]
fn repeated_publication_does_not_repeat_completed_effects() {
    let (p, id, _) = verified(default_policy());
    let first = p.ok(&["publish", &id, "--fixture", "github.json"]);
    // A completed publication must not consult GitHub again: a broken fixture proves it.
    let state = p.read("github.json");
    fs::write(p.repo.join("github.json"), "not json").unwrap();
    let second = p.ok(&["publish", &id, "--fixture", "github.json"]);
    assert_eq!(first["publication"], second["publication"]);
    fs::write(p.repo.join("github.json"), state.to_string()).unwrap();
    assert_eq!(p.pull_requests().len(), 1);
}

#[test]
fn interruption_after_pull_request_creation_reconciles_without_duplicate() {
    let (p, id, commit) = verified(default_policy());
    let branch = format!("kiln/{id}/integration");
    p.write(
        "github.json",
        json!({"pull_requests":[], "interrupt":"after_create"}),
    );
    let out = p.publish(&id);
    assert!(!out.status.success(), "interrupted publication must fail");
    let interrupted = p.ok(&["inspect", &id]);
    assert_eq!(interrupted["publication"]["status"], "pushed");
    assert_eq!(interrupted["publication"]["branch"], branch);
    assert!(interrupted["publication"]["pull_request"].is_null());
    assert_eq!(p.pull_requests().len(), 1, "the remote PR exists");

    let run = p.ok(&["publish", &id, "--fixture", "github.json"]);
    let prs = p.pull_requests();
    assert_eq!(prs.len(), 1, "retry must not duplicate the pull request");
    assert_eq!(run["publication"]["status"], "published");
    assert_eq!(run["publication"]["reconciled"], true);
    assert_eq!(
        run["publication"]["pull_request"]["number"],
        prs[0]["number"]
    );
    assert_eq!(p.remote_ref(&branch).as_deref(), Some(commit.as_str()));
}

#[test]
fn interruption_before_pull_request_creation_resumes_after_push() {
    let (p, id, commit) = verified(default_policy());
    let branch = format!("kiln/{id}/integration");
    p.write(
        "github.json",
        json!({"pull_requests":[], "interrupt":"before_create"}),
    );
    assert!(!p.publish(&id).status.success());
    assert_eq!(p.remote_ref(&branch).as_deref(), Some(commit.as_str()));
    assert!(p.pull_requests().is_empty());
    assert_eq!(p.ok(&["inspect", &id])["publication"]["status"], "pushed");

    let run = p.ok(&["publish", &id, "--fixture", "github.json"]);
    assert_eq!(p.pull_requests().len(), 1);
    assert_eq!(run["publication"]["status"], "published");
    assert_eq!(run["publication"]["reconciled"], false);
}

#[test]
fn existing_open_pull_request_is_adopted_and_its_description_refreshed() {
    let (p, id, _) = verified(default_policy());
    let branch = format!("kiln/{id}/integration");
    p.write(
        "github.json",
        json!({"pull_requests":[
            {"repository":"acme/widgets","number":41,"url":"https://github.com/acme/widgets/pull/41",
             "head":"unrelated","base":"main","title":"Other","body":"other"},
            {"repository":"acme/widgets","number":42,"url":"https://github.com/acme/widgets/pull/42",
             "head":branch,"base":"main","title":"Stale","body":"stale"}
        ]}),
    );
    let run = p.ok(&["publish", &id, "--fixture", "github.json"]);
    let prs = p.pull_requests();
    assert_eq!(prs.len(), 2);
    assert_eq!(run["publication"]["pull_request"]["number"], 42);
    assert_eq!(run["publication"]["reconciled"], true);
    assert!(prs[1]["body"]
        .as_str()
        .unwrap()
        .contains("## Validation evidence"));
    assert_eq!(prs[0]["body"], "other");
}

#[test]
fn merge_and_deploy_authority_requires_explicit_configuration() {
    let mut policy = default_policy();
    policy["merge"] = json!(true);
    policy["deploy"] = json!(true);
    let (p, id, _) = verified(policy);
    let run = p.ok(&["publish", &id, "--fixture", "github.json"]);
    assert_eq!(run["publication"]["merge_authorized"], true);
    assert_eq!(run["publication"]["deploy_authorized"], true);
    let body = p.pull_requests()[0]["body"].as_str().unwrap().to_owned();
    assert!(body.contains("explicitly authorized by project configuration"));
}

#[test]
fn malformed_publication_policy_is_rejected_at_preparation() {
    for policy in [
        json!({"github_repository":"acme/widgets","target_branch":"main","merge":"yes"}),
        json!({"github_repository":"acme/widgets","target_branch":"main","auto_merge":true}),
        json!({"github_repository":"widgets","target_branch":"main"}),
        json!({"github_repository":"acme/widgets","target_branch":"-main"}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(repo)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        fs::write(
            repo.join("one.md"),
            "# A\n## Acceptance criteria\n- works\n",
        )
        .unwrap();
        fs::write(
            repo.join("kiln.json"),
            json!({"build":CHECK,"test":CHECK,"startup":CHECK,"acceptance_criteria":["x"],
                "isolation":{"runtime":"system","network":"none","commands":[CHECK]},
                "publication":policy})
            .to_string(),
        )
        .unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(["prepare", "--config", "kiln.json", "--spec", "one.md"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{policy} must be rejected");
        assert!(String::from_utf8_lossy(&out.stderr).contains("publication"));
    }
}

#[test]
fn unconfigured_publication_is_refused() {
    let (p, id, _) = verified(Value::Null);
    let out = p.publish(&id);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("publication"));
}

/// GitHub REST contract: a fake `gh` records requests so the exact API usage is
/// checked without network access.
#[test]
fn github_contract_lists_open_pull_requests_then_creates_one() {
    let (p, id, _) = verified(default_policy());
    let branch = format!("kiln/{id}/integration");
    let dir = p.repo.parent().unwrap().join("gh");
    fs::create_dir_all(&dir).unwrap();
    let script = dir.join("gh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncd '{d}'\nprintf '%s\\n' \"$*\" >> calls.log\ncase \"$*\" in\n*'--method GET'*) printf '[]' ;;\n*'--method POST'*) cat > create.json; printf '{{\"number\":7,\"html_url\":\"https://github.com/acme/widgets/pull/7\",\"head\":{{\"ref\":\"{b}\"}},\"base\":{{\"ref\":\"main\"}}}}' ;;\n*) exit 1 ;;\nesac\n",
            d = dir.display(),
            b = branch
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let run = p.ok(&["publish", &id, "--gh", script.to_str().unwrap()]);
    assert_eq!(run["publication"]["pull_request"]["number"], 7);
    assert_eq!(
        run["publication"]["pull_request"]["url"],
        "https://github.com/acme/widgets/pull/7"
    );
    let calls = fs::read_to_string(dir.join("calls.log")).unwrap();
    let calls: Vec<_> = calls.lines().collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[0],
        format!(
            "api --method GET repos/acme/widgets/pulls -f state=open -f head=acme:{branch} -f base=main"
        )
    );
    assert_eq!(
        calls[1],
        "api --method POST repos/acme/widgets/pulls --input -"
    );
    let request: Value =
        serde_json::from_slice(&fs::read(dir.join("create.json")).unwrap()).unwrap();
    assert_eq!(request["head"], branch);
    assert_eq!(request["base"], "main");
    assert!(request["body"]
        .as_str()
        .unwrap()
        .contains("## Validation evidence"));
}

#[test]
fn github_contract_reconciles_listed_pull_request_with_update() {
    let (p, id, _) = verified(default_policy());
    let branch = format!("kiln/{id}/integration");
    let dir = p.repo.parent().unwrap().join("gh");
    fs::create_dir_all(&dir).unwrap();
    let script = dir.join("gh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncd '{d}'\nprintf '%s\\n' \"$*\" >> calls.log\ncase \"$*\" in\n*'--method GET'*) printf '[{{\"number\":9,\"html_url\":\"https://github.com/acme/widgets/pull/9\",\"head\":{{\"ref\":\"{b}\"}},\"base\":{{\"ref\":\"main\"}},\"body\":\"old\"}}]' ;;\n*'--method PATCH'*) cat > update.json; printf '{{}}' ;;\n*) exit 1 ;;\nesac\n",
            d = dir.display(),
            b = branch
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let run = p.ok(&["publish", &id, "--gh", script.to_str().unwrap()]);
    assert_eq!(run["publication"]["pull_request"]["number"], 9);
    assert_eq!(run["publication"]["reconciled"], true);
    let calls = fs::read_to_string(dir.join("calls.log")).unwrap();
    assert!(!calls.contains("POST"), "no duplicate PR: {calls}");
    assert!(calls.contains("api --method PATCH repos/acme/widgets/pulls/9 --input -"));
}
