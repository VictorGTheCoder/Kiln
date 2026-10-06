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
    limits::{LimitExhaustion, LimitState, RunLimits},
    replanning::Replanner,
    review::{FixtureReviewAgent, ReviewAgent},
    Engine, Run,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Condvar, Mutex,
    },
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
    /// Configured limits, observed usage and any run-wide exhaustion.
    #[serde(default)]
    pub limits: Option<crate::limits::LimitState>,
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
    /// Ticket-scoped exhaustion (`correction_cycles`), distinct from run-wide limits.
    #[serde(default)]
    pub exhaustion: Option<String>,
}

/// Why a ticket pipeline ended without integration.
#[derive(Debug)]
enum Halt {
    /// The ticket's own correction allowance is spent.
    CorrectionsExhausted(String),
    /// The ticket still fails after its bounded replanning attempt(s).
    ReplanningExhausted(String),
    /// A run-wide limit stopped further work; the ticket stays resumable.
    Limit(String),
}
impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Halt::CorrectionsExhausted(reason)
            | Halt::ReplanningExhausted(reason)
            | Halt::Limit(reason) => write!(f, "{reason}"),
        }
    }
}
impl std::error::Error for Halt {}
const CORRECTION_CYCLES: &str = "correction_cycles";
const REPLANNING: &str = "replanning";
const RUN_LIMIT: &str = "run_limit";

