//! Synchronization of verified progress back to imported GitHub issues, through the
//! CLI boundary. The Git remote is a local bare repository and GitHub is a durable
//! fixture (or a fake `gh` program for the contract checks); no test touches the
//! network or real GitHub state.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Project {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    id: String,
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
            "{args:?}: {}",
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
    fn read(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.repo.join(path)).unwrap()).unwrap()
    }
    fn implement(&self, ticket: &str, files: Value) {
        self.write(
            "implement.json",
            json!({"files":files,"outcome":"completed"}),
        );
        self.ok(&["implement", &self.id, ticket, "--fixture", "implement.json"]);
    }
    fn deliver(&self, ticket: &str, files: Value) {
        self.implement(ticket, files);
        self.ok(&["review", &self.id, ticket, "--fixture", "review.json"]);
        self.ok(&["integrate", &self.id, ticket]);
    }
    fn sync(&self) -> Output {
        self.cli(&["sync", &self.id, "--fixture", "github.json"])
    }
    fn synced(&self) -> Value {
        self.ok(&["sync", &self.id, "--fixture", "github.json"])
    }
    fn issue(&self, number: u64) -> Value {
        self.read("github.json")["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["number"] == number)
            .unwrap()
            .clone()
    }
    fn comments(&self, number: u64) -> Vec<Value> {
        self.issue(number)["comments"].as_array().unwrap().clone()
    }
    /// The single Kiln progress comment Kiln's identity wrote on an issue.
    fn progress(&self, number: u64) -> String {
        let comments: Vec<_> = self
            .comments(number)
            .into_iter()
            .filter(|c| {
                c["author"] == KILN && c["body"].as_str().unwrap().contains("kiln:progress")
            })
            .collect();
        assert_eq!(comments.len(), 1, "issue #{number}: {comments:?}");
        comments[0]["body"].as_str().unwrap().to_owned()
    }
    fn edit_github(&self, edit: impl FnOnce(&mut Value)) {
        let mut state = self.read("github.json");
        edit(&mut state);
        self.write("github.json", state);
    }
}

const CHECK: [&str; 3] = ["sh", "-c", "test -f one.txt"];
const STARTUP: [&str; 3] = ["sh", "-c", "touch .ready; exec sleep 30"];
const PROBE: [&str; 3] = ["sh", "-c", "test -f .ready"];
const FLOW: [&str; 3] = ["sh", "-c", "grep -q v2 one.txt"];
const CRITERION: &str = "Messages are sent as protocol v2";
/// The GitHub identity Kiln writes as, in the fixture and the fake `gh`.
const KILN: &str = "kiln-bot";

fn body(blocked_by: &[u64]) -> String {
    let mut body =
        format!("## Acceptance criteria\n- {CRITERION}\n## Spec coverage\n- one.md#ac-1\n");
    if !blocked_by.is_empty() {
        body.push_str("## Blocked by\n");
        for n in blocked_by {
            body.push_str(&format!("- #{n}\n"));
        }
    }
    body
}

