//! Run controller: advances the accepted dependency frontier across specs, runs
//! independent implementation sessions concurrently in fresh contexts and separate
//! worktrees, and serializes integration against the current integration branch.
//!
//! A ticket is released to dependents only after its exact reviewed commit has been
//! integrated and the combined result verified. A blocked ticket keeps every
//! descendant from starting while independent tickets continue.
use crate::{
    correction::{CorrectionAgent, FixtureCorrectionAgent},
    execution::{
        AgentResult, FixtureImplementationAgent, ImplementationAgent, ImplementationRequest,
    },
    review::{FixtureReviewAgent, ReviewAgent},
    Engine, Run,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::{mpsc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub const DEFAULT_IMPLEMENTATION_CONCURRENCY: usize = 3;

/// Durable, presented scheduler state (visible through `kiln inspect` and the web view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerState {
    pub status: String,
    /// Maximum tickets simultaneously in their implementation phase
    /// (implementation, review and correction of that session).
    pub implementation_concurrency: usize,
    /// Tickets currently holding an implementation slot.
    pub active: Vec<String>,
    pub peak_active: usize,
    pub tickets: Vec<TicketSchedule>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketSchedule {
    pub id: String,
    pub blocked_by: Vec<String>,
    /// waiting | implementing | awaiting_integration | integrating | integrated | blocked
    pub state: String,
    /// Prerequisites not yet integrated and verified.
    pub waiting_on: Vec<String>,
    pub blocker: Option<String>,
    pub session_id: Option<String>,
}

/// Something that can both correct a ticket and review the correction.
pub trait Corrector: CorrectionAgent + ReviewAgent {}
impl<T: CorrectionAgent + ReviewAgent> Corrector for T {}

/// Provider factory. Every call returns a fresh agent so each session has its own
/// context and cancellation scope; agents never cross worker threads.
pub trait TicketProviders: Sync {
    fn implementer(&self, ticket: &str) -> Result<Box<dyn ImplementationAgent + '_>>;
    fn reviewer(&self, ticket: &str) -> Result<Box<dyn ReviewAgent + '_>>;
    /// Optional bounded correction for rejected work and integration conflicts.
    fn corrector(&self, ticket: &str) -> Result<Option<Box<dyn Corrector + '_>>>;
}

pub fn implementation_concurrency(run: &Run) -> usize {
    run.config
        .extensions
        .get("implementation_concurrency")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_IMPLEMENTATION_CONCURRENCY)
}

enum Event {
    /// The ticket left its implementation phase and released its slot.
    Implemented(String, Option<String>),
    Integrating(String),
    Finished(String, std::result::Result<(), String>),
}

impl Engine {
    /// Run every accepted ticket to integration or blockage. Returns the final run.
    pub fn run_tickets(&self, id: &str, providers: &dyn TicketProviders) -> Result<Run> {
        let _owner = self.own_run(id)?;
        self.schedule(id, providers, false)
    }

    /// Caller holds run ownership. On resume, tickets recorded as blocked stay blocked.
    pub(crate) fn schedule(
        &self,
        id: &str,
        providers: &dyn TicketProviders,
        resume: bool,
    ) -> Result<Run> {
        let mut state = self.transact(id, |run| {
            let plan = run
                .plan
                .as_ref()
                .filter(|p| p.executable)
                .context("scheduling requires an independently verified executable plan")?;
            let known: BTreeSet<&str> = plan.tickets.iter().map(|t| t.id.as_str()).collect();
            let mut tickets = Vec::new();
            for t in &plan.tickets {
                if let Some(missing) = t.blocked_by.iter().find(|b| !known.contains(b.as_str())) {
                    bail!("ticket {} depends on unknown ticket {missing}", t.id);
                }
                let integrated = integrated(run, &t.id);
                // Resume keeps recorded blockers instead of retrying blocked tickets.
                let blocker = run
                    .scheduler
                    .iter()
                    .flat_map(|s| &s.tickets)
                    .find(|r| resume && r.id == t.id && r.state == "blocked")
                    .map(|r| r.blocker.clone().unwrap_or_default());
                tickets.push(TicketSchedule {
                    id: t.id.clone(),
                    blocked_by: t.blocked_by.clone(),
                    state: if integrated {
                        "integrated"
                    } else if blocker.is_some() {
                        "blocked"
                    } else {
                        "waiting"
                    }
                    .into(),
                    waiting_on: Vec::new(),
                    blocker,
                    session_id: run
                        .sessions
                        .iter()
                        .rev()
                        .find(|s| s.ticket_id == t.id)
                        .map(|s| s.id.clone()),
                });
            }
            let mut state = SchedulerState {
                status: "running".into(),
                implementation_concurrency: implementation_concurrency(run),
                active: Vec::new(),
                peak_active: 0,
                tickets,
            };
            refresh(&mut state);
            run.status = "running".into();
            run.scheduler = Some(state.clone());
            Ok(state)
        })?;
        let integration = Mutex::new(());
        let (sender, receiver) = mpsc::channel::<Event>();
        std::thread::scope(|scope| -> Result<()> {
            let mut workers = 0usize;
            loop {
                // Dispatch the ready frontier in plan order up to the cap.
                let mut dispatched = false;
                while state.active.len() < state.implementation_concurrency {
                    let Some(next) = state.tickets.iter_mut().find(|t| {
                        t.state == "waiting" && t.waiting_on.is_empty() && t.blocker.is_none()
                    }) else {
                        break;
                    };
                    next.state = "implementing".into();
                    let ticket = next.id.clone();
                    state.active.push(ticket.clone());
                    state.peak_active = state.peak_active.max(state.active.len());
                    workers += 1;
                    dispatched = true;
                    let sender = sender.clone();
                    let integration = &integration;
                    scope.spawn(move || {
                        let result =
                            self.ticket_pipeline(id, &ticket, providers, integration, &sender);
                        let _ = sender.send(Event::Finished(
                            ticket,
                            result.map_err(|e| format!("{e:#}")),
                        ));
                    });
                }
                if dispatched {
                    self.record_schedule(id, &state)?;
                }
                if workers == 0 {
                    break;
                }
                let event = receiver.recv().context("scheduler worker channel closed")?;
                match event {
                    Event::Implemented(ticket, session) => {
                        state.active.retain(|t| *t != ticket);
                        let t = find(&mut state, &ticket);
                        t.state = "awaiting_integration".into();
                        t.session_id = session;
                    }
                    Event::Integrating(ticket) => {
                        find(&mut state, &ticket).state = "integrating".into()
                    }
                    Event::Finished(ticket, result) => {
                        workers -= 1;
                        state.active.retain(|t| *t != ticket);
                        let t = find(&mut state, &ticket);
                        match result {
                            Ok(()) => t.state = "integrated".into(),
                            Err(reason) => {
                                t.state = "blocked".into();
                                t.blocker = Some(reason);
                            }
                        }
                        refresh(&mut state);
                    }
                }
                self.record_schedule(id, &state)?;
            }
            Ok(())
        })?;
        for t in &mut state.tickets {
            if t.state == "waiting" && t.blocker.is_none() {
                t.blocker = Some(format!(
                    "prerequisites {:?} were never integrated",
                    t.waiting_on
                ));
            }
        }
        state.status = if state.tickets.iter().all(|t| t.state == "integrated") {
            // Ticket integration does not claim global workflow success.
            "awaiting_validation"
        } else {
            "blocked"
        }
        .into();
        self.transact(id, |run| {
            run.status = state.status.clone();
            run.scheduler = Some(state.clone());
            Ok(run.clone())
        })
    }

    fn record_schedule(&self, id: &str, state: &SchedulerState) -> Result<()> {
        self.transact(id, |run| {
            run.scheduler = Some(state.clone());
            Ok(())
        })
    }

    /// One ticket: implement → review → bounded correction → serialized integration.
    fn ticket_pipeline(
        &self,
        id: &str,
        ticket: &str,
        providers: &dyn TicketProviders,
        integration: &Mutex<()>,
        events: &mpsc::Sender<Event>,
    ) -> Result<()> {
        // Resume continues recorded work: only a ticket without a live attempt is implemented.
        let current = self.inspect(id)?;
        let mut run = match latest_session(&current, ticket) {
            Ok(s) if matches!(s.status.as_str(), "implemented" | "failed") => current,
            _ => {
                let implementer = providers.implementer(ticket)?;
                self.implement_ticket(id, ticket, implementer.as_ref())?
            }
        };
        let mut session = latest_session(&run, ticket)?;
        if session.status == "implemented"
            && !run.reviews.iter().any(|r| r.session_id == session.id)
        {
            let reviewer = providers.reviewer(ticket)?;
            run = self.review_ticket(id, ticket, reviewer.as_ref())?;
            session = latest_session(&run, ticket)?;
        }
        let corrector = providers.corrector(ticket)?;
        if session.status != "implemented" || !self.review_gate(&run, &session)? {
            if let Some(corrector) = &corrector {
                run = self.correct_ticket(id, ticket, corrector.as_ref(), corrector.as_ref())?;
                session = latest_session(&run, ticket)?;
            }
        }
        if session.status != "implemented" || !self.review_gate(&run, &session)? {
            bail!(
                "{}",
                session.failure.clone().unwrap_or_else(|| {
                    "implementation did not pass fresh independent review".into()
                })
            );
        }
        let _ = events.send(Event::Implemented(ticket.into(), Some(session.id.clone())));
        let _serialized = integration
            .lock()
            .map_err(|_| anyhow::anyhow!("integration lock poisoned"))?;
        let _ = events.send(Event::Integrating(ticket.into()));
        let provider = corrector.as_ref().map(|c| {
            (
                c.as_ref() as &dyn CorrectionAgent,
                c.as_ref() as &dyn ReviewAgent,
            )
        });
        let run = self.integrate_ticket(id, ticket, provider)?;
        let attempt = run
            .integrations
            .iter()
            .rev()
            .find(|i| i.ticket_id == ticket)
            .context("missing integration attempt")?;
        if attempt.status != "integrated" {
            bail!(
                "integration {}: {}",
                attempt.status,
                attempt.failure.clone().unwrap_or_default()
            );
        }
        Ok(())
    }
}

/// Latest attempt of a ticket; interrupted attempts are superseded history.
fn latest_session(run: &Run, ticket: &str) -> Result<crate::execution::ImplementationSession> {
    run.sessions
        .iter()
        .rev()
        .find(|s| s.ticket_id == ticket && s.status != "interrupted")
        .cloned()
        .context("missing implementation session")
}
fn integrated(run: &Run, ticket: &str) -> bool {
    run.sessions
        .iter()
        .any(|s| s.ticket_id == ticket && s.status == "integrated" && s.verification_passed)
        && run
            .integrations
            .iter()
            .any(|i| i.ticket_id == ticket && i.status == "integrated")
}
fn find<'a>(state: &'a mut SchedulerState, ticket: &str) -> &'a mut TicketSchedule {
    state
        .tickets
        .iter_mut()
        .find(|t| t.id == ticket)
        .expect("scheduled ticket")
}
/// Recompute waiting prerequisites and propagate blockers to waiting descendants.
fn refresh(state: &mut SchedulerState) {
    let status: BTreeMap<String, String> = state
        .tickets
        .iter()
        .map(|t| (t.id.clone(), t.state.clone()))
        .collect();
    let blocked = blocked_closure(state);
    for t in &mut state.tickets {
        if t.state != "waiting" {
            continue;
        }
        t.waiting_on = t
            .blocked_by
            .iter()
            .filter(|b| status.get(*b).map(String::as_str) != Some("integrated"))
            .cloned()
            .collect();
        if let Some(root) = blocked.get(&t.id) {
            t.blocker = Some(format!("prerequisite {root} is blocked"));
        }
    }
}
/// Waiting tickets transitively depending on a blocked ticket, mapped to a blocked ancestor.
fn blocked_closure(state: &SchedulerState) -> BTreeMap<String, String> {
    let mut result: BTreeMap<String, String> = BTreeMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for t in &state.tickets {
            if result.contains_key(&t.id) || t.state != "waiting" {
                continue;
            }
            let root = t.blocked_by.iter().find_map(|b| {
                let dep = state.tickets.iter().find(|x| x.id == *b)?;
                if dep.state == "blocked" {
                    Some(dep.id.clone())
                } else {
                    result.get(b).cloned()
                }
            });
            if let Some(root) = root {
                result.insert(t.id.clone(), root);
                changed = true;
            }
        }
    }
    result
}