/// Run-wide limit evaluation shared by the controller and its workers.
struct Gate<'a> {
    limits: RunLimits,
    started: Instant,
    exhausted: Mutex<Option<LimitExhaustion>>,
    providers: &'a dyn TicketProviders,
}
impl Gate<'_> {
    fn exhausted(&self) -> Option<LimitExhaustion> {
        self.exhausted.lock().ok().and_then(|e| e.clone())
    }
    /// Record the first exhaustion; returns the recorded reason.
    fn exhaust(&self, exhaustion: LimitExhaustion) -> String {
        let Ok(mut current) = self.exhausted.lock() else {
            return exhaustion.reason;
        };
        if current.is_none() {
            *current = Some(exhaustion);
            if self.limits.limit_policy == crate::limits::STOP {
                self.providers.stop_active();
            }
        }
        current.as_ref().map(|e| e.reason.clone()).unwrap_or_default()
    }
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
    /// Optional bounded replanning after exhausted correction cycles.
    fn replanner(&self, _ticket: &str) -> Result<Option<Box<dyn Replanner + '_>>> {
        Ok(None)
    }
    /// Cancel every in-flight provider session (the `stop` limit policy).
    fn stop_active(&self) {}
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
    Finished(String, std::result::Result<(), (String, Option<&'static str>)>),
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
                    exhaustion: None,
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
                limits: Some(LimitState::new(RunLimits::from_config(&run.config)?)),
            };
            refresh(&mut state);
            run.status = "running".into();
            run.scheduler = Some(state.clone());
            Ok(state)
        })?;
        let gate = Gate {
            limits: state.limits.as_ref().expect("limits").configured.clone(),
            started: Instant::now(),
            exhausted: Mutex::new(None),
            providers,
        };
        let deadline = gate.limits.duration().map(|d| gate.started + d);
        let integration = Mutex::new(());
        let (sender, receiver) = mpsc::channel::<Event>();
        std::thread::scope(|scope| -> Result<()> {
            let mut workers = 0usize;
            loop {
                // Dispatch the ready frontier in plan order up to the cap, unless a
                // run-wide limit is exhausted: then nothing further starts.
                let mut dispatched = false;
                let exhausted = self.check_limits(id, &gate)?.is_some();
                while !exhausted && state.active.len() < state.implementation_concurrency {
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
                    let gate = &gate;
                    scope.spawn(move || {
                        let result = self.ticket_pipeline(id, &ticket, gate, integration, &sender);
                        // Settle the outcome of revised work; a run-limit stop stays open.
                        let settled = match &result {
                            Ok(()) => Some("integrated"),
                            Err(e) if matches!(e.downcast_ref::<Halt>(), Some(Halt::Limit(_))) => None,
                            Err(_) => Some("blocked"),
                        };
                        let result = match settled.map(|r| self.settle_replanning(id, &ticket, r)) {
                            Some(Err(e)) if result.is_ok() => Err(e),
                            _ => result,
                        };
                        let result = result.map_err(|e| {
                            let kind = match e.downcast_ref::<Halt>() {
                                Some(Halt::CorrectionsExhausted(_)) => Some(CORRECTION_CYCLES),
                                Some(Halt::ReplanningExhausted(_)) => Some(REPLANNING),
                                Some(Halt::Limit(_)) => Some(RUN_LIMIT),
                                None => None,
                            };
                            (format!("{e:#}"), kind)
                        });
                        let _ = sender.send(Event::Finished(ticket, result));
                    });
                }
                if dispatched || exhausted {
                    self.record_schedule(id, &mut state, &gate)?;
                }
                if workers == 0 {
                    break;
                }
                let wait = deadline
                    .filter(|_| gate.exhausted().is_none())
                    .map(|d| d.saturating_duration_since(Instant::now()));
                let event = match wait {
                    None => receiver.recv().context("scheduler worker channel closed")?,
                    Some(left) => match receiver.recv_timeout(left) {
                        Ok(event) => event,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // Duration elapsed mid-session: re-evaluated on next loop.
                            self.check_limits(id, &gate)?;
                            continue;
                        }
                        Err(e) => return Err(e).context("scheduler worker channel closed"),
                    },
                };
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
                            // Stopped by a run-wide limit: resumable, not blocked.
                            Err((_, Some(RUN_LIMIT))) => t.state = "stopped".into(),
                            Err((reason, exhaustion)) => {
                                t.state = "blocked".into();
                                t.blocker = Some(reason);
                                t.exhaustion = exhaustion.map(Into::into);
                            }
                        }
                        refresh(&mut state);
                    }
                }
                self.record_schedule(id, &mut state, &gate)?;
            }
            Ok(())
        })?;
        let exhausted = gate.exhausted();
        for t in &mut state.tickets {
            // After run-wide exhaustion, unstarted tickets stay resumable.
            if t.state == "waiting" && t.blocker.is_none() && exhausted.is_none() {
                t.blocker = Some(format!(
                    "prerequisites {:?} were never integrated",
                    t.waiting_on
                ));
            }
        }
        state.status = if exhausted.is_some() {
            // Never success: completed work and resumable state are preserved.
            "limit_exhausted"
        } else if state.tickets.iter().all(|t| t.state == "integrated") {
            // Ticket integration does not claim global workflow success.
            "awaiting_validation"
        } else {
            "blocked"
        }
        .into();
        self.transact(id, |run| {
            if let Some(limits) = &mut state.limits {
                limits.exhausted = exhausted;
                limits.observe(run);
            }
            run.status = state.status.clone();
            run.scheduler = Some(state.clone());
            Ok(run.clone())
        })
    }

    fn record_schedule(&self, id: &str, state: &mut SchedulerState, gate: &Gate) -> Result<()> {
        self.transact(id, |run| {
            if let Some(limits) = &mut state.limits {
                limits.exhausted = gate.exhausted();
                limits.observe(run);
            }
            run.scheduler = Some(state.clone());
            Ok(())
        })
    }

    /// Evaluate run-wide limits against durable state; the first exhaustion wins
    /// and, under the `stop` policy, cancels active provider sessions.
    fn check_limits(&self, id: &str, gate: &Gate) -> Result<Option<String>> {
        if let Some(e) = gate.exhausted() {
            return Ok(Some(e.reason));
        }
        let usage = crate::limits::account(&self.inspect(id)?);
        Ok(gate
            .limits
            .evaluate(&usage, gate.started.elapsed())
            .map(|e| gate.exhaust(e)))
    }
    /// Pipeline checkpoint: fail with a resumable stop once limits are exhausted.
    fn checkpoint(&self, id: &str, gate: &Gate) -> Result<()> {
        match self.check_limits(id, gate)? {
            Some(reason) => Err(Halt::Limit(reason).into()),
            None => Ok(()),
        }
    }

    /// After correction exhaustion: one bounded replanning attempt while the
    /// ticket's allowance and run-wide limits allow. True when revised work may start.
    fn replan(&self, id: &str, ticket: &str, gate: &Gate, run: &Run) -> Result<bool> {
        if self.replanning_attempts(run, ticket) >= gate.limits.replanning_attempts {
            return Ok(false);
        }
        let Some(replanner) = gate.providers.replanner(ticket)? else {
            return Ok(false);
        };
        self.checkpoint(id, gate)?;
        let attempt = self.replan_ticket(id, ticket, replanner.as_ref())?;
        if attempt.outcome != "replanned" {
            return Err(Halt::ReplanningExhausted(format!(
                "replanning attempt {} for ticket {ticket} {}: {}",
                attempt.attempt,
                attempt.outcome,
                attempt.findings.iter().map(|f| f.message.as_str()).collect::<Vec<_>>().join("; ")
            ))
            .into());
        }
        Ok(true)
    }

    /// One ticket: implement → review → bounded correction → bounded replanning →
    /// serialized integration.
    fn ticket_pipeline(
        &self,
        id: &str,
        ticket: &str,
        gate: &Gate,
        integration: &Mutex<()>,
        events: &mpsc::Sender<Event>,
    ) -> Result<()> {
        let providers = gate.providers;
        // Retained dependent work after a spec replan: revalidate, never re-implement.
        if crate::spec_replanning::pending_revalidation(&self.inspect(id)?, ticket).is_some() {
            self.checkpoint(id, gate)?;
            let revalidation = self.revalidate_ticket(id, ticket)?;
            if revalidation.outcome != "revalidated" {
                bail!(
                    "revalidation of retained work failed: {}",
                    revalidation.failure.unwrap_or_default()
                );
            }
            return Ok(());
        }
        // A replanning attempt recorded before a resume still counts.
        let mut replanned = self
            .inspect(id)?
            .replans
            .iter()
            .any(|r| r.ticket_id == ticket && r.outcome == "replanned");
        let (corrector, session) = loop {
            self.checkpoint(id, gate)?;
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
                self.checkpoint(id, gate)?;
                let reviewer = providers.reviewer(ticket)?;
                run = self.review_ticket(id, ticket, reviewer.as_ref())?;
                session = latest_session(&run, ticket)?;
            }
            let corrector = providers.corrector(ticket)?;
            if session.status != "implemented" || !self.review_gate(&run, &session)? {
                if let Some(corrector) = &corrector {
                    self.checkpoint(id, gate)?;
                    run = self.correct_ticket_within(
                        id,
                        ticket,
                        corrector.as_ref(),
                        corrector.as_ref(),
                        &|| matches!(self.check_limits(id, gate), Ok(None)),
                    )?;
                    session = latest_session(&run, ticket)?;
                }
            }
            if session.status == "implemented" && self.review_gate(&run, &session)? {
                break (corrector, session);
            }
            // A session cancelled or cut short by an exhausted limit is resumable.
            if let Some(e) = gate.exhausted() {
                return Err(Halt::Limit(e.reason).into());
            }
            if replanned {
                return Err(Halt::ReplanningExhausted(format!(
                    "ticket {ticket} still fails after its replanning attempt"
                ))
                .into());
            }
            let exhausted = run
                .corrections
                .iter()
                .rev()
                .find(|c| c.ticket_id == ticket)
                .is_some_and(|c| c.outcome == "exhausted");
            if exhausted {
                if self.replan(id, ticket, gate, &run)? {
                    replanned = true;
                    continue;
                }
                return Err(Halt::CorrectionsExhausted(format!(
                    "correction cycles exhausted for ticket {ticket}"
                ))
                .into());
            }
            bail!(
                "{}",
                session.failure.clone().unwrap_or_else(|| {
                    "implementation did not pass fresh independent review".into()
                })
            );
        };
        let _ = events.send(Event::Implemented(ticket.into(), Some(session.id.clone())));
        let _serialized = integration
            .lock()
            .map_err(|_| anyhow::anyhow!("integration lock poisoned"))?;
        self.checkpoint(id, gate)?;
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
            if let Some(e) = gate.exhausted() {
                return Err(Halt::Limit(e.reason).into());
            }
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
        && crate::spec_replanning::pending_revalidation(run, ticket).is_none()
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
    /// Set by `stop_active`: waiting sessions end as stopped.
    #[serde(skip)]
    stopped: AtomicBool,
    /// Tickets whose replanning ran: later sessions use the replanned responses.
    #[serde(skip)]
    replanned: Mutex<HashSet<String>>,
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
    /// Bounded replanning: `ticket` (revised), `verification` (independent),
    /// and the `implementation` and `review` of the replanned work.
    #[serde(default)]
    replanning: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct ScenarioReplanning {
    ticket: crate::planning::Ticket,
    verification: crate::planning::Verification,
    implementation: serde_json::Value,
    review: serde_json::Value,
}
const RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(30);
impl FixtureScenario {
    pub fn load(path: &Path, repository: &Path) -> Result<Self> {
        let mut scenario: Self = serde_json::from_slice(&std::fs::read(path)?)
            .context("invalid scheduler scenario fixture")?;
        scenario.repository = repository.to_path_buf();
        Ok(scenario)
    }
    fn ensure_running(&self) -> Result<()> {
        if self.stopped.load(Ordering::SeqCst) {
            bail!("session stopped by run limit");
        }
        Ok(())
    }
    /// The ticket's responses; after its replanning, those of the replanned work.
    fn ticket(&self, ticket: &str) -> Result<ScenarioTicket> {
        let mut spec: ScenarioTicket = serde_json::from_value(
            self.tickets
                .get(ticket)
                .with_context(|| {
                    format!("scenario has no deterministic response for ticket {ticket}")
                })?
                .clone(),
        )?;
        let replanned = self.replanned.lock().is_ok_and(|r| r.contains(ticket));
        if let (true, Some(value)) = (replanned, &spec.replanning) {
            let r: ScenarioReplanning = serde_json::from_value(value.clone())
                .context("invalid replanning fixture")?;
            spec.implementation = r.implementation;
            spec.review = r.review;
            spec.corrections = None;
        }
        Ok(spec)
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
            scenario.ensure_running()?;
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
                scenario.ensure_running()?;
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
struct ScenarioReplanner<'a> {
    scenario: &'a FixtureScenario,
    ticket: String,
    spec: ScenarioReplanning,
}
impl crate::replanning::ReplanningAgent for ScenarioReplanner<'_> {
    fn replan(&self, _: &crate::replanning::ReplanRequest) -> Result<crate::planning::Ticket> {
        self.scenario
            .replanned
            .lock()
            .map_err(|_| anyhow::anyhow!("scenario observation poisoned"))?
            .insert(self.ticket.clone());
        Ok(self.spec.ticket.clone())
    }
}
impl crate::planning::PlanningAgent for ScenarioReplanner<'_> {
    fn generate(&self, _: &crate::planning::PlanningRequest) -> Result<Vec<crate::planning::Ticket>> {
        bail!("replanning fixtures do not generate plans")
    }
    fn verify(&self, _: &crate::planning::VerificationRequest) -> Result<crate::planning::Verification> {
        Ok(self.spec.verification.clone())
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
    fn replanner(&self, ticket: &str) -> Result<Option<Box<dyn Replanner + '_>>> {
        Ok(match self.ticket(ticket)?.replanning {
            Some(value) => Some(Box::new(ScenarioReplanner {
                scenario: self,
                ticket: ticket.into(),
                spec: serde_json::from_value(value).context("invalid replanning fixture")?,
            })),
            None => None,
        })
    }
    fn stop_active(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.signal.notify_all();
    }
}

