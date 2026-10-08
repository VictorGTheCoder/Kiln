//! Live provider activity: every redacted provider stdout line is appended, as
//! it arrives, to `.kiln/runs/<id>/agents/<ticket>-<stage>.log`, and can be
//! echoed as a readable summary ([`summarise`]) of codex and claude events.
//!
//! The engine names the session with [`Engine::with_agent_log`] around each
//! provider call; the provider adapter only sees the thread-local sink. Like
//! the journal, these logs only observe: failing to write them never fails a
//! session, and recovery never reads them.
use crate::Engine;
use serde_json::Value;
use std::{
    cell::RefCell,
    fs,
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

/// One provider session being logged: which ticket and stage it serves.
#[derive(Debug, Clone)]
pub struct SessionLog {
    pub path: PathBuf,
    /// Ticket id; `run` for run-level sessions such as planning.
    pub ticket: String,
    pub stage: String,
}

/// One readable summary of a provider event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub ticket: String,
    pub stage: String,
    pub summary: String,
}
impl Activity {
    /// One terminal line: `HH:MM:SS  ticket  stage | summary`.
    pub fn line(&self) -> String {
        let ts = crate::journal::timestamp(std::time::SystemTime::now());
        let clock = ts.get(11..19).unwrap_or(&ts);
        format!(
            "{clock}  {}  {} | {}",
            self.ticket, self.stage, self.summary
        )
    }
}

/// File name component of a ticket id: anything outside `[A-Za-z0-9._-]`
/// becomes `-` (ticket ids such as `github:owner/repo#7` contain `:/#`).
pub fn file_component(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

thread_local! {
    static SESSION_LOG: RefCell<Option<SessionLog>> = const { RefCell::new(None) };
}

impl Engine {
    /// Path of the live log of one ticket (or `None` for the run) and stage.
    pub fn agent_log_path(&self, id: &str, ticket: Option<&str>, stage: &str) -> PathBuf {
        self.repository
            .join(".kiln/runs")
            .join(id)
            .join("agents")
            .join(format!(
                "{}-{}.log",
                file_component(ticket.unwrap_or("run")),
                file_component(stage)
            ))
    }
    /// Run `operation` with provider sessions on this thread logged to
    /// `agents/<ticket>-<stage>.log` of run `id`.
    pub fn with_agent_log<T>(
        &self,
        id: &str,
        ticket: Option<&str>,
        stage: &str,
        operation: impl FnOnce() -> T,
    ) -> T {
        let log = SessionLog {
            path: self.agent_log_path(id, ticket, stage),
            ticket: ticket.unwrap_or("run").to_owned(),
            stage: stage.to_owned(),
        };
        SESSION_LOG.with(|current| {
            let previous = current.replace(Some(log));
            let result = operation();
            current.replace(previous);
            result
        })
    }
}

/// Live sink of one provider session, opened by the adapter when it starts.
pub(crate) struct Sink {
    file: Option<fs::File>,
    log: SessionLog,
}
impl Sink {
    /// The sink named by the enclosing [`Engine::with_agent_log`], if any.
    pub(crate) fn current() -> Option<Self> {
        let log = SESSION_LOG.with(|current| current.borrow().clone())?;
        let file = log
            .path
            .parent()
            .and_then(|dir| fs::create_dir_all(dir).ok())
            .and_then(|_| {
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log.path)
                    .ok()
            });
        Some(Self { file, log })
    }
    /// Append one already-redacted line and echo its summary. Best effort.
    pub(crate) fn line(&mut self, safe: &str) {
        if let Some(file) = &mut self.file {
            let _ = file.write_all(format!("{safe}\n").as_bytes());
        }
        let Some(summary) = summarise_line(safe) else {
            return;
        };
        if let Ok(echo) = ECHO.get_or_init(Default::default).lock() {
            if let Some(echo) = echo.as_ref() {
                echo(&Activity {
                    ticket: self.log.ticket.clone(),
                    stage: self.log.stage.clone(),
                    summary,
                });
            }
        }
    }
}

type Echo = Box<dyn Fn(&Activity) + Send + Sync>;
static ECHO: OnceLock<Mutex<Option<Echo>>> = OnceLock::new();

