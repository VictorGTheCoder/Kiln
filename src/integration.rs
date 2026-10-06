//! Serialized integration of exact reviewed commits; failed combinations never publish.
use crate::{
    correction::{CorrectionAgent, CorrectionRequest},
    execution::{git, CheckResult},
    review::ReviewAgent,
    Engine, Run,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, process::Command};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAttempt {
    pub id: String,
    pub ticket_id: String,
    pub session_id: String,
    pub reviewed_commit: String,
    pub base_commit: String,
    pub candidate_commit: Option<String>,
    pub integrated_commit: Option<String>,
    pub worktree: String,
    pub status: String,
    pub checks: Vec<CheckResult>,
    pub conflicts: Vec<String>,
    pub failure: Option<String>,
}
struct Lock(std::path::PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
/// Replace an attempt by identity; other tickets' records may follow it.
fn record(run: &mut Run, attempt: &IntegrationAttempt) {
    if let Some(existing) = run.integrations.iter_mut().find(|i| i.id == attempt.id) {
        *existing = attempt.clone();
    }
}
struct ConflictCorrection<'a>(&'a dyn CorrectionAgent);
impl CorrectionAgent for ConflictCorrection<'_> {
    fn prepare_redaction(&self) -> Result<()> {
        self.0.prepare_redaction()
    }
    fn redact_output(&self, s: &str) -> String {
        self.0.redact_output(s)
    }
    fn correct(&self, r: &CorrectionRequest) -> Result<crate::execution::AgentResult> {
        let result = self.0.correct(r)?;
        git(&r.worktree, &["add", "-A"])?;
        git(&r.worktree, &["diff", "--cached", "--check"])
            .context("conflict resolution contains conflict markers or invalid whitespace")?;
        if !git(&r.worktree, &["diff", "--name-only", "--diff-filter=U"])?.is_empty() {
            bail!("unresolved conflicts");
        }
        Ok(result)
    }
}
impl Engine {
    /// Scheduler boundary: only `integrated` attempts satisfy prerequisites. The lock
    /// serializes integration and the final ref update also checks the recorded base.
    pub fn integrate_ticket(
        &self,
        id: &str,
        ticket_id: &str,
        provider: Option<(&dyn CorrectionAgent, &dyn ReviewAgent)>,
    ) -> Result<Run> {
        let lock_path = self.repository.join(".kiln/integration.lock");
        let mut owner = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .context(
                "another integration is active; inspect lock before recovering interrupted work",
            )?;
        let _lock = Lock(lock_path);
        // Owning run identity lets resume release a lock its interrupted process left.
        std::io::Write::write_all(&mut owner, id.as_bytes())?;
        if let Some((a, r)) = provider {
            a.prepare_redaction()?;
            r.prepare_redaction()?;
        }
        let mut run = self.inspect(id)?;
        let index = run
            .sessions
            .iter()
            .rposition(|s| s.ticket_id == ticket_id && s.status == "implemented")
            .context("integration requires implemented ticket")?;
        let original = run.sessions[index].clone();
        if !self.review_gate(&run, &original)? {
            bail!("integration requires fresh exact-commit review and verification");
        }
        let branch = run
            .integration_branch
            .clone()
            .context("missing dedicated integration branch")?;
        if branch != format!("kiln/{}/integration", run.id) {
            bail!("invalid dedicated integration branch");
        }
        let base = git(&self.repository, &["rev-parse", &branch])?;
        let commit = original.commit.clone().context("missing reviewed commit")?;
        let attempt_id = format!("{}-integration-{}", run.id, run.integrations.len() + 1);
        let path = self.repository.join(".kiln/worktrees").join(&attempt_id);
        let mut attempt = IntegrationAttempt {
            id: attempt_id.clone(),
            ticket_id: ticket_id.into(),
            session_id: original.id.clone(),
            reviewed_commit: commit.clone(),
            base_commit: base.clone(),
            candidate_commit: None,
            integrated_commit: None,
            worktree: path.to_string_lossy().into(),
            status: "running".into(),
            checks: vec![],
            conflicts: vec![],
            failure: None,
        };
        run.integrations.push(attempt.clone());
        run = self.save_ticket(&run, ticket_id)?;
        let result = (|| -> Result<()> {
            git(
                &self.repository,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    path.to_str().context("worktree UTF-8")?,
                    &base,
                ],
            )?;
            let merged = Command::new("git")
                .args([
                    "-c",
                    "user.name=Kiln",
                    "-c",
                    "user.email=kiln@localhost",
                    "merge",
                    "--no-ff",
                    "--no-edit",
                    &commit,
                ])
                .current_dir(&path)
                .output()?;
            if !merged.status.success() {
                attempt.conflicts = git(&path, &["diff", "--name-only", "--diff-filter=U"])?
                    .lines()
                    .map(str::to_owned)
                    .collect();
                if attempt.conflicts.is_empty() {
                    bail!("merge failed: {}", String::from_utf8_lossy(&merged.stderr));
                }
                // Preserve conflict evidence before handing control to bounded correction.
                attempt.status = "conflicted".into();
                record(&mut run, &attempt);
                run = self.save_ticket(&run, ticket_id)?;
                let (agent, reviewer) = provider.context(
                    "integration conflict requires correction provider; branch remains unchanged",
                )?;
                let mut correction = original.clone();
                correction.id = format!("{attempt_id}-correction");
                correction.context_id = correction.id.clone();
                correction.worktree = path.to_string_lossy().into();
                correction.branch = String::new();
                correction.base_commit = base.clone();
                correction.status = "failed".into();
                correction.commit = None;
                correction.verification_passed = false;
                correction.failure=Some(format!("Resolve integration conflicts against {base}: {}. Preserve both tickets and rerun verification.",attempt.conflicts.join(", ")));
                correction.checks.clear();
                let correction_id = correction.id.clone();
                run.sessions.push(correction);
                run = self.save_ticket(&run, ticket_id)?;
                run = self.correct_ticket(id, ticket_id, &ConflictCorrection(agent), reviewer)?;
                // Concurrent sessions may have been appended; target the exact identity.
                let corrected = run
                    .sessions
                    .iter()
                    .find(|s| s.id == correction_id)
                    .context("missing corrected session")?;
                if !self.review_gate(&run, corrected)? {
                    bail!("conflict correction did not pass fresh review");
                }
                attempt.session_id = corrected.id.clone();
            }
            crate::recovery::fault("integration.after_merge", ticket_id);
            let candidate = git(&path, &["rev-parse", "HEAD"])?;
            attempt.candidate_commit = Some(candidate.clone());
            for (name, argv) in [("build", &run.config.build), ("test", &run.config.test)] {
                let result =
                    crate::sandbox::Sandbox::command(&run.config.isolation, &path, name, argv, &[])
                        .and_then(|mut c| Ok(c.output()?));
                let mut check = match result {
                    Ok(o) => CheckResult {
                        name: name.into(),
                        command: argv.clone(),
                        exit_code: o.status.code(),
                        stdout: String::from_utf8_lossy(&o.stdout).into(),
                        stderr: String::from_utf8_lossy(&o.stderr).into(),
                        passed: o.status.success(),
                    },
                    Err(e) => CheckResult {
                        name: name.into(),
                        command: argv.clone(),
                        exit_code: None,
                        stdout: String::new(),
                        stderr: e.to_string(),
                        passed: false,
                    },
                };
                let redact = |s: &str| {
                    let s = run.config.isolation.redact(s);
                    if let Some((a, r)) = provider {
                        r.redact_output(&a.redact_output(&s))
                    } else {
                        s
                    }
                };
                check.stdout = redact(&check.stdout);
                check.stderr = redact(&check.stderr);
                attempt.checks.push(check);
            }
            if attempt.checks.iter().any(|c| !c.passed) {
                bail!("combined verification failed");
            }
            if !git(&path, &["status", "--porcelain", "--untracked-files=all"])?.is_empty()
                || git(&path, &["rev-parse", "HEAD"])? != candidate
            {
                bail!("combined verification mutated candidate");
            }
            if !self.review_gate(&run, &original)? {
                bail!("reviewed implementation changed during integration");
            }
            // Persist the verified candidate before publishing its Git reference.
            // Resume can reconcile this exact intent against the integration ref.
            attempt.status = "verified".into();
            record(&mut run, &attempt);
            run = self.save_ticket(&run, ticket_id)?;
            crate::recovery::fault("integration.before_update_ref", ticket_id);
            git(
                &self.repository,
                &[
                    "update-ref",
                    &format!("refs/heads/{branch}"),
                    &candidate,
                    &base,
                ],
            )?;
            crate::recovery::fault("integration.after_update_ref", ticket_id);
            attempt.integrated_commit = Some(candidate);
            attempt.status = "integrated".into();
            run.sessions[index].status = "integrated".into();
            if attempt.session_id != original.id {
                if let Some(corrected) =
                    run.sessions.iter_mut().find(|s| s.id == attempt.session_id)
                {
                    corrected.status = "integrated".into();
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            attempt.status = "failed".into();
            let mut failure = run.config.isolation.redact(&format!("{e:#}"));
            if let Some((a, r)) = provider {
                failure = r.redact_output(&a.redact_output(&failure));
            }
            attempt.failure = Some(failure);
        }
        record(&mut run, &attempt);
        run = self.save_ticket(&run, ticket_id)?;
        Ok(run)
    }
}
