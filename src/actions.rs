//! Dashboard actions: Plan, Start, Pause, Resume and Cancel from the browser.
//!
//! Plan, Start and Resume launch the same `kiln plan`, `kiln start` and
//! `kiln resume` commands an operator would type, with no flags, so they use
//! the same default inference (repository, config, GitHub repository,
//! provider) and the same plan reuse. Inference is checked before launching so
//! a missing input is reported at once with the CLI's message; a later failure
//! of the launched command is reported with the error it printed. Pause and
//! Cancel request control of a run exactly like `kiln pause` / `kiln cancel`.
use crate::{defaults, Engine, Run};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex, PoisonError},
    thread,
};

/// Stand-ins for the provider and GitHub handed to every launched command
/// (hidden test flags of `kiln dashboard`); empty in normal use.
#[derive(Debug, Clone, Default)]
pub struct Fixtures {
    pub issue_fixture: Option<PathBuf>,
    pub planning_fixture: Option<PathBuf>,
    pub run_fixture: Option<PathBuf>,
    pub publication_fixture: Option<PathBuf>,
    pub gh: Option<PathBuf>,
    pub repair_fixture: Option<PathBuf>,
}

/// The latest launched action and how it ended.
#[derive(Debug, Clone, Serialize)]
pub struct LastAction {
    pub action: String,
    pub run: Option<String>,
    /// `running`, `succeeded` or `failed`.
    pub state: String,
    /// The error the command reported, as the CLI prints it (without `Kiln: `).
    pub error: Option<String>,
}

/// Launches and tracks dashboard actions for one repository.
pub struct Actions {
    engine: Engine,
    fixtures: Fixtures,
    last: Arc<Mutex<Option<LastAction>>>,
}

/// Outcome of a POST action: HTTP status and JSON body.
pub type Reply = (u16, Value);

impl Actions {
    pub fn new(engine: Engine, fixtures: Fixtures) -> Self {
        Self {
            engine,
            fixtures,
            last: Arc::new(Mutex::new(None)),
        }
    }

