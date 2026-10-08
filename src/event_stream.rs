//! Server-Sent Events stream of one run for the dashboard
//! (`GET /api/dashboard/runs/<id>/events`).
//!
//! A client first receives every event it has not seen yet, then live ones as
//! they are appended, and finally an `end` event once no process writes the
//! run's journal any more; the server then closes the connection. A run
//! without a journal (recorded by an earlier Kiln version, or not started)
//! gets only the `end` event.
//!
//! Frames:
//! - `event: journal`: `data` is one [`crate::journal::Event`].
//! - `event: provider`: `data` is one line of a provider session log
//!   (`.kiln/runs/<id>/agents/<ticket>-<stage>.log`, already redacted when it
//!   was written): `{"ticket", "stage", "line", "summary"}`, where `line` is
//!   the raw provider event and `summary` its readable
//!   [`crate::activity::summarise_line`] (`null` when it has none). Lines of
//!   the session being written are sent as the provider emits them, so the
//!   latest ones are the active ticket's.
//! - `event: end`: `data` is `{"status": <recorded run status>}`.
//!
//! Every data frame carries an `id:` that is the stream [`Cursor`]; a client
//! reconnecting with `Last-Event-ID` resumes right after it. To stream another
//! kind of event, add its position to [`Cursor`] and poll it next to the
//! journal in [`EventStream::serve`].
use crate::{activity, journal::Journal, Engine, Run};
use anyhow::Result;
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

/// How often the journal is polled for new lines.
const POLL: Duration = Duration::from_millis(200);
/// Idle time after which a comment is sent, to notice closed connections.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Position of a client in every source of the stream, sent as the SSE `id`
/// in the form `journal=<byte offset>[;agent:<log file>=<byte offset>]...`;
/// unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    /// Byte offset in the run's journal after the last event sent.
    pub journal: u64,
    /// Byte offset after the last line sent of each provider session log, by
    /// file name (e.g. `github-o-r-7-implementation.log`).
    pub agents: BTreeMap<String, u64>,
}
impl Cursor {
    /// The cursor of a `Last-Event-ID`; the start of the stream when absent or
    /// unreadable.
    pub fn parse(id: Option<&str>) -> Self {
        let mut cursor = Self::default();
        for part in id.unwrap_or_default().split(';') {
            let Some((key, value)) = part.trim().split_once('=') else {
                continue;
            };
            let Ok(offset) = value.parse() else {
                continue;
            };
            if key == "journal" {
                cursor.journal = offset;
            } else if let Some(name) = key.strip_prefix("agent:") {
                if is_log_name(name) {
                    cursor.agents.insert(name.to_owned(), offset);
                }
            }
        }
        cursor
    }
}
impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "journal={}", self.journal)?;
        for (name, offset) in &self.agents {
            write!(f, ";agent:{name}={offset}")?;
        }
        Ok(())
    }
}

/// A provider session log file name, as [`Engine::agent_log_path`] writes it.
fn is_log_name(name: &str) -> bool {
    name.len() > 4 && name.ends_with(".log") && activity::file_component(name) == name
}

/// Complete lines of `path` from byte `offset` on, each with the offset after
/// it, and the offset after the last complete line. A line still being
/// written is left for the next read.
fn read_lines_from(path: &PathBuf, offset: u64) -> (Vec<(String, u64)>, u64) {
    let mut bytes = Vec::new();
    let read = fs::File::open(path).and_then(|mut file| {
        file.seek(SeekFrom::Start(offset))?;
        file.read_to_end(&mut bytes)
    });
    if read.is_err() {
        return (Vec::new(), offset);
    }
    let mut lines = Vec::new();
    let mut position = 0;
    while let Some(newline) = bytes[position..].iter().position(|b| *b == b'\n') {
        let end = position + newline + 1;
        let line = String::from_utf8_lossy(&bytes[position..end - 1]).into_owned();
        if !line.trim().is_empty() {
            lines.push((line, offset + end as u64));
        }
        position = end;
    }
    (lines, offset + position as u64)
}

/// One SSE frame.
fn frame(event: &str, id: Option<&Cursor>, data: &str) -> String {
    let mut frame = String::new();
    if let Some(id) = id {
        frame.push_str(&format!("id: {id}\n"));
    }
    frame.push_str(&format!("event: {event}\n"));
    for line in data.lines() {
        frame.push_str(&format!("data: {line}\n"));
    }
    frame.push('\n');
    frame
}

/// The event stream of one recorded run.
pub struct EventStream {
    engine: Engine,
    run: Run,
    journal: Journal,
}
impl EventStream {
    /// Fails when the run is not recorded.
    pub fn open(engine: &Engine, id: &str) -> Result<Self> {
        let run = engine.inspect(id)?;
        Ok(Self {
            journal: Journal::of(engine, id),
            engine: engine.clone(),
            run,
        })
    }

