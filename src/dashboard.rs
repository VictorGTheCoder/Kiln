//! Dashboard data: the runs list and a run's ticket kanban, derived only from
//! recorded run state (`.kiln/runs/<id>.json`), so runs recorded by earlier
//! Kiln versions appear the same way. Also the embedded static page and script,
//! and the record of where a dashboard server is running.
use crate::{state::Stage, Engine, Run};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The dashboard page. It loads only [`SCRIPT`]; no front-end build step.
pub const PAGE: &str = include_str!("dashboard/index.html");
/// The dashboard script, served from the binary at `/dashboard.js`.
pub const SCRIPT: &str = include_str!("dashboard/app.js");

/// Kanban columns in pipeline order.
pub const STAGES: [&str; 6] = [
    "planned",
    "implementing",
    "review",
    "integrating",
    "pr",
    "ci",
];

/// One entry of the runs list.
#[derive(Debug, Clone, Serialize)]
pub struct RunEntry {
    pub id: String,
    pub status: String,
    pub created_unix_ms: u128,
    pub github_repository: Option<String>,
    pub tickets: usize,
}

/// A run's tickets by stage.
#[derive(Debug, Clone, Serialize)]
pub struct Board {
    pub id: String,
    pub status: String,
    pub created_unix_ms: u128,
    pub github_repository: Option<String>,
    /// Every stage in pipeline order, empty ones included.
    pub stages: Vec<Column>,
    /// What the operator should notice, in recorded order.
    pub attention: Vec<Attention>,
    /// Tickets the run is working on now (implementation, review or
    /// correction): whose provider activity the dashboard shows by default.
    pub active: Vec<String>,
}
/// Something that needs the operator's attention.
#[derive(Debug, Clone, Serialize)]
pub struct Attention {
    /// failure | divergence | decision
    pub kind: &'static str,
    /// The ticket concerned; absent for run-level items.
    pub ticket: Option<String>,
    pub message: String,
}
impl Attention {
    fn new(kind: &'static str, ticket: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            kind,
            ticket: ticket.map(str::to_owned),
            message: message.into(),
        }
    }
}