/// Deterministic scenario provider for the CLI boundary. Per ticket:
/// `implementation` (implementation fixture), `review` (both axes), optional
/// `corrections` (correction fixture), and test hooks observed by the agent:
/// `await_started` (tickets whose sessions must be running concurrently),
/// `await_file` (repository-relative release marker) and `require_files`
/// (paths that must exist in the session worktree, i.e. integrated prerequisites).
/// `max_active_implementations` fails any agent that observes more concurrent sessions.
#[derive(Deserialize)]
pub struct FixtureScenario {
    tickets: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    max_active_implementations: Option<usize>,
    #[serde(skip)]
    repository: PathBuf,
    #[serde(skip)]
    observed: Mutex<Observed>,
    #[serde(skip)]
    signal: Condvar,
}
#[derive(Default)]
struct Observed {
    started: HashSet<String>,
    active: usize,
}
#[derive(Deserialize)]
struct ScenarioTicket {
    implementation: serde_json::Value,
    review: serde_json::Value,
    #[serde(default)]
    corrections: Option<serde_json::Value>,
    #[serde(default)]
    await_started: Vec<String>,
    #[serde(default)]
    await_file: Option<String>,
    #[serde(default)]
    require_files: Vec<String>,
}
const RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(30);
impl FixtureScenario {
    pub fn load(path: &Path, repository: &Path) -> Result<Self> {
        let mut scenario: Self = serde_json::from_slice(&std::fs::read(path)?)
            .context("invalid scheduler scenario fixture")?;
        scenario.repository = repository.to_path_buf();
        Ok(scenario)
    }
    fn ticket(&self, ticket: &str) -> Result<ScenarioTicket> {
        Ok(serde_json::from_value(
            self.tickets
                .get(ticket)
                .with_context(|| {
                    format!("scenario has no deterministic response for ticket {ticket}")
                })?
                .clone(),
        )?)
    }
}
struct ScenarioImplementer<'a> {
    scenario: &'a FixtureScenario,
    ticket: String,
    spec: ScenarioTicket,
    inner: FixtureImplementationAgent,
}
struct Active<'a>(&'a FixtureScenario);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        if let Ok(mut observed) = self.0.observed.lock() {
            observed.active -= 1;
        }
        self.0.signal.notify_all();
    }
}
impl ImplementationAgent for ScenarioImplementer<'_> {
    fn implement(&self, request: &ImplementationRequest) -> Result<AgentResult> {
        let scenario = self.scenario;
        // Declared before the mutex guard so it drops after the guard is released.
        let _active;
        let mut observed = scenario
            .observed
            .lock()
            .map_err(|_| anyhow::anyhow!("scenario observation poisoned"))?;
        observed.started.insert(self.ticket.clone());
        observed.active += 1;
        _active = Active(scenario);
        if let Some(limit) = scenario.max_active_implementations {
            if observed.active > limit {
                bail!(
                    "observed {} concurrent implementation sessions; limit {limit}",
                    observed.active
                );
            }
        }
        scenario.signal.notify_all();
        let deadline = Instant::now() + RENDEZVOUS_TIMEOUT;
        while !self
            .spec
            .await_started
            .iter()
            .all(|t| observed.started.contains(t))
        {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!(
                    "tickets {:?} never ran concurrently with {}",
                    self.spec.await_started,
                    self.ticket
                );
            }
            observed = scenario
                .signal
                .wait_timeout(observed, left)
                .map_err(|_| anyhow::anyhow!("scenario observation poisoned"))?
                .0;
        }
        drop(observed);
        if let Some(marker) = &self.spec.await_file {
            let marker = scenario.repository.join(marker);
            while !marker.exists() {
                if Instant::now() > deadline {
                    bail!("release marker {} never appeared", marker.display());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        for file in &self.spec.require_files {
            if !request.worktree.join(file).exists() {
                bail!("prerequisite output {file} is absent from the session worktree");
            }
        }
        self.inner.implement(request)
    }
}
impl TicketProviders for FixtureScenario {
    fn implementer(&self, ticket: &str) -> Result<Box<dyn ImplementationAgent + '_>> {
        let spec = self.ticket(ticket)?;
        let inner = serde_json::from_value(spec.implementation.clone())
            .context("invalid implementation fixture")?;
        Ok(Box::new(ScenarioImplementer {
            scenario: self,
            ticket: ticket.into(),
            spec,
            inner,
        }))
    }
    fn reviewer(&self, ticket: &str) -> Result<Box<dyn ReviewAgent + '_>> {
        let review: FixtureReviewAgent = serde_json::from_value(self.ticket(ticket)?.review)
            .context("invalid review fixture")?;
        Ok(Box::new(review))
    }
    fn corrector(&self, ticket: &str) -> Result<Option<Box<dyn Corrector + '_>>> {
        Ok(match self.ticket(ticket)?.corrections {
            Some(value) => {
                let corrector: FixtureCorrectionAgent =
                    serde_json::from_value(value).context("invalid correction fixture")?;
                Some(Box::new(corrector))
            }
            None => None,
        })
    }
}

/// Real providers: a fresh Codex adapter (and therefore context and stop handle)
/// for every role of every session.
pub struct CodexProviders(pub crate::codex::CodexConfig);
impl TicketProviders for CodexProviders {
    fn implementer(&self, _: &str) -> Result<Box<dyn ImplementationAgent + '_>> {
        Ok(Box::new(crate::codex::CodexAdapter::new(self.0.clone())))
    }
    fn reviewer(&self, _: &str) -> Result<Box<dyn ReviewAgent + '_>> {
        Ok(Box::new(crate::codex::CodexAdapter::new(self.0.clone())))
    }
    fn corrector(&self, _: &str) -> Result<Option<Box<dyn Corrector + '_>>> {
        Ok(Some(Box::new(crate::codex::CodexAdapter::new(
            self.0.clone(),
        ))))
    }
}
