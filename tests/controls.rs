use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

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
        repo.git(&["config", "commit.gpgsign", "false"]);
        fs::write(
            repo.path.join("one.md"),
            "# One\n## Acceptance criteria\n- Works\n",
        )
        .unwrap();
        fs::write(repo.path.join(".gitignore"), ".kiln/\n").unwrap();
        fs::write(repo.path.join("kiln.json"), json!({
            "build":["git","diff","--check"],"test":["git","diff","--check"],
            "startup":["git","--version"],"acceptance_criteria":["works"],
            "isolation":{"network":"none","runtime":"system","commands":[["git","diff","--check"],["git","--version"]]}
        }).to_string()).unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
        let out = self.cli(&["prepare", "--config", "kiln.json", "--spec", "one.md"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let prepared: Value = serde_json::from_slice(&out.stdout).unwrap();
        let id = prepared["id"].as_str().unwrap().to_owned();
        fs::write(
            self.path.join("plan.json"),
            json!({"tickets":[{
            "id":"a","title":"a","description":"Deliver","acceptance_criteria":["works"],
            "covers":["one.md#ac-1"],"blocked_by":[]
        }],"verification":{"outcome":"verified","findings":[]}})
            .to_string(),
        )
        .unwrap();
        let out = self.cli(&["plan", &id, "--fixture", "plan.json"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        id
    }
    fn start_waiting_run(&self, id: &str) -> Child {
        fs::write(
            self.path.join("scenario.json"),
            json!({"tickets":{"a":{
                "implementation":{"files":{"a.txt":"works"},"outcome":"completed"},
                "review":{"standards":{"outcome":"approved","findings":[],"evidence":"ok"},
                          "spec":{"outcome":"approved","findings":[],"evidence":"ok"}},
                "await_file":".kiln/release"
            }}})
            .to_string(),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(["run", id, "--fixture", "scenario.json"])
            .current_dir(&self.path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
    fn wait_for_run(&self, id: &str, status: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let out = self.cli(&["inspect", id]);
            if out.status.success() {
                let value: Value = serde_json::from_slice(&out.stdout).unwrap();
                if value["status"] == status {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(30));
        }
        panic!("run {id} did not reach {status}");
    }
}

#[test]
fn pause_settles_active_work_and_resume_keeps_completed_work() {
    let repo = Repo::new();
    let id = repo.planned();
    let child = repo.start_waiting_run(&id);
    repo.wait_for_run(&id, "running");
    let paused = repo.cli(&["pause", &id]);
    assert!(
        paused.status.success(),
        "{}",
        String::from_utf8_lossy(&paused.stderr)
    );
    fs::write(repo.path.join(".kiln/release"), "go").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run: Value = serde_json::from_slice(
        &fs::read(repo.path.join(".kiln/runs").join(format!("{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(run["status"], "paused");
    assert_eq!(run["sessions"].as_array().unwrap().len(), 1);

    let resumed = repo.cli(&["resume", &id, "--fixture", "scenario.json"]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let run: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(run["status"], "awaiting_validation");
    assert_eq!(run["sessions"].as_array().unwrap().len(), 1);
}

#[test]
fn cancel_stops_active_work_and_prevents_new_issue_starts() {
    let repo = Repo::new();
    let id = repo.planned();
    let child = repo.start_waiting_run(&id);
    repo.wait_for_run(&id, "running");
    let cancelled = repo.cli(&["cancel", &id]);
    assert!(
        cancelled.status.success(),
        "{}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let inspect = repo.cli(&["inspect", &id]);
    let run: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(run["status"], "cancelled");
    assert_eq!(run["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(run["scheduler"]["tickets"][0]["state"], "stopped");
    let resume = repo.cli(&["resume", &id, "--fixture", "scenario.json"]);
    assert!(
        !resume.status.success(),
        "cancelled work must not silently restart"
    );
}

#[test]
fn probing_for_an_owner_never_makes_taking_ownership_fail() {
    let repo = Repo::new();
    let id = repo.planned();
    let engine = kiln::Engine::open(&repo.path).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let prober = {
        let engine = kiln::Engine::open(&repo.path).unwrap();
        let (id, stop) = (id.clone(), stop.clone());
        thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                engine.is_owned_elsewhere(&id).unwrap();
            }
        })
    };
    let mut failures = 0;
    for _ in 0..3000 {
        match engine.own_run(&id) {
            Ok(owner) => drop(owner),
            Err(_) => failures += 1,
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    prober.join().unwrap();
    assert_eq!(
        failures, 0,
        "a status probe made a scheduler fail to own its run"
    );
}
