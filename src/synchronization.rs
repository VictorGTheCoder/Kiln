//! Synchronization of recorded, verified progress back to imported GitHub issues.
//!
//! Kiln owns exactly one marked progress comment per imported issue and never edits
//! issue titles, bodies or other comments. Progress is derived only from recorded
//! verified results: an agent exiting or a branch being pushed is never completion.
//! Remote edits (of the issue since import, or of Kiln's own comment) are detected
//! and surfaced; the approved repository specs remain the requirements authority.
use crate::execution::git;
use crate::publication::PublicationSettings;
use crate::validation::{ValidationReport, VERIFIED};
use crate::{Engine, Run};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteIssue {
    pub title: String,
    pub body: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    /// Login of the comment author; only Kiln's own identity can own progress.
    #[serde(default)]
    pub author: String,
    pub body: String,
}
/// The GitHub issue operations synchronization relies on. There is deliberately no
/// operation that edits an issue's title or body.
pub trait IssueTracker {
    /// Login of the authenticated identity Kiln writes comments as.
    fn current_user(&self) -> Result<String>;
    fn issue(&self, repository: &str, number: u64) -> Result<RemoteIssue>;
    fn comments(&self, repository: &str, number: u64) -> Result<Vec<IssueComment>>;
    fn create_comment(&self, repository: &str, number: u64, body: &str) -> Result<IssueComment>;
    fn update_comment(&self, repository: &str, id: u64, body: &str) -> Result<()>;
}

/// Durable record of the last synchronization of a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Synchronization {
    pub issues: Vec<IssueSynchronization>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSynchronization {
    pub ticket_id: String,
    pub repository: String,
    pub number: u64,
    /// Progress derived from recorded results, e.g. `verified-on-pull-request`.
    pub state: String,
    /// `pending` (a remote write may be in flight), `synchronized` or `conflict`.
    pub status: String,
    pub comment_id: Option<u64>,
    /// Hash of the comment body Kiln last confirmed on GitHub.
    pub published_sha256: Option<String>,
    /// Hash of a body Kiln attempted to write but has not confirmed.
    pub pending_sha256: Option<String>,
    /// Detected remote edits and divergence, surfaced rather than overwritten.
    #[serde(default)]
    pub divergence: Vec<String>,
}