/// Hand a summary of every provider event of this process to `echo` (e.g.
/// `kiln start --verbose`).
pub fn echo(echo: impl Fn(&Activity) + Send + Sync + 'static) {
    if let Ok(mut slot) = ECHO.get_or_init(Default::default).lock() {
        *slot = Some(Box::new(echo));
    }
}

/// Summary of one raw provider stdout line; `None` for lines that are not
/// JSON or carry nothing worth showing.
pub fn summarise_line(line: &str) -> Option<String> {
    summarise(&serde_json::from_str(line).ok()?)
}

const LIMIT: usize = 160;

/// Readable summary of one codex (`codex exec --json`) or claude
/// (`--output-format stream-json`) event: file edited, command run, message.
pub fn summarise(event: &Value) -> Option<String> {
    match event["type"].as_str()? {
        // codex
        "item.completed" => {
            let item = &event["item"];
            match item["type"].as_str()? {
                "command_execution" => labelled("ran", item["command"].as_str()?),
                "file_change" => {
                    let paths: Vec<&str> = item["changes"]
                        .as_array()?
                        .iter()
                        .filter_map(|c| c["path"].as_str())
                        .collect();
                    labelled("edited", &paths.join(", "))
                }
                "agent_message" => labelled("message", item["text"].as_str()?),
                "error" => labelled("error", item["message"].as_str()?),
                _ => None,
            }
        }
        "turn.completed" => Some("turn completed".into()),
        "turn.failed" | "error" => Some("provider failure".into()),
        // claude
        "assistant" => {
            let parts: Vec<String> = event["message"]["content"]
                .as_array()?
                .iter()
                .filter_map(claude_content)
                .collect();
            (!parts.is_empty()).then(|| parts.join("; "))
        }
        "result" if event["is_error"] == true => Some("provider failure".into()),
        "result" => Some("turn completed".into()),
        _ => None,
    }
}

fn claude_content(content: &Value) -> Option<String> {
    match content["type"].as_str()? {
        "text" => labelled("message", content["text"].as_str()?),
        "tool_use" => {
            let name = content["name"].as_str().unwrap_or("tool");
            let input = &content["input"];
            match name {
                "Bash" => labelled("ran", input["command"].as_str()?),
                "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => labelled(
                    "edited",
                    input["file_path"]
                        .as_str()
                        .or_else(|| input["notebook_path"].as_str())?,
                ),
                _ => Some(format!("tool: {name}")),
            }
        }
        _ => None,
    }
}

/// `label: value` on one line, clipped; `None` when `value` is blank.
fn labelled(label: &str, value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()).then(|| format!("{label}: {}", clip(&value)))
}

fn clip(text: &str) -> String {
    if text.chars().count() <= LIMIT {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(LIMIT).collect();
    clipped.push_str("...");
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_and_claude_events_have_the_same_summaries() {
        let codex = [
            json!({"type":"item.completed","item":{"type":"command_execution","command":"cargo test"}}),
            json!({"type":"item.completed","item":{"type":"file_change","changes":[{"path":"src/a.rs","kind":"update"}]}}),
            json!({"type":"item.completed","item":{"type":"agent_message","text":"All done"}}),
        ];
        let claude = [
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}),
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/a.rs"}}]}}),
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"All done"}]}}),
        ];
        let expected = ["ran: cargo test", "edited: src/a.rs", "message: All done"];
        for (event, want) in codex.iter().zip(expected) {
            assert_eq!(summarise(event).as_deref(), Some(want));
        }
        for (event, want) in claude.iter().zip(expected) {
            assert_eq!(summarise(event).as_deref(), Some(want));
        }
    }

    #[test]
    fn unknown_and_malformed_lines_have_no_summary() {
        assert_eq!(summarise_line("not json {"), None);
        assert_eq!(summarise_line(r#"{"type":"mystery"}"#), None);
        assert_eq!(summarise_line(r#"{"no":"type"}"#), None);
        assert_eq!(summarise_line("[1,2]"), None);
    }

    #[test]
    fn ticket_ids_become_safe_file_names() {
        assert_eq!(
            file_component("github:example/project#7"),
            "github-example-project-7"
        );
        assert_eq!(file_component("pilot-1.a_b"), "pilot-1.a_b");
    }
}
