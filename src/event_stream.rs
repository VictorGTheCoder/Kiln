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
//! - `event: end`: `data` is `{"status": <recorded run status>}`.
//!
//! Every data frame carries an `id:` that is the stream [`Cursor`]; a client
//! reconnecting with `Last-Event-ID` resumes right after it. To stream another
//! kind of event (e.g. provider activity), add its position to [`Cursor`] and
//! poll it next to the journal in [`EventStream::serve`].
use crate::{journal::Journal, Engine, Run};
use anyhow::Result;
use std::{
    fmt,
    io::Write,
    time::{Duration, Instant},
};

/// How often the journal is polled for new lines.
const POLL: Duration = Duration::from_millis(200);
/// Idle time after which a comment is sent, to notice closed connections.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Position of a client in every source of the stream, sent as the SSE `id`
/// in the form `journal=<byte offset>`; unknown keys are ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    /// Byte offset in the run's journal after the last event sent.
    pub journal: u64,
}
impl Cursor {
    /// The cursor of a `Last-Event-ID`; the start of the stream when absent or
    /// unreadable.
    pub fn parse(id: Option<&str>) -> Self {
        let mut cursor = Self::default();
        for part in id.unwrap_or_default().split(';') {
            if let Some(("journal", value)) = part.trim().split_once('=') {
                cursor.journal = value.parse().unwrap_or(0);
            }
        }
        cursor
    }
}
impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "journal={}", self.journal)
    }
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
        let cursor = Cursor { journal: 42 };
        assert_eq!(Cursor::parse(Some(&cursor.to_string())), cursor);
        assert_eq!(Cursor::parse(Some("agents=7;journal=9")).journal, 9);
        assert_eq!(Cursor::parse(Some("garbage")), Cursor::default());
        assert_eq!(Cursor::parse(None), Cursor::default());
    }
}