/// A prepared run whose plan was imported from GitHub issues `(number, blocked_by)`.
fn project(issues: &[(u64, &[u64])], workflows: bool) -> Project {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let remote = temp.path().join("remote.git");
    Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&remote)
        .output()
        .unwrap();
    let mut p = Project {
        _temp: temp,
        repo,
        id: String::new(),
    };
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["config", "user.email", "test@example.com"]);
    p.git(&["config", "user.name", "Test"]);
    p.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    let file = |name: &str, content: &str| fs::write(p.repo.join(name), content).unwrap();
    file(".gitignore", ".kiln/\n*.json\n!kiln.json\n");
    file(
        "one.md",
        &format!("# Sender\n## Acceptance criteria\n- {CRITERION}\n"),
    );
    file("one.txt", "v1\n");
    p.write(
        "kiln.json",
        json!({
            "build": CHECK, "test": CHECK, "startup": STARTUP,
            "acceptance_criteria": ["Sender speaks v2"],
            "isolation": {"runtime":"system","network":"none",
                "commands":[CHECK, STARTUP, PROBE, FLOW]},
            "validation": {"startup_probe": PROBE, "timeout_ms": 3000, "workflows":
                if workflows { json!([{"criterion":"one.md#ac-1","command":FLOW}]) } else { json!([]) }},
            "publication": {"github_repository":"acme/widgets","target_branch":"main"}
        }),
    );
    p.git(&["add", "."]);
    p.git(&["commit", "-qm", "initial"]);
    p.git(&["push", "-q", "origin", "main"]);
    let prepared = p.ok(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
    p.id = prepared["id"].as_str().unwrap().to_owned();

    let url = |n: u64| format!("https://github.com/acme/widgets/issues/{n}");
    let imported: Vec<_> = issues
        .iter()
        .map(|(n, blockers)| {
            json!({"number":n,"url":url(*n),"title":format!("Issue {n}"),"body":body(blockers),
                "labels":["ready-for-agent"],"blocked_by":[]})
        })
        .collect();
    p.write("import.json", json!({ "issues": imported }));
    p.write(
        "verify.json",
        json!({"tickets":[],"verification":{"outcome":"verified","findings":[]}}),
    );
    let mut args = vec![
        "import".to_owned(),
        p.id.clone(),
        "--github-repo".into(),
        "acme/widgets".into(),
        "--fixture".into(),
        "import.json".into(),
        "--verification-fixture".into(),
        "verify.json".into(),
    ];
    for (n, _) in issues {
        args.push("--issue".into());
        args.push(n.to_string());
    }
    p.ok(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let approved = json!({"outcome":"approved","findings":[],"evidence":"Observed change"});
    p.write("review.json", json!({"standards":approved,"spec":approved}));
    let remote_issues: Vec<_> = issues
        .iter()
        .map(|(n, blockers)| {
            json!({"repository":"acme/widgets","number":n,"title":format!("Issue {n}"),
            "body":body(blockers),"comments":[
                {"id": 1000 + n, "author": "alice", "body": "Unrelated human discussion"}
            ]})
        })
        .collect();
    p.write(
        "github.json",
        json!({"pull_requests":[], "user": KILN, "issues": remote_issues}),
    );
    p
}
fn ticket(n: u64) -> String {
    format!("github:acme/widgets#{n}")
}
/// Issue 7 delivered, validated and published as a pull request; issue 8 untouched.
fn published() -> Project {
    let p = project(&[(7, &[]), (8, &[])], true);
    p.deliver(&ticket(7), json!({"one.txt":"v2\n"}));
    p.ok(&["validate", &p.id]);
    p.ok(&["publish", &p.id, "--fixture", "github.json"]);
    p
}

#[test]
fn verified_ticket_on_pull_request_reports_progress_with_evidence_and_pull_request_links() {
    let p = published();
    let run = p.synced();
    let pr = p.read("github.json")["pull_requests"][0].clone();
    let pr_url = pr["url"].as_str().unwrap();
    let commit = p.git(&["rev-parse", &format!("kiln/{}/integration", p.id)]);

    let progress = p.progress(7);
    for expected in [
        "Verified on pull request",
        pr_url,
        &format!("{pr_url}#validation-evidence"),
        &format!("https://github.com/acme/widgets/commit/{commit}"),
        "not merged into `main`",
        CRITERION,
        "verified",
        run["validation_reports"][0]["id"].as_str().unwrap(),
    ] {
        assert!(
            progress.contains(expected),
            "missing {expected:?} in\n{progress}"
        );
    }
    // Unrelated issue content is untouched.
    assert_eq!(p.comments(7)[0]["body"], "Unrelated human discussion");
    assert_eq!(p.issue(7)["body"], body(&[]));

    let record = &run["synchronization"]["issues"];
    let seven = record
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["ticket_id"] == ticket(7))
        .unwrap();
    assert_eq!(seven["state"], "verified-on-pull-request");
    assert_eq!(seven["status"], "synchronized");
    assert!(seven["comment_id"].is_u64());
    assert_eq!(
        p.ok(&["inspect", &p.id])["synchronization"],
        run["synchronization"]
    );

    let untouched = p.progress(8);
    assert!(untouched.contains("Not started"), "{untouched}");
}

