//! Resume of interrupted runs. Recorded state is reconciled against observed Git
//! and process state before anything is restarted: completed effects (commits,
//! integrations) are adopted rather than repeated, incomplete work is restarted or
//! continued with an explicit reason, and evidence that cannot be confirmed is rerun
//! or marked unable to verify. Every decision is recorded on the run.
use crate::{
    execution::{git, CheckResult},
    scheduler::TicketProviders,
    Engine, Run,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Exit status of an injected interruption.
pub const FAULT_EXIT: i32 = 86;

/// Recovery-test fault injection: when `KILN_FAULT_INJECT` names `point@ticket`
/// (or `point`), end the process abruptly, without destructors or a final save,
/// exactly as a crash would.
pub(crate) fn fault(point: &str, ticket: &str) {
    let Ok(spec) = std::env::var("KILN_FAULT_INJECT") else {
        return;
    };
    if spec
        .split(',')
        .any(|s| s == point || s == format!("{point}@{ticket}"))
    {
        eprintln!("Kiln: injected interruption at {point} for ticket {ticket}");
        std::process::exit(FAULT_EXIT);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recovery {
    pub id: String,
    pub started_unix_ms: u128,
    pub decisions: Vec<RecoveryDecision>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryDecision {
    pub ticket_id: Option<String>,
    /// Session, integration attempt or lock the decision concerns.
    pub subject: String,
    /// preserved | adopted | completed | continued | restarted | rerun | unable-to-verify | released
    pub action: String,
    pub reason: String,
}
fn decision(ticket: Option<&str>, subject: &str, action: &str, reason: String) -> RecoveryDecision {
    RecoveryDecision {
        ticket_id: ticket.map(str::to_owned),
        subject: subject.into(),
        action: action.into(),
        reason,
    }
}

fn clean(worktree: &Path) -> Result<bool> {
    Ok(git(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty())
}
fn check(
    config: &crate::ProjectConfig,
    worktree: &Path,
    name: &str,
    argv: &[String],
) -> CheckResult {
    crate::sandbox::Sandbox::check(&config.isolation, worktree, name, argv)
}

impl Engine {
    /// Reopen an interrupted run: reconcile, record decisions, then continue scheduling.
    pub fn resume(&self, id: &str, providers: &dyn TicketProviders) -> Result<Run> {
        let _owner = self.own_run(id)?;
        self.reconcile(id)?;
        self.schedule(id, providers, true)
    }

    /// Compare recorded state with observed Git state and record the decisions.
    fn reconcile(&self, id: &str) -> Result<()> {
        self.transact(id, |run| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
            let mut decisions = Vec::new();
            // A lock inode persists for process-safe flock. Clear stale metadata only
            // after acquiring it; a live integration keeps the inode and marker intact.
            if self.clear_interrupted_integration_marker(&run.id)? {
                decisions.push(decision(None, "integration.lock", "released", "integration lock was left by this run's interrupted process; no live owner remains".into()));
            }
            let tip = run
                .integration_branch
                .as_ref()
                .and_then(|b| git(&self.repository, &["rev-parse", "--verify", &format!("refs/heads/{b}")]).ok());
            let ancestor = |commit: &str, tip: &str| {
                git(&self.repository, &["merge-base", "--is-ancestor", commit, tip]).is_ok()
            };
            for index in 0..run.integrations.len() {
                let attempt = run.integrations[index].clone();
                if !matches!(attempt.status.as_str(), "running" | "conflicted" | "verified") {
                    continue;
                }
                let candidate = attempt.candidate_commit.clone().unwrap_or_default();
                let verified = attempt.status == "verified";
                let action = match tip.as_deref() {
                    Some(tip) if verified && !candidate.is_empty() && ancestor(&candidate, tip) => Some(("adopted", format!(
                        "integration branch already contains verified candidate {candidate}; the ref update completed before the interruption but was not recorded, so it is adopted, not merged again"
                    ))),
                    // The recorded combined verification applies to exactly this base, which
                    // is unchanged: finish the compare-and-swap ref update it was verified for.
                    Some(tip) if verified && tip == attempt.base_commit && !candidate.is_empty() => {
                        let branch = run.integration_branch.clone().unwrap_or_default();
                        git(&self.repository, &["update-ref", &format!("refs/heads/{branch}"), &candidate, tip])?;
                        Some(("completed", format!(
                            "candidate {candidate} was verified against unchanged base {tip} before the interruption; completed its integration ref update"
                        )))
                    }
                    _ => None,
                };
                if let Some((action, reason)) = action {
                    let target = &mut run.integrations[index];
                    target.status = "integrated".into();
                    target.integrated_commit = Some(candidate.clone());
                    for session in run.sessions.iter_mut().filter(|s| {
                        s.ticket_id == attempt.ticket_id
                            && s.status == "implemented"
                            && (s.id == attempt.session_id || s.commit.as_deref() == Some(&attempt.reviewed_commit))
                    }) {
                        session.status = "integrated".into();
                    }
                    decisions.push(decision(Some(&attempt.ticket_id), &attempt.id, action, reason));
                } else {
                    // Unverified or unconfirmable: never counted. The integration branch is
                    // only moved after verification, so the merge is redone on the current tip.
                    let reason = format!(
                        "integration attempt was interrupted while {} (integration tip {}); its merge and combined checks are not counted, restarting integration of the reviewed commit {}",
                        attempt.status,
                        tip.as_deref().unwrap_or("missing"),
                        attempt.reviewed_commit
                    );
                    let target = &mut run.integrations[index];
                    target.status = "interrupted".into();
                    target.failure = Some(reason.clone());
                    let correction = format!("{}-correction", attempt.id);
                    for session in run.sessions.iter_mut().filter(|s| s.id == correction) {
                        session.status = "interrupted".into();
                        session.failure = Some(reason.clone());
                    }
                    decisions.push(decision(Some(&attempt.ticket_id), &attempt.id, "restarted", reason));
                }
            }
            let config = run.config.clone();
            for session in run.sessions.iter_mut().filter(|s| s.status == "running") {
                let worktree = Path::new(&session.worktree);
                let observed = worktree
                    .exists()
                    .then(|| git(worktree, &["rev-parse", "HEAD"]).ok())
                    .flatten();
                if let Some(head) = observed.as_deref().filter(|h| *h != session.base_commit) {
                    // The commit effect happened but was never recorded. Adopt the exact
                    // commit only when it directly extends the recorded base and the
                    // worktree holds nothing else; its verification evidence was lost,
                    // so rerun it rather than assume it passed.
                    let parent = git(worktree, &["rev-parse", "HEAD^"]).ok();
                    if parent.as_deref() == Some(session.base_commit.as_str()) && clean(worktree)? {
                        session.commit = Some(head.to_owned());
                        session.diff = git(worktree, &["diff", "--binary", &session.base_commit, head])?;
                        session.checks = [("build", &config.build), ("test", &config.test)]
                            .into_iter()
                            .map(|(name, argv)| check(&config, worktree, name, argv))
                            .collect();
                        let passed = session.checks.iter().all(|c| c.passed) && clean(worktree)?;
                        let (action, reason) = if passed {
                            session.status = "implemented".into();
                            session.verification_passed = true;
                            ("adopted", format!("implementation commit {head} was created before the interruption but not recorded; adopted it and reran build and test, which passed"))
                        } else {
                            session.status = "failed".into();
                            session.verification_passed = false;
                            ("rerun", format!("implementation commit {head} was created before the interruption but not recorded; rerunning its verification failed, so it continues through correction"))
                        };
                        session.failure = (!passed).then(|| reason.clone());
                        decisions.push(decision(Some(&session.ticket_id), &session.id, action, reason));
                        continue;
                    }
                }
                let reason = match observed {
                    None => "interrupted before its worktree was observable; restarting in a fresh session".to_owned(),
                    Some(_) => format!(
                        "interrupted before an implementation commit was created; uncommitted changes remain in {} for inspection; restarting in a fresh session and context",
                        session.worktree
                    ),
                };
                session.status = "interrupted".into();
                session.failure = Some(reason.clone());
                decisions.push(decision(Some(&session.ticket_id), &session.id, "restarted", reason));
            }
            let snapshot = run.clone();
            // Blockers recorded before the interruption stand; resume does not retry them.
            let blocked: Vec<(String, String)> = snapshot
                .scheduler
                .iter()
                .flat_map(|s| &s.tickets)
                .filter(|t| t.state == "blocked")
                .map(|t| (t.id.clone(), t.blocker.clone().unwrap_or_default()))
                .collect();
            for (ticket, blocker) in &blocked {
                decisions.push(decision(Some(ticket), ticket, "preserved", format!("recorded blocker is preserved: {blocker}")));
            }
            for integration in snapshot.integrations.iter().filter(|i| i.status == "integrated") {
                if decisions.iter().any(|d| d.subject == integration.id) {
                    continue;
                }
                let commit = integration.integrated_commit.clone().unwrap_or_default();
                if tip.as_deref().is_some_and(|tip| !commit.is_empty() && ancestor(&commit, tip)) {
                    decisions.push(decision(Some(&integration.ticket_id), &integration.id, "preserved", format!(
                        "integrated commit {commit} is contained in the integration branch with recorded combined verification; not repeated"
                    )));
                    continue;
                }
                // Recorded success that Git no longer confirms is never counted.
                let reason = format!(
                    "unable to verify recorded integration {commit}: the integration branch (tip {}) no longer contains it",
                    tip.as_deref().unwrap_or("missing")
                );
                if let Some(target) = run.integrations.iter_mut().find(|i| i.id == integration.id) {
                    target.status = "unable-to-verify".into();
                    target.failure = Some(reason.clone());
                }
                for session in run.sessions.iter_mut().filter(|s| {
                    s.ticket_id == integration.ticket_id && s.status == "integrated"
                }) {
                    session.status = "unable-to-verify".into();
                    session.failure = Some(reason.clone());
                }
                if let Some(t) = run
                    .scheduler
                    .iter_mut()
                    .flat_map(|s| s.tickets.iter_mut())
                    .find(|t| t.id == integration.ticket_id)
                {
                    t.state = "blocked".into();
                    t.blocker = Some(reason.clone());
                }
                decisions.push(decision(Some(&integration.ticket_id), &integration.id, "unable-to-verify", reason));
            }
            // A run-wide limit cancelled these tickets mid-session: the cut-short attempt
            // is not a failure to correct, so it restarts in a fresh session.
            let stopped: Vec<&str> = snapshot
                .scheduler
                .iter()
                .flat_map(|s| &s.tickets)
                .filter(|t| t.state == "stopped")
                .map(|t| t.id.as_str())
                .collect();
            for ticket in stopped {
                let latest = run
                    .sessions
                    .iter_mut()
                    .rev()
                    .find(|s| s.ticket_id == ticket);
                if let Some(session) = latest.filter(|s| {
                    matches!(s.status.as_str(), "failed" | "interrupted")
                }) {
                    let reason = format!(
                        "session was stopped by an exhausted run-wide limit before completing ({}); restarting in a fresh session without charging a correction cycle",
                        session.failure.as_deref().unwrap_or("no recorded failure")
                    );
                    session.status = "interrupted".into();
                    session.failure = Some(reason.clone());
                    decisions.push(decision(Some(ticket), &session.id, "restarted", reason));
                }
            }
            for session in snapshot.sessions.iter().filter(|s| {
                s.status == "implemented" && !blocked.iter().any(|(t, _)| *t == s.ticket_id)
            }) {
                let (action, reason) = if !snapshot.reviews.iter().any(|r| r.session_id == session.id) {
                    let started = std::fs::read_dir(self.repository.join(".kiln/contexts"))
                        .into_iter()
                        .flatten()
                        .flatten()
                        .any(|e| e.file_name().to_string_lossy().starts_with(&format!("{}-review-", session.id)));
                    ("rerun", if started {
                        "review was interrupted before its verdict was recorded; partial review evidence is not counted, rerunning both review axes on the recorded commit".to_owned()
                    } else {
                        "implemented commit has no recorded review; running both review axes".to_owned()
                    })
                } else if self.review_gate(&snapshot, session)? {
                    ("preserved", "recorded review approves the exact implemented commit; continuing to integration".to_owned())
                } else {
                    ("continued", "recorded review does not approve the current commit; continuing bounded correction".to_owned())
                };
                decisions.push(decision(Some(&session.ticket_id), &session.id, action, reason));
            }
            run.recoveries.push(Recovery {
                id: format!("{}-recovery-{}", run.id, run.recoveries.len() + 1),
                started_unix_ms: now,
                decisions,
            });
            Ok(())
        })
    }
}
