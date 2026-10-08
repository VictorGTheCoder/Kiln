//! Out-of-process pause and cancellation requests for an owned scheduler run.
//! The small sidecar lets control commands communicate without taking the run's
//! scheduler lock or racing its durable state writes.
use crate::Engine;
use anyhow::{bail, Context, Result};
use std::{fs, io::Write, path::PathBuf};

/// Error of `kiln resume` (and the dashboard Resume) when nothing can continue.
pub const NOTHING_TO_RESUME: &str =
    "no paused or interrupted run to resume; start one with `kiln start`";

fn path(engine: &Engine, id: &str) -> PathBuf {
    engine
        .repository
        .join(".kiln/runs")
        .join(format!("{id}.control"))
}

impl Engine {
    /// Request a graceful pause or an immediate cancellation of an active run.
    pub fn request_control(&self, id: &str, action: &str) -> Result<()> {
        if !matches!(action, "pause" | "cancel") {
            bail!("control action must be pause or cancel");
        }
        let run = self.inspect(id)?;
        if run.status != "running" {
            bail!("run '{id}' is not active (status: {})", run.status);
        }
        if !self.is_owned_elsewhere(id)? {
            bail!("run '{id}' has no active scheduler process");
        }
        let target = path(self, id);
        let mut staging =
            tempfile::NamedTempFile::new_in(target.parent().context("run directory")?)?;
        staging.write_all(action.as_bytes())?;
        staging.as_file().sync_all()?;
        staging.persist(&target).map_err(|e| e.error)?;
        fs::File::open(target.parent().unwrap())?.sync_all()?;
        if self.inspect(id)?.status != "running" {
            self.clear_control(id)?;
            bail!("run '{id}' finished before the control request was applied");
        }
        Ok(())
    }

    /// The most recently created run matching `keep`.
    fn latest_run_where(
        &self,
        keep: impl Fn(&crate::Run) -> Result<bool>,
    ) -> Result<Option<crate::Run>> {
        let mut latest: Option<crate::Run> = None;
        for id in self.list()? {
            let run = self.inspect(&id)?;
            if latest
                .as_ref()
                .is_none_or(|old| old.created_unix_ms < run.created_unix_ms)
                && keep(&run)?
            {
                latest = Some(run);
            }
        }
        Ok(latest)
    }

    /// Id of the latest run whose scheduler is live in another process: the
    /// target of `kiln pause` and `kiln cancel` without an id.
    pub fn latest_active_run(&self) -> Result<Option<String>> {
        Ok(self
            .latest_run_where(|run| {
                Ok(run.status == "running" && self.is_owned_elsewhere(&run.id)?)
            })?
            .map(|run| run.id))
    }

    /// The latest backlog run that can continue: paused, stopped by a limit, or
    /// left `running` by a process that is gone. The target of `kiln resume`.
    pub fn latest_resumable_run(&self) -> Result<Option<crate::Run>> {
        self.latest_run_where(|run| {
            let backlog = run
                .backlog
                .as_ref()
                .is_some_and(|backlog| backlog.mode == "issue-graph");
            Ok(backlog
                && match run.status.as_str() {
                    "paused" | "limit_exhausted" => true,
                    "running" => !self.is_owned_elsewhere(&run.id)?,
                    _ => false,
                })
        })
    }

    pub(crate) fn requested_control(&self, id: &str) -> Result<Option<String>> {
        let target = path(self, id);
        match fs::read_to_string(target) {
            Ok(action) if matches!(action.trim(), "pause" | "cancel") => {
                Ok(Some(action.trim().into()))
            }
            Ok(_) => bail!("run control request is invalid"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn clear_control(&self, id: &str) -> Result<()> {
        match fs::remove_file(path(self, id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("clear run control request"),
        }
    }
}
