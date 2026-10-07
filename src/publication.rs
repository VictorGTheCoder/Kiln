//! Publication of a verified integration branch as a reviewable pull request.
//!
//! Only the exact revision recorded as `verified` by the latest validation report
//! may be published. Publication pushes the dedicated integration branch, never the
//! primary branch, and records remote identity before and after every external
//! effect so an interrupted publication reconciles instead of duplicating.
use crate::execution::git;
use crate::validation::{ValidationReport, VERIFIED};
use crate::{Engine, ProjectConfig, Run};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Project policy from the optional `publication` configuration section.
/// Merging into the primary branch and deployment are disabled unless the
/// project explicitly enables them; opening a pull request never implies either.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationSettings {
    pub github_repository: String,
    pub target_branch: String,
    #[serde(default = "default_remote")]
    pub remote: String,
    #[serde(default)]
    pub merge: bool,
    #[serde(default)]
    pub deploy: bool,
}
fn default_remote() -> String {
    "origin".into()
}
impl PublicationSettings {
    /// `Ok(None)` when the project has not configured publication.
    pub fn from_config(config: &ProjectConfig) -> Result<Option<Self>> {
        let Some(value) = config.extensions.get("publication") else {
            return Ok(None);
        };
        let settings: Self = serde_json::from_value(value.clone()).context(
            "publication must be {github_repository, target_branch, remote?, merge?: bool, deploy?: bool}",
        )?;
        if !valid_repository(&settings.github_repository) {
            bail!("publication.github_repository must be owner/name");
        }
        for (name, value) in [
            ("target_branch", &settings.target_branch),
            ("remote", &settings.remote),
        ] {
            if value.is_empty()
                || value.starts_with('-')
                || value.contains("..")
                || value
                    .bytes()
                    .any(|b| !(b.is_ascii_alphanumeric() || b"-_./".contains(&b)))
            {
                bail!("publication.{name} is not a valid Git name");
            }
        }
        if settings.target_branch.starts_with("kiln/") {
            bail!("publication.target_branch must not be a Kiln-owned branch");
        }
        Ok(Some(settings))
    }
}
pub fn valid_repository(repository: &str) -> bool {
    let pieces: Vec<_> = repository.split('/').collect();
    pieces.len() == 2
        && pieces.iter().all(|p| {
            !p.is_empty()
                && !p.starts_with('.')
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub head: String,
    pub base: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestDraft {
    pub title: String,
    pub head: String,
    pub base: String,
    pub body: String,
    pub draft: bool,
}
/// The GitHub pull request operations publication relies on.
pub trait PullRequestHost {
    fn find_open(&self, repository: &str, head: &str, base: &str) -> Result<Option<PullRequest>>;
    fn create(&self, repository: &str, draft: &PullRequestDraft) -> Result<PullRequest>;
    fn update(&self, repository: &str, number: u64, draft: &PullRequestDraft) -> Result<()>;
    /// Convert an existing ready-for-review pull request to draft and confirm the result.
    fn convert_to_draft(&self, repository: &str, pull_request: &PullRequest) -> Result<bool>;
}

/// Durable record of the remote effects of publishing a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Publication {
    /// `pushing`, `pushed` (branch is remote, no recorded PR yet) or `published`.
    pub status: String,
    pub validation_report: String,
    pub published_commit: String,
    pub github_repository: String,
    pub remote: String,
    pub branch: String,
    pub target_branch: String,
    pub pull_request: Option<PullRequest>,
    /// True when an existing remote pull request was adopted instead of created.
    pub reconciled: bool,
    pub merge_authorized: bool,
    pub deploy_authorized: bool,
}

/// Simulated GitHub stored in a JSON file `{"pull_requests":[...], "interrupt"?}`.
/// `interrupt` (`before_create` or `after_create`) fails the next publication at
/// that point once, simulating a process interruption.
pub struct FixturePullRequests {
    path: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct FixtureState {
    pull_requests: Vec<FixturePullRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interrupt: Option<String>,
    /// Other simulated GitHub state (such as issues) sharing the fixture file.
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}
#[derive(Serialize, Deserialize)]
struct FixturePullRequest {
    repository: String,
    #[serde(default = "open")]
    state: String,
    #[serde(flatten)]
    pull_request: PullRequest,
}
fn open() -> String {
    "open".into()
}
impl FixturePullRequests {
    pub fn new(path: &Path) -> Self {
        Self { path: path.into() }
    }
    fn load(&self) -> Result<FixtureState> {
        serde_json::from_slice(&std::fs::read(&self.path).context("read GitHub fixture")?)
            .context("invalid GitHub fixture")
    }
    fn store(&self, state: &FixtureState) -> Result<()> {
        std::fs::write(&self.path, serde_json::to_vec_pretty(state)?)?;
        Ok(())
    }
    fn interrupt(&self, state: &mut FixtureState, point: &str) -> Result<()> {
        if state.interrupt.as_deref() == Some(point) {
            state.interrupt = None;
            self.store(state)?;
            bail!("simulated interruption {point}");
        }
        Ok(())
    }
}
impl PullRequestHost for FixturePullRequests {
    fn find_open(&self, repository: &str, head: &str, base: &str) -> Result<Option<PullRequest>> {
        Ok(self
            .load()?
            .pull_requests
            .into_iter()
            .find(|p| {
                p.repository == repository
                    && p.state == "open"
                    && p.pull_request.head == head
                    && p.pull_request.base == base
            })
            .map(|p| p.pull_request))
    }
    fn create(&self, repository: &str, draft: &PullRequestDraft) -> Result<PullRequest> {
        let mut state = self.load()?;
        self.interrupt(&mut state, "before_create")?;
        let number = state
            .pull_requests
            .iter()
            .map(|p| p.pull_request.number)
            .max()
            .unwrap_or(0)
            + 1;
        let pull_request = PullRequest {
            number,
            url: format!("https://github.com/{repository}/pull/{number}"),
            head: draft.head.clone(),
            base: draft.base.clone(),
            title: draft.title.clone(),
            body: draft.body.clone(),
            draft: draft.draft,
            node_id: None,
        };
        state.pull_requests.push(FixturePullRequest {
            repository: repository.into(),
            state: open(),
            pull_request: pull_request.clone(),
        });
        self.store(&state)?;
        self.interrupt(&mut state, "after_create")?;
        Ok(pull_request)
    }
    fn update(&self, repository: &str, number: u64, draft: &PullRequestDraft) -> Result<()> {
        let mut state = self.load()?;
        let entry = state
            .pull_requests
            .iter_mut()
            .find(|p| p.repository == repository && p.pull_request.number == number)
            .context("pull request missing from fixture")?;
        entry.pull_request.title = draft.title.clone();
        entry.pull_request.body = draft.body.clone();
        self.store(&state)
    }
    fn convert_to_draft(&self, repository: &str, pull_request: &PullRequest) -> Result<bool> {
        let mut state = self.load()?;
        let entry = state
            .pull_requests
            .iter_mut()
            .find(|p| p.repository == repository && p.pull_request.number == pull_request.number)
            .context("pull request missing from fixture")?;
        entry.pull_request.draft = true;
        self.store(&state)?;
        Ok(true)
    }
}

/// Real GitHub through the authenticated `gh` CLI REST API.
pub struct GitHubPullRequests {
    pub program: PathBuf,
}
impl Default for GitHubPullRequests {
    fn default() -> Self {
        Self {
            program: "gh".into(),
        }
    }
}
impl GitHubPullRequests {
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
            .context("GitHub publication requires authenticated gh")?;
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
    fn parse(value: &serde_json::Value) -> Result<PullRequest> {
        Ok(PullRequest {
            number: value["number"]
                .as_u64()
                .context("pull request has no number")?,
            url: value["html_url"]
                .as_str()
                .context("pull request has no URL")?
                .into(),
            head: value["head"]["ref"].as_str().unwrap_or_default().into(),
            base: value["base"]["ref"].as_str().unwrap_or_default().into(),
            title: value["title"].as_str().unwrap_or_default().into(),
            body: value["body"].as_str().unwrap_or_default().into(),
            draft: value["draft"].as_bool().unwrap_or(false),
            node_id: value["node_id"].as_str().map(String::from),
        })
    }
}
impl PullRequestHost for GitHubPullRequests {
    fn find_open(&self, repository: &str, head: &str, base: &str) -> Result<Option<PullRequest>> {
        let owner = repository.split('/').next().unwrap_or_default();
        let value = self.api(
            &[
                "--method",
                "GET",
                &format!("repos/{repository}/pulls"),
                "-f",
                "state=open",
                "-f",
                &format!("head={owner}:{head}"),
                "-f",
                &format!("base={base}"),
            ],
            None,
        )?;
        let pulls = value.as_array().context("invalid pull request listing")?;
        // The listing filter is trusted only after confirming the exact identity.
        for pull in pulls {
            let pull = Self::parse(pull)?;
            if pull.head == head && pull.base == base {
                return Ok(Some(pull));
            }
        }
        Ok(None)
    }
    fn create(&self, repository: &str, draft: &PullRequestDraft) -> Result<PullRequest> {
        let value = self.api(
            &[
                "--method",
                "POST",
                &format!("repos/{repository}/pulls"),
                "--input",
                "-",
            ],
            Some(&serde_json::json!({
                "title": draft.title, "head": draft.head, "base": draft.base,
                "body": draft.body, "maintainer_can_modify": false, "draft": draft.draft,
            })),
        )?;
        Self::parse(&value)
    }
    fn update(&self, repository: &str, number: u64, draft: &PullRequestDraft) -> Result<()> {
        self.api(
            &[
                "--method",
                "PATCH",
                &format!("repos/{repository}/pulls/{number}"),
                "--input",
                "-",
            ],
            Some(&serde_json::json!({"title": draft.title, "body": draft.body})),
        )?;
        Ok(())
    }
    fn convert_to_draft(&self, _repository: &str, pull_request: &PullRequest) -> Result<bool> {
        let node_id = pull_request
            .node_id
            .as_deref()
            .context("GitHub response omitted pull request node_id; refusing to report a non-draft PR as delivered")?;
        let query = "mutation ConvertPullRequestToDraft($pullRequestId: ID!) { convertPullRequestToDraft(input: {pullRequestId: $pullRequestId}) { pullRequest { id isDraft } } }";
        let value = self.api(
            &[
                "graphql",
                "-f",
                &format!("query={query}"),
                "-F",
                &format!("pullRequestId={node_id}"),
            ],
            None,
        )?;
        let converted = &value["data"]["convertPullRequestToDraft"]["pullRequest"];
        if converted["id"].as_str() != Some(node_id) {
            bail!("GitHub draft conversion response did not match the selected pull request");
        }
        let is_draft = converted["isDraft"]
            .as_bool()
            .context("GitHub draft conversion response did not confirm isDraft")?;
        if !is_draft {
            bail!("GitHub did not convert the existing pull request to draft");
        }
        Ok(true)
    }
}

fn argv(command: &[String]) -> String {
    serde_json::to_string(command).unwrap_or_default()
}
fn describe(run: &Run, report: &ValidationReport, settings: &PublicationSettings) -> String {
    let commit = report.integrated_commit.as_deref().unwrap_or_default();
    let mut body = format!(
        "Kiln run `{}` delivers integrated commit `{commit}`, verified by validation report `{}`.\n\n## Delivered behavior\n\n",
        run.id, report.id
    );
    let integrated: Vec<&str> = run
        .integrations
        .iter()
        .filter(|i| i.status == "integrated")
        .map(|i| i.ticket_id.as_str())
        .collect();
    let tickets = run
        .plan
        .as_ref()
        .map(|p| p.tickets.as_slice())
        .unwrap_or(&[]);
    for ticket in tickets
        .iter()
        .filter(|t| integrated.contains(&t.id.as_str()))
    {
        body.push_str(&format!("- **{}** (`{}`)", ticket.title, ticket.id));
        if let Some(line) = ticket.description.lines().find(|l| !l.trim().is_empty()) {
            body.push_str(&format!(": {}", line.trim()));
        }
        body.push('\n');
        for criterion in &ticket.acceptance_criteria {
            body.push_str(&format!("  - {criterion}\n"));
        }
    }
    body.push_str("\n## Specifications\n\n");
    for spec in &run.effective_specs() {
        body.push_str(&format!(
            "- `{}` (sha256 `{}`{})\n",
            spec.path,
            spec.content_sha256,
            spec.source_revision
                .as_deref()
                .map(|r| format!(", source revision `{r}`"))
                .unwrap_or_default()
        ));
    }
    body.push_str(&format!(
        "\n## Validation evidence\n\nOverall outcome: **{}** for `{commit}`.\n\nGlobal checks:\n\n",
        report.outcome
    ));
    for check in &report.checks {
        body.push_str(&format!(
            "- {}: {} `{}` (exit {})\n",
            check.name,
            if check.passed { "passed" } else { "failed" },
            argv(&check.command),
            check
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".into())
        ));
    }
    body.push_str("\nAcceptance criteria:\n\n");
    for criterion in &report.criteria {
        body.push_str(&format!(
            "- {} `{}`: {}\n",
            criterion.outcome, criterion.id, criterion.criterion
        ));
        for evidence in &criterion.evidence {
            body.push_str(&format!(
                "  - {} check `{}`: {} (exit {})\n",
                evidence.source,
                argv(&evidence.check.command),
                if evidence.check.passed {
                    "passed"
                } else {
                    "failed"
                },
                evidence
                    .check
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "none".into())
            ));
        }
    }
    body.push_str("\n## Limitations\n\n");
    let pending: Vec<_> = tickets
        .iter()
        .filter(|t| !integrated.contains(&t.id.as_str()))
        .collect();
    if pending.is_empty() {
        body.push_str("- Every planned ticket is integrated.\n");
    } else {
        for ticket in pending {
            body.push_str(&format!(
                "- Planned ticket `{}` ({}) is not integrated in this delivery.\n",
                ticket.id, ticket.title
            ));
        }
    }
    body.push_str(&format!(
        "- Evidence covers only the checks listed above, executed in isolated sandboxes against `{commit}`; later commits are not covered.\n"
    ));
    body.push_str(&format!(
        "- Merging into `{}`: {}.\n- Deployment: {}.\n",
        settings.target_branch,
        if settings.merge {
            "explicitly authorized by project configuration"
        } else {
            "not authorized by Kiln; requires explicit project configuration and human review"
        },
        if settings.deploy {
            "explicitly authorized by project configuration"
        } else {
            "not authorized by Kiln; requires explicit project configuration"
        }
    ));
    run.config.isolation.redact(&body)
}

impl Engine {
    /// Push the verified integration branch and open (or reconcile) its pull request.
    pub fn publish(&self, id: &str, host: &dyn PullRequestHost) -> Result<Run> {
        let mut run = self.inspect(id)?;
        let settings = PublicationSettings::from_config(&run.config)?.with_context(|| {
            "publication is not configured; add publication.github_repository and publication.target_branch"
        })?;
        let branch = run
            .integration_branch
            .clone()
            .context("run has no integration branch; only a verified delivery can be published")?;
        let report = run.validation_reports.last().cloned().with_context(|| {
            format!("run {id} has no validation report; only a verified delivery can be published")
        })?;
        if report.outcome != VERIFIED {
            bail!(
                "latest validation is {}; only a verified delivery can be published",
                report.outcome
            );
        }
        if report.input_version != run.input_version() {
            bail!(
                "latest validation covers input version {} but approved specs are at input version {}; run and validate the replanned work before publishing",
                report.input_version,
                run.input_version()
            );
        }
        let tip = git(
            &self.repository,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{branch}^{{commit}}"),
            ],
        )?;
        let commit = report
            .integrated_commit
            .clone()
            .context("verified report has no integrated commit")?;
        if report.integration_branch.as_deref() != Some(branch.as_str()) || commit != tip {
            bail!(
                "integration branch moved since it was verified; validate {tip} before publishing"
            );
        }
        if let Some(previous) = &run.publication {
            if previous.status == "published" && previous.published_commit == commit {
                return Ok(run);
            }
        }
        let mut publication = Publication {
            status: "pushing".into(),
            validation_report: report.id.clone(),
            published_commit: commit.clone(),
            github_repository: settings.github_repository.clone(),
            remote: settings.remote.clone(),
            branch: branch.clone(),
            target_branch: settings.target_branch.clone(),
            pull_request: run
                .publication
                .as_ref()
                .and_then(|p| p.pull_request.clone()),
            reconciled: false,
            merge_authorized: settings.merge,
            deploy_authorized: settings.deploy,
        };
        let _ = self.transact(id, |latest| {
            let current_tip = git(&self.repository, &["rev-parse", "--verify", &format!("refs/heads/{branch}^{{commit}}")])?;
            if latest.input_version() != report.input_version
                || latest.validation_reports.last().map(|r| r.id.as_str()) != Some(report.id.as_str())
                || latest.integration_branch.as_deref() != Some(branch.as_str())
                || latest.validation_reports.last().is_none_or(|r| r.outcome != VERIFIED || r.integration_branch.as_deref() != Some(branch.as_str()) || r.integrated_commit.as_deref() != Some(current_tip.as_str()))
            {
                bail!("verified delivery changed before publication started; validate the current revision before publishing");
            }
            latest.publication = Some(publication.clone());
            Ok(latest.clone())
        })?;

        let remote_ref = format!("refs/heads/{branch}");
        let remote_tip = git(
            &self.repository,
            &["ls-remote", "--", &settings.remote, &remote_ref],
        )?;
        if remote_tip.split_whitespace().next() != Some(commit.as_str()) {
            git(
                &self.repository,
                &[
                    "push",
                    "--porcelain",
                    "--",
                    &settings.remote,
                    &format!("{commit}:{remote_ref}"),
                ],
            )
            .context("push integration branch")?;
        }
        publication.status = "pushed".into();
        run = self.transact(id, |latest| {
            if latest.input_version() != report.input_version
                || latest.validation_reports.last().map(|r| r.id.as_str()) != Some(report.id.as_str())
            {
                bail!("latest validation changed during publication; refusing to record a stale publication");
            }
            latest.publication = Some(publication.clone());
            Ok(latest.clone())
        })?;

        let draft = PullRequestDraft {
            title: format!("Kiln verified delivery: run {}", run.id),
            head: branch.clone(),
            base: settings.target_branch.clone(),
            body: describe(&run, &report, &settings),
            draft: true,
        };
        let pull_request = match host.find_open(
            &settings.github_repository,
            &branch,
            &settings.target_branch,
        )? {
            Some(mut existing) => {
                publication.reconciled = true;
                if existing.body != draft.body || existing.title != draft.title {
                    host.update(&settings.github_repository, existing.number, &draft)?;
                }
                if !existing.draft {
                    if !host.convert_to_draft(&settings.github_repository, &existing)? {
                        bail!("existing pull request was not confirmed as draft; refusing to report publication as delivered");
                    }
                    existing.draft = true;
                }
                PullRequest {
                    title: draft.title.clone(),
                    body: draft.body.clone(),
                    ..existing
                }
            }
            None => host.create(&settings.github_repository, &draft)?,
        };
        if pull_request.head != branch || pull_request.base != settings.target_branch {
            bail!("GitHub returned a pull request for a different branch pair");
        }
        if !pull_request.draft {
            bail!(
                "GitHub pull request is not a draft; refusing to record the issue run as delivered"
            );
        }
        publication.pull_request = Some(pull_request);
        publication.status = "published".into();
        self.transact(id, |latest| {
            let current_tip = git(&self.repository, &["rev-parse", "--verify", &format!("refs/heads/{branch}^{{commit}}")])?;
            if latest.input_version() != report.input_version
                || latest.validation_reports.last().map(|r| r.id.as_str()) != Some(report.id.as_str())
                || latest.validation_reports.last().is_none_or(|r| r.outcome != VERIFIED || r.integrated_commit.as_deref() != Some(current_tip.as_str()))
            {
                bail!("verified delivery changed during publication; refusing to record a stale publication");
            }
            latest.publication = Some(publication);
            Ok(latest.clone())
        })
    }
}