#[test]
fn agent_exit_or_pushed_branch_is_not_reported_as_completed_and_blockers_are_shown() {
    let p = project(&[(7, &[]), (8, &[7])], true);
    // The agent exits successfully and its branch is pushed, but nothing is reviewed,
    // integrated or verified.
    p.implement(&ticket(7), json!({"one.txt":"v2\n"}));
    let session = p.ok(&["inspect", &p.id])["sessions"][0]["branch"]
        .as_str()
        .unwrap()
        .to_owned();
    p.git(&["push", "-q", "origin", &session]);

    let run = p.synced();
    let seven = p.progress(7);
    assert!(seven.contains("In progress"), "{seven}");
    assert!(seven.contains("Completed: no"), "{seven}");
    assert!(!seven.contains("Completed: yes"), "{seven}");
    let eight = p.progress(8);
    assert!(eight.contains("Blocked"), "{eight}");
    assert!(eight.contains(&ticket(7)), "{eight}");
    assert!(eight.contains("Completed: no"), "{eight}");
    let states: Vec<_> = run["synchronization"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["state"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(states, ["in-progress", "blocked"]);
}

#[test]
fn failed_and_unable_to_verify_validation_are_not_reported_as_completed() {
    for (workflows, files, state, headline) in [
        (
            true,
            json!({"other.txt":"x\n"}),
            "validation-failed",
            "Validation failed",
        ),
        (
            false,
            json!({"one.txt":"v2\n"}),
            "unable-to-verify",
            "Unable to verify",
        ),
    ] {
        let p = project(&[(7, &[])], workflows);
        p.deliver(&ticket(7), files);
        assert!(!p.cli(&["validate", &p.id]).status.success());
        // Push the integration branch: a pushed branch is still not completion.
        p.git(&[
            "push",
            "-q",
            "origin",
            &format!("kiln/{}/integration", p.id),
        ]);

        let run = p.synced();
        assert_eq!(run["synchronization"]["issues"][0]["state"], state);
        let progress = p.progress(7);
        assert!(progress.contains(headline), "{progress}");
        assert!(progress.contains("Completed: no"), "{progress}");
        assert!(!progress.contains("Completed: yes"), "{progress}");
        assert!(!progress.contains("Pull request:"), "{progress}");
    }
}

#[test]
fn work_on_pull_request_branch_is_distinguished_from_work_merged_into_primary_branch() {
    let p = published();
    let first = p.synced();
    assert_eq!(
        first["synchronization"]["issues"][0]["state"],
        "verified-on-pull-request"
    );
    let comment_id = first["synchronization"]["issues"][0]["comment_id"].clone();

    // The pull request is merged on the remote primary branch.
    let commit = p.git(&["rev-parse", &format!("kiln/{}/integration", p.id)]);
    p.git(&["push", "-q", "origin", &format!("{commit}:refs/heads/main")]);

    let second = p.synced();
    let seven = &second["synchronization"]["issues"][0];
    assert_eq!(seven["state"], "merged");
    assert_eq!(
        seven["comment_id"], comment_id,
        "same comment is reconciled"
    );
    let progress = p.progress(7);
    assert!(progress.contains("Merged into `main`"), "{progress}");
    assert!(progress.contains("Completed: yes"), "{progress}");
    assert!(!progress.contains("not merged"), "{progress}");
}

#[test]
fn repeated_synchronization_reconciles_instead_of_posting_duplicates() {
    let p = published();
    let first = p.synced();
    let before = p.read("github.json");
    let second = p.synced();
    assert_eq!(
        p.read("github.json"),
        before,
        "unchanged progress writes nothing"
    );
    assert_eq!(first["synchronization"], second["synchronization"]);
    assert_eq!(p.comments(7).len(), 2);
    assert_eq!(p.comments(8).len(), 2);
}

#[test]
fn interrupted_synchronization_retry_adopts_the_posted_comment() {
    for point in ["after_comment", "before_comment"] {
        let p = published();
        p.edit_github(|state| state["interrupt"] = json!(point));
        assert!(!p.sync().status.success(), "{point} must fail");
        let interrupted = p.ok(&["inspect", &p.id]);
        assert_eq!(
            interrupted["synchronization"]["issues"][0]["status"], "pending",
            "{point}"
        );

        let run = p.synced();
        let seven = &run["synchronization"]["issues"][0];
        assert_eq!(seven["status"], "synchronized");
        p.progress(7); // exactly one progress comment
        assert_eq!(p.comments(7).len(), 2, "{point}");
        let posted = p
            .comments(7)
            .into_iter()
            .find(|c| c["body"].as_str().unwrap().contains("kiln:progress"))
            .unwrap();
        assert_eq!(posted["author"], KILN);
        assert_eq!(seven["comment_id"], posted["id"]);
        assert!(run["synchronization"]["issues"][0]["divergence"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn remote_issue_edits_are_surfaced_without_changing_specs_or_issue_content() {
    let p = published();
    let edited = "## Acceptance criteria\n- Messages are sent as protocol v3\n";
    p.edit_github(|state| state["issues"][0]["body"] = json!(edited));
    let spec = fs::read_to_string(p.repo.join("one.md")).unwrap();
    let plan = p.ok(&["inspect", &p.id])["plan"].clone();

    let out = p.sync();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("divergence"), "{stderr}");
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    let seven = &run["synchronization"]["issues"][0];
    assert_eq!(seven["status"], "synchronized");
    assert!(seven["divergence"][0]
        .as_str()
        .unwrap()
        .contains("edited on GitHub since import"));
    let progress = p.progress(7);
    assert!(progress.contains("approved repository specs remain authoritative"));
    // Kiln preserves the remote edit and its own approved requirements.
    assert_eq!(p.issue(7)["body"], edited);
    assert_eq!(p.comments(7)[0]["body"], "Unrelated human discussion");
    assert_eq!(fs::read_to_string(p.repo.join("one.md")).unwrap(), spec);
    assert_eq!(run["plan"], plan);
    assert_eq!(run["specs"][0]["content"], spec);
}

#[test]
fn remotely_edited_progress_comment_is_surfaced_and_not_overwritten() {
    let p = published();
    p.synced();
    let human = "<!-- kiln:progress -->\nHuman notes replacing Kiln's progress";
    p.edit_github(|state| {
        let comments = state["issues"][0]["comments"].as_array_mut().unwrap();
        let kiln = comments
            .iter_mut()
            .find(|c| c["body"].as_str().unwrap().contains("kiln:progress"))
            .unwrap();
        kiln["body"] = json!(format!("{human}\n{}", kiln["body"].as_str().unwrap()));
    });
    let edited = p.comments(7);
    // Progress changes (merge), so Kiln would otherwise rewrite its comment.
    let commit = p.git(&["rev-parse", &format!("kiln/{}/integration", p.id)]);
    p.git(&["push", "-q", "origin", &format!("{commit}:refs/heads/main")]);

    let out = p.sync();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("conflict"), "{stderr}");
    assert!(stderr.contains("was edited on GitHub"), "{stderr}");
    assert_eq!(p.comments(7), edited, "the remote edit is preserved");
    let run = p.ok(&["inspect", &p.id]);
    assert_eq!(run["synchronization"]["issues"][0]["status"], "conflict");
    // Other issues still synchronize.
    assert_eq!(
        run["synchronization"]["issues"][1]["status"],
        "synchronized"
    );
}

/// GitHub REST contract: a fake `gh` serves `issue.json` and `comments.json` and
/// records every request, so the exact API usage is checked without network access.
struct FakeGh {
    dir: PathBuf,
    script: PathBuf,
}
impl FakeGh {
    fn new(p: &Project, post: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = p.repo.parent().unwrap().join("gh");
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("gh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\ncd '{}'\nprintf '%s\\n' \"$*\" >> calls.log\ncase \"$*\" in\n\
                 *'--method GET repos/acme/widgets/issues/7/comments'*) cat comments.json ;;\n\
                 *'--method GET repos/acme/widgets/issues/7') cat issue.json ;;\n\
                 *'--method GET user') printf '{{\"login\":\"{KILN}\"}}' ;;\n\
                 *'--method POST repos/acme/widgets/issues/7/comments --input -') cat > posted.json; {post} ;;\n\
                 *'--method PATCH repos/acme/widgets/issues/comments/'*' --input -') cat > patched.json; printf '{{}}' ;;\n\
                 *) exit 1 ;;\nesac\n",
                dir.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let gh = Self { dir, script };
        gh.serve(
            "issue.json",
            json!({"number":7,"title":"Issue 7","body":body(&[])}),
        );
        gh.serve(
            "comments.json",
            json!([{"id":11,"user":{"login":"alice"},"body":"Unrelated human discussion"}]),
        );
        gh
    }
    fn serve(&self, file: &str, value: Value) {
        fs::write(self.dir.join(file), value.to_string()).unwrap();
    }
    fn sync(&self, p: &Project) -> Output {
        p.cli(&["sync", &p.id, "--gh", self.script.to_str().unwrap()])
    }
    fn calls(&self) -> Vec<String> {
        let calls = fs::read_to_string(self.dir.join("calls.log")).unwrap_or_default();
        fs::remove_file(self.dir.join("calls.log")).ok();
        calls.lines().map(String::from).collect()
    }
    fn posted(&self) -> String {
        let posted: Value =
            serde_json::from_slice(&fs::read(self.dir.join("posted.json")).unwrap()).unwrap();
        posted["body"].as_str().unwrap().to_owned()
    }
}
const CREATED: &str = r#"printf '{"id":555,"body":"created"}'"#;
fn published_single() -> Project {
    let p = project(&[(7, &[])], true);
    p.deliver(&ticket(7), json!({"one.txt":"v2\n"}));
    p.ok(&["validate", &p.id]);
    p.ok(&["publish", &p.id, "--fixture", "github.json"]);
    p
}

#[test]
fn github_contract_synchronizes_progress_comment() {
    let p = published_single();
    let gh = FakeGh::new(&p, CREATED);
    let out = gh.sync(&p);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        gh.calls(),
        [
            "api --method GET user",
            "api --method GET repos/acme/widgets/issues/7",
            "api --method GET repos/acme/widgets/issues/7/comments?per_page=100&page=1",
            "api --method POST repos/acme/widgets/issues/7/comments --input -",
        ]
    );
    let posted = gh.posted();
    assert!(posted.contains("kiln:progress"));
    assert!(posted.contains("Verified on pull request"));
    assert!(posted.contains("https://github.com/acme/widgets/pull/1"));
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(run["synchronization"]["issues"][0]["comment_id"], 555);

    // Repeating with the comment already present writes nothing.
    gh.serve(
        "comments.json",
        json!([{"id":11,"user":{"login":"alice"},"body":"Unrelated human discussion"},{"id":555,"user":{"login":KILN},"body":posted}]),
    );
    assert!(gh.sync(&p).status.success());
    let calls = gh.calls();
    assert!(
        calls.iter().all(|c| c.contains("--method GET")),
        "{calls:?}"
    );
}

#[test]
fn github_contract_interrupted_retry_adopts_comment_created_before_failure() {
    let p = published_single();
    // GitHub stores the comment but the response is lost.
    let gh = FakeGh::new(&p, "exit 1");
    assert!(!gh.sync(&p).status.success());
    let posted = gh.posted();
    gh.calls();
    gh.serve(
        "comments.json",
        json!([{"id":11,"user":{"login":"alice"},"body":"Unrelated human discussion"},{"id":777,"user":{"login":KILN},"body":posted}]),
    );

    let out = gh.sync(&p);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = gh.calls();
    assert!(
        !calls.iter().any(|c| c.contains("POST")),
        "duplicate: {calls:?}"
    );
    assert!(!calls.iter().any(|c| c.contains("PATCH")), "{calls:?}");
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(run["synchronization"]["issues"][0]["comment_id"], 777);
    assert_eq!(
        run["synchronization"]["issues"][0]["status"],
        "synchronized"
    );
}

#[test]
fn github_contract_remote_divergence_is_surfaced_not_overwritten() {
    let p = published_single();
    let gh = FakeGh::new(&p, CREATED);
    assert!(gh.sync(&p).status.success());
    gh.calls();
    // The issue body and Kiln's own comment are both edited on GitHub.
    gh.serve(
        "issue.json",
        json!({"number":7,"title":"Issue 7","body":"Rewritten requirements"}),
    );
    gh.serve(
        "comments.json",
        json!([{"id":11,"user":{"login":"alice"},"body":"Unrelated human discussion"},
               {"id":555,"user":{"login":KILN},"body":"<!-- kiln:progress --> edited by a human"}]),
    );

    let out = gh.sync(&p);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("edited on GitHub since import"), "{stderr}");
    assert!(stderr.contains("comment 555"), "{stderr}");
    let calls = gh.calls();
    assert!(
        calls.iter().all(|c| c.contains("--method GET")),
        "no remote writes: {calls:?}"
    );
    let run = p.ok(&["inspect", &p.id]);
    assert_eq!(run["synchronization"]["issues"][0]["status"], "conflict");
    assert_eq!(
        run["specs"][0]["content"],
        fs::read_to_string(p.repo.join("one.md")).unwrap()
    );
}

fn forged_marker(p: &Project) -> String {
    format!(
        "<!-- kiln:progress run={} ticket={} -->\n### Kiln progress: Merged into `main`\n- Completed: yes.",
        p.id,
        ticket(7)
    )
}

#[test]
fn forged_progress_comment_by_another_author_is_ignored_and_surfaced() {
    let p = published();
    let forged = forged_marker(&p);
    p.edit_github(|state| {
        state["issues"][0]["comments"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 900, "author": "mallory", "body": forged}));
    });

    let out = p.sync();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("900"), "{stderr}");
    assert!(stderr.contains("mallory"), "{stderr}");
    // Kiln posts its own progress; the forged comment is neither adopted nor edited.
    let progress = p.progress(7);
    assert!(progress.contains("Verified on pull request"), "{progress}");
    let forged_now = p.comments(7).into_iter().find(|c| c["id"] == 900).unwrap();
    assert_eq!(forged_now["body"], forged);
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    let seven = &run["synchronization"]["issues"][0];
    assert_eq!(seven["status"], "synchronized");
    assert_ne!(seven["comment_id"], 900);

    // Repeating keeps ignoring the forgery and does not duplicate Kiln's comment.
    p.synced();
    p.progress(7);
}