/// Simulated GitHub issues in the shared GitHub fixture file:
/// `{"user", "issues":[{repository, number, title, body, comments:[{id, author, body}]}],
/// "interrupt"?}` where `user` is the login Kiln writes as.
/// `interrupt` (`before_comment` or `after_comment`) fails the next comment creation
/// at that point once, simulating a process interruption.
pub struct FixtureIssueTracker {
    path: PathBuf,
}
impl FixtureIssueTracker {
    pub fn new(path: &Path) -> Self {
        Self { path: path.into() }
    }
    fn load(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&std::fs::read(&self.path).context("read GitHub fixture")?)
            .context("invalid GitHub fixture")
    }
    fn store(&self, state: &serde_json::Value) -> Result<()> {
        std::fs::write(&self.path, serde_json::to_vec_pretty(state)?)?;
        Ok(())
    }
    fn interrupt(&self, state: &mut serde_json::Value, point: &str) -> Result<()> {
        if state["interrupt"].as_str() == Some(point) {
            state
                .as_object_mut()
                .context("invalid GitHub fixture")?
                .remove("interrupt");
            self.store(state)?;
            bail!("simulated interruption {point}");
        }
        Ok(())
    }
    fn entry<'a>(
        state: &'a mut serde_json::Value,
        repository: &str,
        number: u64,
    ) -> Result<&'a mut serde_json::Value> {
        state["issues"]
            .as_array_mut()
            .context("GitHub fixture has no issues")?
            .iter_mut()
            .find(|i| i["repository"] == repository && i["number"] == number)
            .with_context(|| format!("issue {repository}#{number} missing from fixture"))
    }
}
impl IssueTracker for FixtureIssueTracker {
    fn current_user(&self) -> Result<String> {
        Ok(self.load()?["user"]
            .as_str()
            .context("GitHub fixture has no user")?
            .into())
    }
    fn issue(&self, repository: &str, number: u64) -> Result<RemoteIssue> {
        let mut state = self.load()?;
        let entry = Self::entry(&mut state, repository, number)?;
        Ok(RemoteIssue {
            title: entry["title"].as_str().unwrap_or_default().into(),
            body: entry["body"].as_str().unwrap_or_default().into(),
        })
    }
    fn comments(&self, repository: &str, number: u64) -> Result<Vec<IssueComment>> {
        let mut state = self.load()?;
        let entry = Self::entry(&mut state, repository, number)?;
        Ok(serde_json::from_value(entry["comments"].clone()).unwrap_or_default())
    }
    fn create_comment(&self, repository: &str, number: u64, body: &str) -> Result<IssueComment> {
        let mut state = self.load()?;
        self.interrupt(&mut state, "before_comment")?;
        let id = state["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|i| i["comments"].as_array().cloned().unwrap_or_default())
            .filter_map(|c| c["id"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        let comment = IssueComment {
            id,
            author: self.current_user()?,
            body: body.into(),
        };
        let entry = Self::entry(&mut state, repository, number)?;
        if !entry["comments"].is_array() {
            entry["comments"] = serde_json::json!([]);
        }
        entry["comments"]
            .as_array_mut()
            .context("invalid comments")?
            .push(serde_json::to_value(&comment)?);
        self.store(&state)?;
        self.interrupt(&mut state, "after_comment")?;
        Ok(comment)
    }
    fn update_comment(&self, repository: &str, id: u64, body: &str) -> Result<()> {
        let mut state = self.load()?;
        let comment = state["issues"]
            .as_array_mut()
            .context("GitHub fixture has no issues")?
            .iter_mut()
            .filter(|i| i["repository"] == repository)
            .flat_map(|i| i["comments"].as_array_mut().into_iter().flatten())
            .find(|c| c["id"] == id)
            .context("comment missing from fixture")?;
        comment["body"] = body.into();
        self.store(&state)
    }
}

/// Real GitHub through the authenticated `gh` CLI REST API.
pub struct GitHubIssueTracker {
    pub program: PathBuf,
}
impl Default for GitHubIssueTracker {
    fn default() -> Self {
        Self {
            program: "gh".into(),
        }
    }
}
impl GitHubIssueTracker {
    fn api(&self, args: &[&str], input: Option<&serde_json::Value>) -> Result<serde_json::Value> {
        let mut child = Command::new(&self.program)
            .arg("api")
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("GitHub synchronization requires authenticated gh")?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .context("gh stdin unavailable")?
                .write_all(&serde_json::to_vec(input)?)?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!(
                "GitHub request {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        serde_json::from_slice(&output.stdout).context("invalid GitHub API response")
    }
    fn comment(value: &serde_json::Value) -> Result<IssueComment> {
        Ok(IssueComment {
            id: value["id"].as_u64().context("comment has no id")?,
            author: value["user"]["login"].as_str().unwrap_or_default().into(),
            body: value["body"].as_str().unwrap_or_default().into(),
        })
    }
}
impl IssueTracker for GitHubIssueTracker {
    fn current_user(&self) -> Result<String> {
        let value = self.api(&["--method", "GET", "user"], None)?;
        Ok(value["login"]
            .as_str()
            .filter(|l| !l.is_empty())
            .context("authenticated GitHub user has no login")?
            .into())
    }
    fn issue(&self, repository: &str, number: u64) -> Result<RemoteIssue> {
        let value = self.api(
            &[
                "--method",
                "GET",
                &format!("repos/{repository}/issues/{number}"),
            ],
            None,
        )?;
        if value.get("pull_request").is_some() {
            bail!("#{number} is a pull request, not an issue");
        }
        Ok(RemoteIssue {
            title: value["title"]
                .as_str()
                .context("issue has no title")?
                .into(),
            body: value["body"].as_str().unwrap_or_default().into(),
        })
    }
    fn comments(&self, repository: &str, number: u64) -> Result<Vec<IssueComment>> {
        let mut comments = Vec::new();
        let mut page = 1;
        loop {
            let value = self.api(
                &[
                    "--method",
                    "GET",
                    &format!(
                        "repos/{repository}/issues/{number}/comments?per_page=100&page={page}"
                    ),
                ],
                None,
            )?;
            let listed = value.as_array().context("invalid comment listing")?;
            for comment in listed {
                comments.push(Self::comment(comment)?);
            }
            if listed.len() < 100 {
                return Ok(comments);
            }
            page += 1;
        }
    }
    fn create_comment(&self, repository: &str, number: u64, body: &str) -> Result<IssueComment> {
        let value = self.api(
            &[
                "--method",
                "POST",
                &format!("repos/{repository}/issues/{number}/comments"),
                "--input",
                "-",
            ],
            Some(&serde_json::json!({ "body": body })),
        )?;
        Self::comment(&value)
    }
    fn update_comment(&self, repository: &str, id: u64, body: &str) -> Result<()> {
        self.api(
            &[
                "--method",
                "PATCH",
                &format!("repos/{repository}/issues/comments/{id}"),
                "--input",
                "-",
            ],
            Some(&serde_json::json!({ "body": body })),
        )?;
        Ok(())
    }
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn marker(run_id: &str, ticket_id: &str) -> String {
    format!("<!-- kiln:progress run={run_id} ticket={ticket_id} -->")
}
fn issue_identity(url: &str) -> Option<(String, u64)> {
    let suffix = url.strip_prefix("https://github.com/")?;
    let (repository, number) = suffix.split_once("/issues/")?;
    Some((repository.into(), number.parse().ok()?))
}

/// Progress of one ticket, derived only from recorded results.
struct Progress {
    state: &'static str,
    headline: String,
    completion: String,
    commit: Option<String>,
    report: Option<ValidationReport>,
}

impl Engine {
    fn reachable(&self, commit: &str, target: &str) -> bool {
        git(
            &self.repository,
            &["merge-base", "--is-ancestor", commit, target],
        )
        .is_ok()
    }
    /// The remote target branch tip after fetching it, when publication is configured.
    fn remote_target(&self, settings: Option<&PublicationSettings>) -> Result<Option<String>> {
        let Some(settings) = settings else {
            return Ok(None);
        };
        let reference = format!("refs/heads/{}", settings.target_branch);
        let listed = git(
            &self.repository,
            &["ls-remote", "--", &settings.remote, &reference],
        )?;
        let Some(tip) = listed.split_whitespace().next().map(String::from) else {
            return Ok(None);
        };
        git(
            &self.repository,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--no-write-fetch-head",
                "--",
                &settings.remote,
                &reference,
            ],
        )
        .context("fetch target branch to detect merged work")?;
        Ok(Some(tip))
    }
    fn progress(
        &self,
        run: &Run,
        ticket_id: &str,
        settings: Option<&PublicationSettings>,
        target_tip: Option<&str>,
    ) -> Progress {
        let not_completed = |reason: &str| format!("Completed: no. {reason}");
        let integrated = run
            .integrations
            .iter()
            .rev()
            .find(|i| i.ticket_id == ticket_id && i.status == "integrated")
            .and_then(|i| i.integrated_commit.clone());
        let report = run.validation_reports.last().cloned();
        if let Some(commit) = integrated {
            let covering = report.as_ref().filter(|r| {
                r.integrated_commit
                    .as_deref()
                    .is_some_and(|c| self.reachable(&commit, c))
            });
            let Some(covering) = covering else {
                return Progress {
                    state: "awaiting-validation",
                    headline: "Integrated, awaiting validation".into(),
                    completion: not_completed(
                        "The integrated commit has not been covered by a validation report.",
                    ),
                    commit: Some(commit),
                    report: None,
                };
            };
            if covering.outcome != VERIFIED {
                let (state, headline) = if covering.outcome == crate::validation::FAILED {
                    ("validation-failed", "Validation failed")
                } else {
                    ("unable-to-verify", "Unable to verify")
                };
                return Progress {
                    state,
                    headline: headline.into(),
                    completion: not_completed(&format!(
                        "The latest validation outcome is {}.",
                        covering.outcome
                    )),
                    commit: Some(commit),
                    report: Some(covering.clone()),
                };
            }
            let target = settings
                .map(|s| s.target_branch.as_str())
                .unwrap_or("the primary branch");
            if target_tip.is_some_and(|tip| self.reachable(&commit, tip)) {
                return Progress {
                    state: "merged",
                    headline: format!("Merged into `{target}`"),
                    completion: format!(
                        "Completed: yes. The verified commit is merged into `{target}`."
                    ),
                    commit: Some(commit),
                    report: Some(covering.clone()),
                };
            }
            let published = run.publication.as_ref().filter(|p| {
                p.status == "published"
                    && p.pull_request.is_some()
                    && self.reachable(&commit, &p.published_commit)
            });
            return match published {
                Some(publication) => Progress {
                    state: "verified-on-pull-request",
                    headline: "Verified on pull request".into(),
                    completion: not_completed(&format!(
                        "Verified on pull request branch `{}`; not merged into `{target}`.",
                        publication.branch
                    )),
                    commit: Some(commit),
                    report: Some(covering.clone()),
                },
                None => Progress {
                    state: "verified",
                    headline: "Verified on the integration branch".into(),
                    completion: not_completed(&format!(
                        "Verified but not yet published as a pull request; not merged into `{target}`."
                    )),
                    commit: Some(commit),
                    report: Some(covering.clone()),
                },
            };
        }
        let blocked = |headline: String, reason: &str| Progress {
            state: "blocked",
            headline,
            completion: not_completed(reason),
            commit: None,
            report: None,
        };
        if let Some(attempt) = run
            .integrations
            .iter()
            .rev()
            .find(|i| i.ticket_id == ticket_id)
            .filter(|a| a.status != "integrated")
        {
            return blocked(
                format!("Blocked: integration {}", attempt.status),
                "Integration did not complete with passing combined checks.",
            );
        }
        if let Some(cycle) = run.corrections.iter().rev().find(|c| {
            c.ticket_id == ticket_id && matches!(c.outcome.as_str(), "no-progress" | "exhausted")
        }) {
            return blocked(
                format!("Blocked: correction {}", cycle.outcome),
                "Review findings were not resolved.",
            );
        }
        match run.sessions.iter().rev().find(|s| s.ticket_id == ticket_id) {
            Some(session) if session.status == "failed" => Progress {
                state: "failed",
                headline: "Implementation failed".into(),
                completion: not_completed("The implementation session failed verification."),
                commit: None,
                report: None,
            },
            Some(_) => Progress {
                state: "in-progress",
                headline: "In progress".into(),
                completion: not_completed(
                    "An implementation exists but has not been reviewed, integrated and verified.",
                ),
                commit: None,
                report: None,
            },
            None => {
                let ticket = run
                    .plan
                    .as_ref()
                    .and_then(|p| p.tickets.iter().find(|t| t.id == ticket_id));
                let waiting: Vec<_> = ticket
                    .map(|t| t.blocked_by.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter(|b| {
                        !run.integrations
                            .iter()
                            .any(|i| &i.ticket_id == *b && i.status == "integrated")
                    })
                    .cloned()
                    .collect();
                if waiting.is_empty() {
                    Progress {
                        state: "not-started",
                        headline: "Not started".into(),
                        completion: not_completed("No implementation has been recorded."),
                        commit: None,
                        report: None,
                    }
                } else {
                    blocked(
                        "Blocked by prerequisites".into(),
                        &format!("Waiting for {} to be integrated.", waiting.join(", ")),
                    )
                }
            }
        }
    }

    /// Reflect recorded progress in the progress comment of every imported issue.
    ///
    /// Only comments authored by the identity Kiln writes as can be adopted as its
    /// progress comment. Every state write is a short locked transaction that changes
    /// only this issue's synchronization record, so concurrent Kiln writers keep theirs.
    pub fn synchronize(&self, id: &str, tracker: &dyn IssueTracker) -> Result<Run> {
        let run = self.inspect(id)?;
        if run.imported_issues.is_empty() {
            bail!("run {id} has no imported GitHub issues to synchronize");
        }
        let settings = PublicationSettings::from_config(&run.config)?;
        let target_tip = self.remote_target(settings.as_ref())?;
        let me = tracker.current_user()?;
        let mut latest = run.clone();
        for imported in run.imported_issues.clone() {
            let (repository, number) =
                issue_identity(&imported.url).context("imported issue has an invalid URL")?;
            let ticket_id = format!("github:{repository}#{number}");
            let progress =
                self.progress(&run, &ticket_id, settings.as_ref(), target_tip.as_deref());

            let remote = tracker.issue(&repository, number)?;
            let mut divergence = Vec::new();
            if remote.title != imported.title || remote.body != imported.body {
                divergence.push(format!(
                    "Issue {repository}#{number} was edited on GitHub since import; the approved repository specs remain authoritative and the remote edit was not adopted."
                ));
            }
            let body = self.render(&run, &ticket_id, &progress, settings.as_ref(), &divergence);
            let desired = sha256(&body);

            let mut record = self
                .inspect(id)?
                .synchronization
                .and_then(|s| s.issues.into_iter().find(|i| i.ticket_id == ticket_id))
                .unwrap_or_else(|| IssueSynchronization {
                    ticket_id: ticket_id.clone(),
                    repository: repository.clone(),
                    number,
                    state: progress.state.into(),
                    status: "pending".into(),
                    comment_id: None,
                    published_sha256: None,
                    pending_sha256: None,
                    divergence: Vec::new(),
                });
            record.state = progress.state.into();

            let comments = tracker.comments(&repository, number)?;
            let mark = marker(&run.id, &ticket_id);
            for forged in comments
                .iter()
                .filter(|c| c.author != me && c.body.contains("kiln:progress"))
            {
                divergence.push(format!(
                    "Comment {} on {repository}#{number} by @{} carries a Kiln progress marker but was not written by Kiln's identity @{me}; it was ignored.",
                    forged.id,
                    if forged.author.is_empty() { "unknown" } else { &forged.author }
                ));
            }
            let own: Vec<_> = comments.iter().filter(|c| c.author == me).collect();
            let existing = own
                .iter()
                .find(|c| Some(c.id) == record.comment_id)
                .or_else(|| own.iter().find(|c| c.body.contains(&mark)));
            match existing {
                Some(comment) => {
                    let known = [&record.published_sha256, &record.pending_sha256];
                    let current = sha256(&comment.body);
                    if !known.iter().any(|k| k.as_deref() == Some(current.as_str())) {
                        divergence.push(format!(
                            "Kiln progress comment {} on {repository}#{number} was edited on GitHub; it was not overwritten.",
                            comment.id
                        ));
                        record.comment_id = Some(comment.id);
                        record.status = "conflict".into();
                    } else {
                        if current != desired {
                            record.pending_sha256 = Some(desired.clone());
                            record.status = "pending".into();
                            self.record_synchronization(id, &record)?;
                            tracker.update_comment(&repository, comment.id, &body)?;
                        }
                        record.comment_id = Some(comment.id);
                        record.published_sha256 = Some(desired.clone());
                        record.pending_sha256 = None;
                        record.status = "synchronized".into();
                    }
                }
                None => {
                    if let Some(previous) = record.comment_id {
                        divergence.push(format!(
                            "Kiln progress comment {previous} on {repository}#{number} was removed on GitHub; it was recreated."
                        ));
                    }
                    record.pending_sha256 = Some(desired.clone());
                    record.status = "pending".into();
                    self.record_synchronization(id, &record)?;
                    let created = tracker.create_comment(&repository, number, &body)?;
                    record.comment_id = Some(created.id);
                    record.published_sha256 = Some(desired.clone());
                    record.pending_sha256 = None;
                    record.status = "synchronized".into();
                }
            }
            record.divergence = divergence;
            latest = self.record_synchronization(id, &record)?;
        }
        Ok(latest)
    }

    /// Upsert one issue's record into the current durable run under the run lock.
    fn record_synchronization(&self, id: &str, record: &IssueSynchronization) -> Result<Run> {
        self.transact(id, |latest| {
            let sync = latest.synchronization.get_or_insert_with(Default::default);
            match sync
                .issues
                .iter_mut()
                .find(|i| i.ticket_id == record.ticket_id)
            {
                Some(existing) => *existing = record.clone(),
                None => sync.issues.push(record.clone()),
            }
            Ok(latest.clone())
        })
    }

    fn render(
        &self,
        run: &Run,
        ticket_id: &str,
        progress: &Progress,
        settings: Option<&PublicationSettings>,
        divergence: &[String],
    ) -> String {
        let mut body = format!(
            "{}\n### Kiln progress: {}\n\nTicket `{ticket_id}` in Kiln run `{}`.\n\n- State: `{}`\n- {}\n",
            marker(&run.id, ticket_id),
            progress.headline,
            run.id,
            progress.state,
            progress.completion,
        );
        let repository = settings.map(|s| s.github_repository.as_str());
        if let Some(commit) = &progress.commit {
            body.push_str(&format!("- Integrated commit: `{commit}`"));
            if let Some(repository) = repository {
                body.push_str(&format!(
                    " (https://github.com/{repository}/commit/{commit})"
                ));
            }
            body.push('\n');
        }
        let pull_request = run
            .publication
            .as_ref()
            .filter(|_| progress.report.is_some())
            .and_then(|p| p.pull_request.as_ref());
        if let Some(pr) = pull_request {
            body.push_str(&format!("- Pull request: {}\n", pr.url));
        }
        if let Some(report) = &progress.report {
            body.push_str(&format!(
                "- Validation report `{}`: overall **{}**",
                report.id, report.outcome
            ));
            if let Some(pr) = pull_request {
                body.push_str(&format!(" (evidence: {}#validation-evidence)", pr.url));
            }
            body.push('\n');
            let covers = run
                .plan
                .as_ref()
                .and_then(|p| p.tickets.iter().find(|t| t.id == ticket_id))
                .map(|t| t.covers.clone())
                .unwrap_or_default();
            let criteria: Vec<_> = report
                .criteria
                .iter()
                .filter(|c| covers.contains(&c.id))
                .collect();
            if !criteria.is_empty() {
                body.push_str("- Acceptance criteria:\n");
                for criterion in criteria {
                    body.push_str(&format!(
                        "  - {} `{}`: {}\n",
                        criterion.outcome, criterion.id, criterion.criterion
                    ));
                }
            }
        }
        for note in divergence {
            body.push_str(&format!("- Divergence: {note}\n"));
        }
        run.config.isolation.redact(&body)
    }
}