    fn busy(&self) -> bool {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|last| last.state == "running")
    }

    /// Actions that make sense now: `available` for the repository and `runs`
    /// per run id, plus the `last` launched action.
    pub fn offered(&self) -> Result<Value> {
        let busy = self.busy();
        let mut runs = BTreeMap::new();
        let mut any_live = false;
        let mut resumable: Option<&Run> = None;
        let recorded: Vec<Run> = self
            .engine
            .list()?
            .iter()
            .filter_map(|id| self.engine.inspect(id).ok())
            .collect();
        for run in &recorded {
            let live = run.status == "running" && crate::web::is_live(&self.engine, &run.id);
            any_live |= live;
            let mut offered: Vec<&str> = Vec::new();
            if live {
                offered.extend(["pause", "cancel"]);
            }
            // The same candidates as `kiln resume`; the latest one is offered.
            let candidate = run
                .backlog
                .as_ref()
                .is_some_and(|b| b.mode == "issue-graph")
                && match run.status.as_str() {
                    "paused" | "limit_exhausted" => true,
                    "running" => !live,
                    _ => false,
                };
            if candidate && resumable.is_none_or(|old| old.created_unix_ms < run.created_unix_ms) {
                resumable = Some(run);
            }
            runs.insert(run.id.clone(), offered);
        }
        if let Some(run) = resumable.filter(|_| !busy) {
            runs.entry(run.id.clone()).or_default().push("resume");
        }
        let available: Vec<&str> = if busy || any_live {
            Vec::new()
        } else {
            vec!["plan", "start"]
        };
        let last = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Ok(json!({"available": available, "runs": runs, "last": last}))
    }

    /// Perform the action at `path` (`/api/dashboard/actions/{plan,start}` or
    /// `/api/dashboard/runs/<id>/{pause,resume,cancel}`); `None` for other paths.
    pub fn perform(&self, path: &str) -> Option<Reply> {
        let result = if let Some(action) = path.strip_prefix("/api/dashboard/actions/") {
            match action {
                "plan" => self.launch_backlog("plan"),
                "start" => self.launch_backlog("start"),
                _ => return None,
            }
        } else {
            let rest = path.strip_prefix("/api/dashboard/runs/")?;
            let (id, action) = rest.split_once('/')?;
            if !matches!(action, "pause" | "cancel" | "resume") {
                return None;
            }
            crate::web::validate_run_id(id).and_then(|()| match action {
                "resume" => self.resume(id),
                _ => self.control(id, action),
            })
        };
        Some(match result {
            Ok(reply) => reply,
            Err(error) => (409, json!({"error": format!("{error:#}")})),
        })
    }

    fn control(&self, id: &str, action: &str) -> Result<Reply> {
        self.engine.request_control(id, action)?;
        Ok((
            200,
            json!({"action": action, "run": id, "state": "requested"}),
        ))
    }

    /// Plan or Start with inferred defaults, as `kiln plan` / `kiln start`.
    fn launch_backlog(&self, action: &'static str) -> Result<Reply> {
        let f = &self.fixtures;
        let inferred = defaults::Defaults::infer(&self.engine.repository, Default::default())?;
        // Fixtures stand in for every agent call exactly when the CLI skips the provider.
        let needs_provider = match action {
            "plan" => f.planning_fixture.is_none(),
            _ => {
                f.planning_fixture.is_none()
                    || f.run_fixture.is_none()
                    || f.repair_fixture.is_none()
            }
        };
        if needs_provider {
            inferred.provider()?;
        }
        let mut args: Vec<OsString> = vec![action.into()];
        if action == "start" {
            // This server already shows the run; the command must not start another.
            args.push("--no-dashboard".into());
        }
        push(&mut args, "--issue-fixture", &f.issue_fixture);
        push(&mut args, "--planning-fixture", &f.planning_fixture);
        if action == "start" {
            push(&mut args, "--run-fixture", &f.run_fixture);
            push(&mut args, "--publication-fixture", &f.publication_fixture);
            push(&mut args, "--gh", &f.gh);
            push(&mut args, "--repair-fixture", &f.repair_fixture);
        }
        self.launch(action, None, args)
    }

    /// Resume `id` as `kiln resume`, which continues the latest resumable run.
    fn resume(&self, id: &str) -> Result<Reply> {
        let f = &self.fixtures;
        let run = self
            .engine
            .latest_resumable_run()?
            .context(crate::control::NOTHING_TO_RESUME)?;
        if run.id != id {
            bail!(
                "run '{id}' cannot be resumed; the latest paused or interrupted run is {}",
                run.id
            );
        }
        if f.run_fixture.is_none() || f.repair_fixture.is_none() {
            let inferred = defaults::Defaults::infer(
                &self.engine.repository,
                defaults::Overrides {
                    github_repo: run.backlog.as_ref().map(|b| b.github_repository.clone()),
                    ..Default::default()
                },
            )?;
            inferred.provider()?;
        }
        // This server already shows the run; the command must not start another.
        let mut args: Vec<OsString> = vec!["resume".into(), "--no-dashboard".into()];
        push(&mut args, "--fixture", &f.run_fixture);
        push(&mut args, "--publication-fixture", &f.publication_fixture);
        push(&mut args, "--gh", &f.gh);
        push(&mut args, "--repair-fixture", &f.repair_fixture);
        self.launch("resume", Some(id.to_owned()), args)
    }

    /// Run `kiln <args>` in the background, one action at a time, and record
    /// how it ends.
    fn launch(&self, action: &str, run: Option<String>, args: Vec<OsString>) -> Result<Reply> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if last.as_ref().is_some_and(|l| l.state == "running") {
            bail!("another dashboard action is still running; wait for it to finish");
        }
        let log_path = self.engine.repository.join(".kiln/dashboard-action.log");
        fs::create_dir_all(log_path.parent().expect("log lives under .kiln"))?;
        let log = fs::File::create(&log_path)?;
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--repo")
            .arg(&self.engine.repository)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .with_context(|| format!("cannot launch kiln {action}"))?;
        *last = Some(LastAction {
            action: action.to_owned(),
            run: run.clone(),
            state: "running".into(),
            error: None,
        });
        let shared = Arc::clone(&self.last);
        let action = action.to_owned();
        thread::spawn(move || {
            let succeeded = child.wait().is_ok_and(|status| status.success());
            let error = (!succeeded)
                .then(|| reported_error(&fs::read_to_string(&log_path).unwrap_or_default()));
            *shared.lock().unwrap_or_else(PoisonError::into_inner) = Some(LastAction {
                action,
                run,
                state: if succeeded { "succeeded" } else { "failed" }.into(),
                error,
            });
        });
        Ok((
            202,
            json!({"action": last.as_ref().unwrap().action, "run": last.as_ref().unwrap().run, "state": "running"}),
        ))
    }
}

fn push(args: &mut Vec<OsString>, flag: &str, value: &Option<PathBuf>) {
    if let Some(value) = value {
        args.push(flag.into());
        args.push(value.into());
    }
}

/// The error a failed command printed (`Kiln: <error>`), else its last output.
fn reported_error(stderr: &str) -> String {
    match stderr.rfind("Kiln: ") {
        Some(at) if at == 0 || stderr[..at].ends_with('\n') => stderr[at + 6..].trim().to_owned(),
        _ => match stderr.trim().lines().last() {
            Some(line) => line.to_owned(),
            None => "the command failed without an error message".to_owned(),
        },
    }
}