/// Real providers: a fresh Codex adapter (and therefore context and stop handle)
/// for every role of every session. `stop_active` cancels every adapter handed
/// out, and adapters created afterwards start stopped.
pub struct CodexProviders {
    config: crate::codex::CodexConfig,
    stops: Mutex<Vec<std::sync::Arc<AtomicBool>>>,
    stopped: AtomicBool,
    /// Repository and policy for fresh replanning contexts, when enabled.
    replanning: Option<(PathBuf, crate::sandbox::IsolationPolicy)>,
}
impl CodexProviders {
    pub fn new(config: crate::codex::CodexConfig) -> Self {
        Self {
            config,
            stops: Mutex::new(Vec::new()),
            stopped: AtomicBool::new(false),
            replanning: None,
        }
    }
    /// Enable bounded replanning in fresh clones of `repository`.
    pub fn with_replanning(
        mut self,
        repository: PathBuf,
        isolation: crate::sandbox::IsolationPolicy,
    ) -> Self {
        self.replanning = Some((repository, isolation));
        self
    }
    fn adapter(&self) -> crate::codex::CodexAdapter {
        let adapter = crate::codex::CodexAdapter::new(self.config.clone());
        if self.stopped.load(Ordering::SeqCst) {
            adapter.stop();
        }
        if let Ok(mut stops) = self.stops.lock() {
            stops.push(adapter.stop_handle());
        }
        adapter
    }
}
impl TicketProviders for CodexProviders {
    fn implementer(&self, _: &str) -> Result<Box<dyn ImplementationAgent + '_>> {
        Ok(Box::new(self.adapter()))
    }
    fn reviewer(&self, _: &str) -> Result<Box<dyn ReviewAgent + '_>> {
        Ok(Box::new(self.adapter()))
    }
    fn corrector(&self, _: &str) -> Result<Option<Box<dyn Corrector + '_>>> {
        Ok(Some(Box::new(self.adapter())))
    }
    fn replanner(&self, _: &str) -> Result<Option<Box<dyn Replanner + '_>>> {
        Ok(self.replanning.as_ref().map(|(repository, isolation)| {
            Box::new(crate::codex::CodexPlanningAgent {
                adapter: self.adapter(),
                repository: repository.clone(),
                isolation: isolation.clone(),
            }) as Box<dyn Replanner>
        }))
    }
    fn stop_active(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Ok(stops) = self.stops.lock() {
            for stop in stops.iter() {
                stop.store(true, Ordering::SeqCst);
            }
        }
    }
}
