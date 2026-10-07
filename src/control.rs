//! Out-of-process pause and cancellation requests for an owned scheduler run.
//! The small sidecar lets control commands communicate without taking the run's
//! scheduler lock or racing its durable state writes.
use crate::Engine;
use anyhow::{bail, Context, Result};
use std::{fs, io::Write, path::PathBuf};

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
        match self.own_run(id) {
            Ok(owner) => {
                drop(owner);
                bail!("run '{id}' has no active scheduler process");
            }
            Err(error) if error.to_string().contains("active in another process") => {}
            Err(error) => return Err(error),
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