    /// Write the HTTP response and the stream to `out` until the run's
    /// journal has no writer left or the client goes away. `redact` is
    /// applied to every frame.
    pub fn serve(
        self,
        mut out: impl Write,
        cursor: Cursor,
        headers: &[(&str, &str)],
        redact: impl Fn(&Run, &str) -> String,
    ) -> std::io::Result<()> {
        let mut head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\n".to_owned();
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        out.flush()?;
        let mut out = Chunked(out);

        let mut cursor = cursor;
        // Ticket and stage of each provider session log, by file name.
        let mut sessions: BTreeMap<String, (String, String)> = BTreeMap::new();
        let mut last_write = Instant::now();
        loop {
            // Check for a writer before reading, so a writer that appends and
            // exits in between still has its last lines sent.
            let live = self.journal.has_writer();
            let (events, next) = self
                .journal
                .read_each_from(cursor.journal)
                .unwrap_or_default();
            let mut chunk = String::new();
            for (event, end) in &events {
                cursor.journal = *end;
                let data = serde_json::to_string(event).unwrap_or_default();
                chunk.push_str(&frame("journal", Some(&cursor), &data));
            }
            // Skip unparsable lines too.
            cursor.journal = cursor.journal.max(next);
            for (name, path) in self.agent_logs() {
                let offset = cursor.agents.get(&name).copied().unwrap_or(0);
                let (lines, next) = read_lines_from(&path, offset);
                if next == offset {
                    continue;
                }
                let (ticket, stage) = sessions
                    .entry(name.clone())
                    .or_insert_with(|| self.session_of(&name))
                    .clone();
                for (line, end) in lines {
                    cursor.agents.insert(name.clone(), end);
                    let summary = activity::summarise_line(&line).map(|s| redact(&self.run, &s));
                    let data = serde_json::json!({
                        "ticket": ticket,
                        "stage": stage,
                        "line": redact(&self.run, &line),
                        "summary": summary,
                    });
                    chunk.push_str(&frame("provider", Some(&cursor), &data.to_string()));
                }
                cursor.agents.insert(name, next);
            }
            if !live {
                let status = self
                    .engine
                    .inspect(&self.run.id)
                    .map_or_else(|_| self.run.status.clone(), |run| run.status);
                let data = serde_json::json!({ "status": status }).to_string();
                chunk.push_str(&frame("end", Some(&cursor), &data));
            }
            if !chunk.is_empty() {
                out.write_all(redact(&self.run, &chunk).as_bytes())?;
                out.flush()?;
                last_write = Instant::now();
            } else if last_write.elapsed() >= KEEP_ALIVE {
                out.write_all(b": keep-alive\n\n")?;
                out.flush()?;
                last_write = Instant::now();
            }
            if !live {
                // The last chunk ends the response; the connection stays usable.
                out.0.write_all(b"0\r\n\r\n")?;
                return out.0.flush();
            }
            std::thread::sleep(POLL);
        }
    }

    /// The run's provider session logs, oldest first, by file name.
    fn agent_logs(&self) -> Vec<(String, PathBuf)> {
        let path = self.engine.agent_log_path(&self.run.id, None, "plan");
        let Some(Ok(entries)) = path.parent().map(fs::read_dir) else {
            return Vec::new();
        };
        let mut logs: Vec<(SystemTime, String, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_owned();
                let metadata = entry.metadata().ok()?;
                (metadata.is_file() && is_log_name(&name)).then(|| {
                    let created = metadata
                        .created()
                        .or_else(|_| metadata.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (created, name, entry.path())
                })
            })
            .collect();
        logs.sort();
        logs.into_iter()
            .map(|(_, name, path)| (name, path))
            .collect()
    }

    /// Ticket id and stage of a provider session log: the file name is
    /// `<ticket>-<stage>.log` with the ticket id made file-safe, so it is
    /// matched against the run's tickets and delivery groups (`run` for
    /// run-level sessions).
    fn session_of(&self, name: &str) -> (String, String) {
        let stem = name.strip_suffix(".log").unwrap_or(name);
        let run = self
            .engine
            .inspect(&self.run.id)
            .unwrap_or_else(|_| self.run.clone());
        let tickets = run
            .plan
            .iter()
            .flat_map(|plan| plan.tickets.iter().map(|t| t.id.clone()))
            .chain(run.delivery_groups.iter().map(|g| g.id.clone()))
            .chain(["run".to_owned()]);
        tickets
            .filter_map(|ticket| {
                let stage =
                    stem.strip_prefix(&format!("{}-", activity::file_component(&ticket)))?;
                (!stage.is_empty()).then(|| (ticket, stage.to_owned()))
            })
            // The longest ticket id wins (`a-b-review` is ticket `a-b`, not `a`).
            .max_by_key(|(ticket, _)| ticket.len())
            .unwrap_or_else(|| match stem.rsplit_once('-') {
                Some((ticket, stage)) => (ticket.to_owned(), stage.to_owned()),
                None => (stem.to_owned(), String::new()),
            })
    }
}

/// HTTP/1.1 chunked transfer coding, one chunk per write, so every frame
/// reaches the client as soon as it is flushed.
struct Chunked<W: Write>(W);
impl<W: Write> Write for Chunked<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !buf.is_empty() {
            write!(self.0, "{:x}\r\n", buf.len())?;
            self.0.write_all(buf)?;
            self.0.write_all(b"\r\n")?;
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_and_ignores_unknown_parts() {
        let cursor = Cursor {
            journal: 42,
            agents: [("a-implementation.log".to_owned(), 7)].into(),
        };
        assert_eq!(Cursor::parse(Some(&cursor.to_string())), cursor);
        assert_eq!(Cursor::parse(Some("agents=7;journal=9")).journal, 9);
        assert!(
            Cursor::parse(Some("agent:../x.log=3;agent:a.txt=1"))
                .agents
                .is_empty(),
            "only provider log names are kept"
        );
        assert_eq!(Cursor::parse(Some("garbage")), Cursor::default());
        assert_eq!(Cursor::parse(None), Cursor::default());
    }
}