/// Failures, divergences from the plan or spec, and decisions Kiln took on
/// its own, from recorded run state.
fn attention(run: &Run) -> Vec<Attention> {
    let mut items = Vec::new();
    for t in run.scheduler.iter().flat_map(|s| &s.tickets) {
        if t.state == "blocked" {
            let reason = t.blocker.clone().unwrap_or_else(|| "blocked".into());
            items.push(Attention::new("failure", Some(&t.id), reason));
        }
    }
    if let Some(e) = run
        .scheduler
        .as_ref()
        .and_then(|s| s.limits.as_ref())
        .and_then(|l| l.exhausted.as_ref())
    {
        items.push(Attention::new(
            "failure",
            None,
            format!("{} limit reached: {}", e.limit, e.reason),
        ));
    }
    for group in &run.delivery_groups {
        let pr = group
            .pull_request
            .as_ref()
            .map(|p| format!(" on PR #{}", p.number))
            .unwrap_or_default();
        let message = match group.status.as_str() {
            "ci-failed" => format!("CI failed{pr}"),
            "delivery-blocked" => format!("Delivery blocked{pr}"),
            _ => continue,
        };
        for ticket in &group.tickets {
            items.push(Attention::new("failure", Some(ticket), message.clone()));
        }
    }
    if let Some(p) = &run.publication {
        if p.status == "ci-failed" {
            items.push(Attention::new("failure", None, "CI failed"));
        }
    }
    for r in &run.replans {
        let message = format!("Replanned ({}) after: {}", r.outcome, r.failures.join("; "));
        items.push(Attention::new("divergence", Some(&r.ticket_id), message));
    }
    for s in &run.spec_revisions {
        let message = format!(
            "Spec {} revised to version {} ({})",
            s.path, s.version, s.status
        );
        items.push(Attention::new("divergence", None, message));
    }
    for d in &run.decisions {
        let message = format!("{} → {} ({})", d.question, d.resolution, d.outcome);
        items.push(Attention::new("decision", None, message));
    }
    for d in run.backlog.iter().flat_map(|b| &b.decisions) {
        items.push(Attention::new("decision", None, d.clone()));
    }
    items
}
#[derive(Debug, Clone, Serialize)]
pub struct Column {
    pub stage: String,
    pub tickets: Vec<Card>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub id: String,
    pub title: String,
    /// Recorded scheduler state, or `planned` before scheduling.
    pub state: String,
    pub blocked_by: Vec<String>,
    pub blocker: Option<String>,
    pub pull_request: Option<CardPullRequest>,
    /// Latest CI observation of the ticket's pull request, if any.
    pub ci: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CardPullRequest {
    pub number: u64,
    pub url: String,
}

/// Recorded runs of the repository, newest first. Unreadable runs are skipped.
pub fn runs(engine: &Engine) -> Result<Vec<RunEntry>> {
    let mut runs: Vec<RunEntry> = engine
        .list()?
        .iter()
        .filter_map(|id| engine.inspect(id).ok())
        .map(|run| RunEntry {
            tickets: tickets_of(&run).len(),
            id: run.id,
            status: run.status,
            created_unix_ms: run.created_unix_ms,
            github_repository: run.backlog.map(|b| b.github_repository),
        })
        .collect();
    runs.sort_by(|a, b| (b.created_unix_ms, &b.id).cmp(&(a.created_unix_ms, &a.id)));
    Ok(runs)
}

/// Planned ticket ids followed by any scheduled ticket missing from the plan.
fn tickets_of(run: &Run) -> Vec<&str> {
    let mut ids: Vec<&str> = run
        .plan
        .iter()
        .flat_map(|p| &p.tickets)
        .map(|t| t.id.as_str())
        .collect();
    for t in run.scheduler.iter().flat_map(|s| &s.tickets) {
        if !ids.contains(&t.id.as_str()) {
            ids.push(&t.id);
        }
    }
    ids
}

impl Board {
    pub fn of(run: &Run) -> Self {
        let mut stages: Vec<Column> = STAGES
            .iter()
            .map(|s| Column {
                stage: (*s).to_owned(),
                tickets: Vec::new(),
            })
            .collect();
        for id in tickets_of(run) {
            let plan = run
                .plan
                .iter()
                .flat_map(|p| &p.tickets)
                .find(|t| t.id == id);
            let schedule = run
                .scheduler
                .iter()
                .flat_map(|s| &s.tickets)
                .find(|t| t.id == id);
            let group = run.delivery_group_of(id);
            let (pull_request, ci) = match group {
                Some(g) => (
                    g.pull_request.as_ref(),
                    g.ci_attempts.last().map(|a| a.status.clone()),
                ),
                // Single-issue runs record one publication for the run.
                None => match run.publication.as_ref() {
                    Some(p) => (p.pull_request.as_ref(), Some(p.status.clone())),
                    None => (None, None),
                },
            };
            let progress = run.ticket_progress(id);
            let stage = column(progress.stage);
            let state = progress.state.as_str();
            let card = Card {
                id: id.to_owned(),
                title: plan.map(|p| p.title.clone()).unwrap_or_default(),
                state: state.to_owned(),
                blocked_by: schedule
                    .map(|s| s.blocked_by.clone())
                    .or_else(|| plan.map(|p| p.blocked_by.clone()))
                    .unwrap_or_default(),
                blocker: schedule.and_then(|s| s.blocker.clone()),
                pull_request: pull_request.map(|p| CardPullRequest {
                    number: p.number,
                    url: p.url.clone(),
                }),
                ci,
            };
            if let Some(column) = stages.iter_mut().find(|c| c.stage == stage) {
                column.tickets.push(card);
            }
        }
        Self {
            id: run.id.clone(),
            status: run.status.clone(),
            created_unix_ms: run.created_unix_ms,
            github_repository: run.backlog.as_ref().map(|b| b.github_repository.clone()),
            stages,
            attention: attention(run),
            active: active_tickets(run),
        }
    }
}

/// Tickets holding an implementation slot (implementing, in review or in
/// correction) while the run is recorded as running.
fn active_tickets(run: &Run) -> Vec<String> {
    let Some(scheduler) = run.scheduler.as_ref().filter(|_| run.status == "running") else {
        return Vec::new();
    };
    scheduler
        .active
        .iter()
        .filter(|id| {
            scheduler
                .tickets
                .iter()
                .any(|t| &t.id == *id && t.state == "implementing")
        })
        .cloned()
        .collect()
}

/// Kanban column of a pipeline stage.
fn column(stage: Stage) -> &'static str {
    match stage {
        Stage::Planned => "planned",
        Stage::Implementing => "implementing",
        Stage::Review => "review",
        Stage::Integrating | Stage::Integrated => "integrating",
        Stage::PullRequest => "pr",
        Stage::Ci | Stage::Delivered => "ci",
    }
}

/// Where a dashboard server of the repository is listening, recorded in
/// `.kiln/dashboard.json` while it runs.
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    url: String,
    pid: u32,
}
fn record_path(repository: &Path) -> PathBuf {
    repository.join(".kiln/dashboard.json")
}

/// Records a running dashboard server; the record is removed when dropped.
#[derive(Debug)]
pub struct Announcement {
    path: PathBuf,
}
impl Announcement {
    pub fn new(repository: &Path, url: &str) -> Result<Self> {
        let path = record_path(repository);
        fs::create_dir_all(path.parent().expect("record lives under .kiln"))?;
        let record = Record {
            url: url.to_owned(),
            pid: std::process::id(),
        };
        let temp = path.with_extension(format!("json.{}", std::process::id()));
        fs::write(&temp, serde_json::to_vec(&record)?)?;
        fs::rename(&temp, &path)?;
        Ok(Self { path })
    }
}
impl Drop for Announcement {
    fn drop(&mut self) {
        // Another server may have announced itself since; leave its record.
        let ours = fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
            .is_some_and(|r| r.pid == std::process::id());
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// URL of the repository's running dashboard server, if one is recorded and
/// its process is still alive (a killed server cannot remove its record).
pub fn running_url(repository: &Path) -> Option<String> {
    let record: Record = serde_json::from_slice(&fs::read(record_path(repository)).ok()?).ok()?;
    let pid = libc::pid_t::try_from(record.pid).ok()?;
    // SAFETY: signal 0 only checks that the process exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    alive.then_some(record.url)
}
