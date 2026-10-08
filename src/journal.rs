//! Per-run event journal: an append-only observation channel at
//! `.kiln/runs/<id>/events.jsonl`, one JSON object per line:
//! `{"ts", "ticket"?, "stage", "status", "message"}`. `ticket` is absent for
//! run-level events.
//!
//! The journal only observes the pipeline. The recorded run state
//! (`.kiln/runs/<id>.json`) stays the source of truth: recovery never reads
//! the journal, and failing to append to it never fails a run.
//!
//! Every process that appends to a run's journal holds a shared `flock` on it
//! until it exits, so a reader can tell whether a writer is still live
//! ([`Journal::has_writer`]).
use crate::Engine;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

/// One pipeline transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// RFC 3339 UTC timestamp with milliseconds, e.g. `2026-10-08T14:03:59.120Z`.
    pub ts: String,
    /// Ticket id; absent for run-level events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    /// run | plan | implementation | review | integration | pull-request | ci
    pub stage: String,
    pub status: String,
    #[serde(default)]
    pub message: String,
}

impl Event {
    /// One readable terminal line: `HH:MM:SS  [ticket  ]stage status[: message]`.
    pub fn line(&self) -> String {
        let clock = self.ts.get(11..19).unwrap_or(&self.ts);
        let ticket = self
            .ticket
            .as_deref()
            .map(|t| format!("{t}  "))
            .unwrap_or_default();
        let message = if self.message.is_empty() {
            String::new()
        } else {
            format!(": {}", self.message)
        };
        format!("{clock}  {ticket}{} {}{message}", self.stage, self.status)
    }
}

/// Journal file of one run.
pub struct Journal {
    pub path: PathBuf,
}

impl Journal {
    pub fn of(engine: &Engine, id: &str) -> Self {
        Self {
            path: engine
                .repository
                .join(".kiln/runs")
                .join(id)
                .join("events.jsonl"),
        }
    }
    pub fn exists(&self) -> bool {
        self.path.is_file()
    }
    /// Events from byte `offset` on; returns them and the offset after the
    /// last complete line. Lines that do not parse (a truncated or damaged
    /// journal) are skipped.
    pub fn read_from(&self, offset: u64) -> Result<(Vec<Event>, u64)> {
        let (events, end) = self.read_each_from(offset)?;
        Ok((events.into_iter().map(|(event, _)| event).collect(), end))
    }
    /// Like [`Self::read_from`], with the offset after each event's line, so
    /// a reader can resume right after any event it has handed on.
    pub fn read_each_from(&self, offset: u64) -> Result<(Vec<(Event, u64)>, u64)> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(e) => return Err(e).with_context(|| format!("read {}", self.path.display())),
        };
        let start = (offset as usize).min(bytes.len());
        let mut events = Vec::new();
        let mut position = start;
        while let Some(newline) = bytes[position..].iter().position(|b| *b == b'\n') {
            let end = position + newline + 1;
            let line = String::from_utf8_lossy(&bytes[position..end]);
            if let Ok(event) = serde_json::from_str(line.trim_end()) {
                events.push((event, end as u64));
            }
            position = end;
        }
        Ok((events, position as u64))
    }
    pub fn read(&self) -> Result<Vec<Event>> {
        Ok(self.read_from(0)?.0)
    }
    /// Whether any live process still appends to this journal.
    pub fn has_writer(&self) -> bool {
        let Ok(file) = fs::File::open(&self.path) else {
            return false;
        };
        let free = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if free {
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        }
        !free
    }
    fn append(&self, event: &Event) -> Result<()> {
        let dir = self.path.parent().context("journal directory")?;
        fs::create_dir_all(dir)?;
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        hold_writer_lock(&self.path, &file)?;
        // One write per line with O_APPEND keeps concurrent workers' lines whole.
        file.write_all(line.as_bytes())?;
        Ok(())
    }
}

type Echo = Box<dyn Fn(&Event) + Send + Sync>;
static ECHO: OnceLock<Mutex<Option<Echo>>> = OnceLock::new();
static WRITERS: OnceLock<Mutex<BTreeMap<PathBuf, fs::File>>> = OnceLock::new();

/// Keep a shared lock on the journal for the rest of this process.
fn hold_writer_lock(path: &Path, file: &fs::File) -> Result<()> {
    let mut writers = WRITERS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| anyhow::anyhow!("journal lock table poisoned"))?;
    if !writers.contains_key(path) {
        let held = file.try_clone()?;
        unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_SH) };
        writers.insert(path.to_owned(), held);
    }
    Ok(())
}

/// Also hand every event appended by this process to `echo` (e.g. the
/// terminal reporter of `kiln start`).
pub fn echo(echo: impl Fn(&Event) + Send + Sync + 'static) {
    if let Ok(mut slot) = ECHO.get_or_init(Default::default).lock() {
        *slot = Some(Box::new(echo));
    }
}

/// RFC 3339 UTC timestamp with milliseconds.
pub fn timestamp(time: SystemTime) -> String {
    let ms = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let (secs, millis) = ((ms / 1000) as i64, ms % 1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

impl Engine {
    /// Append one pipeline transition to the run's journal. Best effort: the
    /// journal is an observation channel and never fails the pipeline.
    pub fn journal(
        &self,
        id: &str,
        ticket: Option<&str>,
        stage: &str,
        status: &str,
        message: impl Into<String>,
    ) {
        let event = Event {
            ts: timestamp(SystemTime::now()),
            ticket: ticket.map(str::to_owned),
            stage: stage.into(),
            status: status.into(),
            message: message.into(),
        };
        // Serialize appends and echoes so terminal lines follow journal order.
        let echo = ECHO.get_or_init(Default::default).lock();
        let _ = Journal::of(self, id).append(&event);
        if let Ok(echo) = &echo {
            if let Some(echo) = echo.as_ref() {
                echo(&event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let t = UNIX_EPOCH + Duration::from_millis(1_791_469_674_123);
        assert_eq!(timestamp(t), "2026-10-08T14:27:54.123Z");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }
}