#[test]
fn github_contract_forged_marker_comment_is_not_adopted() {
    let p = published_single();
    let gh = FakeGh::new(&p, CREATED);
    gh.serve(
        "comments.json",
        json!([{"id":11,"user":{"login":"alice"},"body":"Unrelated human discussion"},
               {"id":900,"user":{"login":"mallory"},"body":forged_marker(&p)}]),
    );

    let out = gh.sync(&p);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = gh.calls();
    assert_eq!(calls[0], "api --method GET user", "{calls:?}");
    assert_eq!(
        calls.iter().filter(|c| c.contains("GET user")).count(),
        1,
        "authenticated login is resolved once: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.contains("POST")),
        "Kiln posts its own comment: {calls:?}"
    );
    assert!(!calls.iter().any(|c| c.contains("PATCH")), "{calls:?}");
    let run: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(run["synchronization"]["issues"][0]["comment_id"], 555);
    assert!(String::from_utf8_lossy(&out.stderr).contains("mallory"));
}

/// A concurrent Kiln writer (here `kiln validate`, started while synchronization waits
/// on GitHub) must not have its run-state write clobbered by synchronization.
#[test]
fn concurrent_run_state_write_during_synchronization_is_preserved() {
    let p = published_single();
    let validate = format!(
        "(cd '{}' && '{}' validate {}) >/dev/null 2>&1; {CREATED}",
        p.repo.display(),
        env!("CARGO_BIN_EXE_kiln"),
        p.id
    );
    let gh = FakeGh::new(&p, &validate);
    let out = gh.sync(&p);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = p.ok(&["inspect", &p.id]);
    assert_eq!(
        run["validation_reports"].as_array().unwrap().len(),
        2,
        "the concurrent validation report must survive"
    );
    assert_eq!(run["synchronization"]["issues"][0]["comment_id"], 555);
}
