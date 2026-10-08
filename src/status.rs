//! Readable and machine-readable summaries of a recorded run (`kiln status`).
//!
//! A summary is derived only from the recorded run state (`.kiln/runs/<id>.json`),
//! so runs recorded by earlier Kiln versions summarise the same way. The event
//! journal (`.kiln/runs/<id>/events.jsonl`) can later refine a summary built
//! here (for example the latest activity of each ticket) without replacing it:
//! the recorded state stays the source of truth.
use crate::{Engine, Run};
use anyhow::Result;
use serde::Serialize;
use std::fmt::Write;

/// Pipeline order of ticket stages; unknown stages sort after these.
const STAGES: [&str; 9] = [
    "planned",
    "waiting",
    "implementing",
    "integrating",
    "integrated",
    "delivering",
    "delivered",
    "blocked",
    "stopped",
];

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub status: String,
    pub created_unix_ms: u128,
    pub github_repository: Option<String>,
    /// Ticket counts in pipeline order; only stages with tickets appear.
    pub stages: Vec<StageCount>,
    pub tickets: Vec<TicketSummary>,
    /// Tickets currently holding an implementation slot.
    pub active: Vec<TicketSummary>,
    pub pull_requests: Vec<PullRequestSummary>,
    /// Local dashboard of a running server. Kiln does not record one yet.
    pub dashboard_url: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct StageCount {
    pub stage: String,
    pub count: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct TicketSummary {
    pub id: String,
    pub title: String,
    pub stage: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct PullRequestSummary {
    pub number: u64,
    pub url: String,
    pub tickets: Vec<String>,
    /// Latest CI observation, else the delivery status awaiting one.
    pub ci: String,
}

/// The run with the newest `created_unix_ms` in the repository, if any.
pub fn latest(engine: &Engine) -> Result<Option<Run>> {
    let mut latest: Option<Run> = None;
    for id in engine.list()? {
        let run = engine.inspect(&id)?;
        if latest
            .as_ref()
            .is_none_or(|l| (run.created_unix_ms, &run.id) > (l.created_unix_ms, &l.id))
        {
            latest = Some(run);
        }
    }
    Ok(latest)
}

impl RunSummary {
    /// Summarise recorded run state.
    pub fn of(run: &Run) -> Self {
        let planned = run
            .plan
            .as_ref()
            .map(|p| p.tickets.as_slice())
            .unwrap_or_default();
        let scheduled = run
            .scheduler
            .as_ref()
            .map(|s| s.tickets.as_slice())
            .unwrap_or_default();
        let mut ids: Vec<&str> = planned.iter().map(|t| t.id.as_str()).collect();
        for t in scheduled {
            if !ids.contains(&t.id.as_str()) {
                ids.push(&t.id);
            }
        }
        let tickets: Vec<TicketSummary> = ids
            .into_iter()
            .map(|id| TicketSummary {
                id: id.to_owned(),
                title: planned
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.title.clone())
                    .unwrap_or_default(),
                stage: stage(
                    run,
                    id,
                    scheduled
                        .iter()
                        .find(|t| t.id == id)
                        .map(|t| t.state.as_str()),
                ),
            })
            .collect();
        let mut stages: Vec<StageCount> = Vec::new();
        for t in &tickets {
            match stages.iter_mut().find(|s| s.stage == t.stage) {
                Some(s) => s.count += 1,
                None => stages.push(StageCount {
                    stage: t.stage.clone(),
                    count: 1,
                }),
            }
        }
        stages.sort_by_key(|s| {
            STAGES
                .iter()
                .position(|k| *k == s.stage)
                .unwrap_or(STAGES.len())
        });
        let active = run
            .scheduler
            .iter()
            .flat_map(|s| &s.active)
            .filter_map(|id| tickets.iter().find(|t| &t.id == id).cloned())
            .collect();
        let mut pull_requests: Vec<PullRequestSummary> = run
            .delivery_groups
            .iter()
            .filter_map(|g| {
                let pr = g.pull_request.as_ref()?;
                Some(PullRequestSummary {
                    number: pr.number,
                    url: pr.url.clone(),
                    tickets: g.tickets.clone(),
                    ci: g
                        .ci_attempts
                        .last()
                        .map_or_else(|| g.status.clone(), |a| a.status.clone()),
                })
            })
            .collect();
        // Single-issue runs record one publication instead of delivery groups.
        if let Some(publication) = &run.publication {
            if let Some(pr) = &publication.pull_request {
                pull_requests.push(PullRequestSummary {
                    number: pr.number,
                    url: pr.url.clone(),
                    tickets: tickets.iter().map(|t| t.id.clone()).collect(),
                    ci: publication.status.clone(),
                });
            }
        }
        Self {
            id: run.id.clone(),
            status: run.status.clone(),
            created_unix_ms: run.created_unix_ms,
            github_repository: run.backlog.as_ref().map(|b| b.github_repository.clone()),
            stages,
            tickets,
            active,
            pull_requests,
            dashboard_url: None,
        }
    }

    /// Readable multi-line summary.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let repository = self
            .github_repository
            .as_ref()
            .map(|r| format!(" for {r}"))
            .unwrap_or_default();
        let _ = writeln!(out, "Run {}{repository}", self.id);
        let _ = writeln!(out, "Status: {}", self.status);
        if self.stages.is_empty() {
            let _ = writeln!(out, "Tickets: none planned");
        } else {
            let counts: Vec<String> = self
                .stages
                .iter()
                .map(|s| format!("{} {}", s.stage, s.count))
                .collect();
            let _ = writeln!(
                out,
                "Tickets ({}): {}",
                self.tickets.len(),
                counts.join(", ")
            );
        }
        for t in &self.active {
            let _ = writeln!(out, "Active: {}  {}", t.id, t.title);
        }
        if self.pull_requests.is_empty() {
            let _ = writeln!(out, "Pull requests: none");
        } else {
            let _ = writeln!(out, "Pull requests:");
            for pr in &self.pull_requests {
                let _ = writeln!(
                    out,
                    "  #{} {}  CI: {}  ({})",
                    pr.number,
                    pr.url,
                    pr.ci,
                    pr.tickets.join(", ")
                );
            }
        }
        if let Some(url) = &self.dashboard_url {
            let _ = writeln!(out, "Dashboard: {url}");
        }
        out
    }
}

/// Stage of one ticket from its scheduler state and its delivery group.
fn stage(run: &Run, id: &str, scheduled: Option<&str>) -> String {
    match scheduled {
        None => "planned".into(),
        Some("awaiting_integration" | "integrating") => "integrating".into(),
        Some("integrated") => {
            match run
                .delivery_groups
                .iter()
                .find(|g| g.tickets.iter().any(|t| t == id))
            {
                Some(g) if g.status == "verified" => "delivered".into(),
                Some(g) if g.pull_request.is_some() => "delivering".into(),
                _ => "integrated".into(),
            }
        }
        Some(state) => state.to_owned(),
    }
}
