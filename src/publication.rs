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
    #[serde(default)]
    pub required_checks_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub required_checks_poll_seconds: Option<u64>,
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
        if settings.required_checks_timeout_seconds == Some(0)
            || settings.required_checks_poll_seconds == Some(0)
        {
            bail!("publication required check timeout and poll interval must be positive integers");
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredCheck {
    pub name: String,
    /// pending | completed
    pub status: String,
    /// success | failure | cancelled | timed_out | action_required | neutral | skipped
    pub conclusion: Option<String>,
    pub id: Option<String>,
    pub url: Option<String>,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredChecks {
    /// False means no required checks were configured for this pull request base.
    pub configured: bool,
    pub checks: Vec<RequiredCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredChecksOutcome {
    /// no-required-checks | passed | failed | timed-out | unavailable
    pub status: String,
    pub checks: Vec<RequiredCheck>,
    pub failure: Option<String>,
}

/// Poll GitHub's required checks until they pass, fail, or the configured
/// timeout expires. Every observation is tied to the exact commit SHA.
pub fn wait_for_required_checks(
    host: &dyn PullRequestHost,
    repository: &str,
    pull_request: u64,
    commit: &str,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
) -> RequiredChecksOutcome {
    wait_for_required_checks_observed(
        host,
        repository,
        pull_request,
        commit,
        timeout,
        poll_interval,
        |_| Ok(()),
    )
    .unwrap_or_else(|error| RequiredChecksOutcome {
        status: "unavailable".into(),
        checks: Vec::new(),
        failure: Some(format!(
            "could not persist GitHub check observation: {error:#}"
        )),
    })
}

pub fn wait_for_required_checks_observed(
    host: &dyn PullRequestHost,
    repository: &str,
    pull_request: u64,
    commit: &str,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
    mut observe: impl FnMut(&RequiredChecksOutcome) -> Result<()>,
) -> Result<RequiredChecksOutcome> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let result = match host.required_checks(repository, pull_request, commit) {
            Ok(result) => result,
            Err(error) => {
                let outcome = RequiredChecksOutcome {
                    status: "unavailable".into(),
                    checks: Vec::new(),
                    failure: Some(format!(
                        "required GitHub checks could not be read: {error:#}"
                    )),
                };
                observe(&outcome)?;
                return Ok(outcome);
            }
        };
        if !result.configured {
            let outcome = RequiredChecksOutcome {
                status: "no-required-checks".into(),
                checks: Vec::new(),
                failure: None,
            };
            observe(&outcome)?;
            return Ok(outcome);
        }
        if result.checks.iter().any(|check| {
            check.commit != commit
                || (check.status == "completed"
                    && !matches!(
                        check.conclusion.as_deref(),
                        Some("success" | "neutral" | "skipped")
                    ))
        }) {
            let outcome = RequiredChecksOutcome {
                status: "failed".into(),
                checks: result.checks,
                failure: Some(
                    "one or more required checks failed or reported a stale head commit".into(),
                ),
            };
            observe(&outcome)?;
            return Ok(outcome);
        }
        if !result.checks.is_empty()
            && result.checks.iter().all(|check| {
                check.commit == commit
                    && check.status == "completed"
                    && matches!(
                        check.conclusion.as_deref(),
                        Some("success" | "neutral" | "skipped")
                    )
            })
        {
            let outcome = RequiredChecksOutcome {
                status: "passed".into(),
                checks: result.checks,
                failure: None,
            };
            observe(&outcome)?;
            return Ok(outcome);
        }
        if std::time::Instant::now() >= deadline {
            let outcome = RequiredChecksOutcome {
                status: "timed-out".into(),
                checks: result.checks,
                failure: Some(format!(
                    "required GitHub checks did not finish within {} seconds",
                    timeout.as_secs()
                )),
            };
            observe(&outcome)?;
            return Ok(outcome);
        }
        observe(&RequiredChecksOutcome {
            status: "pending".into(),
            checks: result.checks,
            failure: None,
        })?;
        std::thread::sleep(
            poll_interval.min(deadline.saturating_duration_since(std::time::Instant::now())),
        );
    }
}

/// The GitHub pull request operations publication relies on.
pub trait PullRequestHost {
    fn find_open(&self, repository: &str, head: &str, base: &str) -> Result<Option<PullRequest>>;
    fn create(&self, repository: &str, draft: &PullRequestDraft) -> Result<PullRequest>;
    fn update(&self, repository: &str, number: u64, draft: &PullRequestDraft) -> Result<()>;
    /// Convert an existing ready-for-review pull request to draft and confirm the result.
    fn convert_to_draft(&self, repository: &str, pull_request: &PullRequest) -> Result<bool>;
    /// Read required checks for the exact pull request head. The default keeps
    /// third-party adapters source-compatible and explicitly reports no checks.
    fn required_checks(
        &self,
        _repository: &str,
        _pull_request: u64,
        _commit: &str,
    ) -> Result<RequiredChecks> {
        Ok(RequiredChecks {
            configured: false,
            checks: Vec::new(),
        })
    }
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

    fn required_checks(
        &self,
        _repository: &str,
        pull_request: u64,
        commit: &str,
    ) -> Result<RequiredChecks> {
        let mut state = self.load()?;
        let snapshots = state
            .other
            .get("check_snapshots")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let snapshot = if snapshots.is_empty() {
            None
        } else {
            let index = state
                .other
                .get("check_snapshot_index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize;
            state
                .other
                .insert("check_snapshot_index".into(), serde_json::json!(index + 1));
            self.store(&state)?;
            snapshots.get(index.min(snapshots.len() - 1)).cloned()
        };
        let get = |key: &str| {
            snapshot
                .as_ref()
                .and_then(|root| root.get(key))
                .or_else(|| state.other.get(key))
        };
        let configured_names: Vec<String> = get("required_check_names")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect();
        let matching: Vec<_> = get("check_runs")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|check| {
                check["pull_request"].as_u64() == Some(pull_request)
                    && matches!(check["commit"].as_str(), Some(value) if value == commit || value == "current")
            })
            .collect();
        let mut names = configured_names;
        for check in &matching {
            if check["required"].as_bool() == Some(true) {
                if let Some(name) = check["name"].as_str() {
                    names.push(name.into());
                }
            }
        }
        names.sort();
        names.dedup();
        let checks: Vec<RequiredCheck> = names
            .into_iter()
            .map(|name| {
                let record = matching
                    .iter()
                    .find(|check| check["name"].as_str() == Some(name.as_str()));
                RequiredCheck {
                    name,
                    status: record
                        .and_then(|check| check["status"].as_str())
                        .unwrap_or("pending")
                        .into(),
                    conclusion: record
                        .and_then(|check| check["conclusion"].as_str())
                        .map(str::to_owned),
                    id: record.and_then(|check| {
                        check["id"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| check["id"].as_u64().map(|id| id.to_string()))
                    }),
                    url: record
                        .and_then(|check| check["url"].as_str())
                        .map(str::to_owned),
                    commit: commit.into(),
                }
            })
            .collect();
        Ok(RequiredChecks {
            configured: !checks.is_empty(),
            checks,
        })
    }
}

/// Real GitHub through the authenticated `gh` CLI REST API.
pub struct GitHubPullRequests {
    pub program: PathBuf,
}
fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
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

    fn required_checks(
        &self,
        repository: &str,
        pull_request: u64,
        commit: &str,
    ) -> Result<RequiredChecks> {
        let pull = self.api(
            &[
                "--method",
                "GET",
                &format!("repos/{repository}/pulls/{pull_request}"),
            ],
            None,
        )?;
        let actual_head = pull["head"]["sha"]
            .as_str()
            .context("pull request response omitted head commit")?;
        if actual_head != commit {
            bail!("pull request head moved to {actual_head}; required checks for {commit} cannot verify the current PR head");
        }
        let base = pull["base"]["ref"]
            .as_str()
            .context("pull request response omitted base branch")?;
        let base_path = encode_path_segment(base);
        let protection = match self.api(
            &[
                "--method",
                "GET",
                &format!(
                    "repos/{repository}/branches/{base_path}/protection/required_status_checks"
                ),
            ],
            None,
        ) {
            Ok(value) => value,
            Err(error) if format!("{error:#}").contains("404") => serde_json::Value::Null,
            Err(error) => return Err(error).context("read required branch-protection checks"),
        };
        let mut required: Vec<String> = protection["contexts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        required.extend(
            protection["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value["context"].as_str().map(str::to_owned)),
        );
        let effective_rules = self
            .api(
                &[
                    "--method",
                    "GET",
                    &format!("repos/{repository}/rules/branches/{base_path}"),
                ],
                None,
            )
            .context("read effective branch rules")?;
        let effective_rules = effective_rules
            .as_array()
            .context("invalid effective branch-rules response")?;
        for rule in effective_rules {
            if rule["type"].as_str() != Some("required_status_checks") {
                continue;
            }
            let checks = rule["parameters"]["required_status_checks"]
                .as_array()
                .context("required_status_checks rule omitted its required contexts")?;
            for check in checks {
                required.push(
                    check["context"]
                        .as_str()
                        .context("required_status_checks rule contains an unnamed context")?
                        .to_owned(),
                );
            }
        }
        required.sort();
        required.dedup();
        if required.is_empty() {
            return Ok(RequiredChecks {
                configured: false,
                checks: Vec::new(),
            });
        }
        let check_runs = self.api(
            &[
                "--method",
                "GET",
                &format!("repos/{repository}/commits/{commit}/check-runs"),
                "-f",
                "per_page=100",
            ],
            None,
        )?;
        let statuses = self.api(
            &[
                "--method",
                "GET",
                &format!("repos/{repository}/commits/{commit}/status"),
            ],
            None,
        )?;
        let checks = required
            .into_iter()
            .map(|name| {
                let run = check_runs["check_runs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|run| {
                        run["name"].as_str() == Some(name.as_str())
                            && run["head_sha"].as_str() == Some(commit)
                    });
                let status = statuses["statuses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|status| {
                        status["context"].as_str() == Some(name.as_str())
                            && status["sha"].as_str() == Some(commit)
                    });
                if let Some(run) = run {
                    RequiredCheck {
                        name,
                        status: run["status"].as_str().unwrap_or("pending").into(),
                        conclusion: run["conclusion"].as_str().map(str::to_owned),
                        id: run["id"].as_u64().map(|id| id.to_string()),
                        url: run["html_url"].as_str().map(str::to_owned),
                        commit: commit.into(),
                    }
                } else if let Some(status) = status {
                    let state = status["state"].as_str().unwrap_or("pending");
                    RequiredCheck {
                        name,
                        status: if state == "pending" {
                            "pending"
                        } else {
                            "completed"
                        }
                        .into(),
                        conclusion: Some(state.into()),
                        id: status["id"].as_u64().map(|id| id.to_string()),
                        url: status["target_url"].as_str().map(str::to_owned),
                        commit: commit.into(),
                    }
                } else {
                    RequiredCheck {
                        name,
                        status: "pending".into(),
                        conclusion: None,
                        id: None,
                        url: None,
                        commit: commit.into(),
                    }
                }
            })
            .collect();
        Ok(RequiredChecks {
            configured: true,
            checks,
        })
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

fn describe_group(
    run: &Run,
    group: &crate::delivery::DeliveryGroup,
    validation: &crate::delivery::DeliveryValidation,
    ci: Option<&crate::delivery::CiAttempt>,
    settings: &PublicationSettings,
) -> String {
    let mut body = format!(
        "Kiln dependency group `{}` from run `{}` is delivered at `{}`.\n\n## Included issues\n\n",
        group.id, run.id, validation.commit
    );
    if let Some(plan) = &run.plan {
        for ticket in plan
            .tickets
            .iter()
            .filter(|t| group.tickets.contains(&t.id))
        {
            body.push_str(&format!("- **{}** (`{}`)\n", ticket.title, ticket.id));
            for criterion in &ticket.acceptance_criteria {
                body.push_str(&format!("  - {criterion}\n"));
            }
        }
    }
    body.push_str("\n## Prerequisite relationships\n\n");
    if group.dependency_edges.is_empty() {
        body.push_str("- This group has no internal prerequisite edges.\n");
    } else {
        for edge in &group.dependency_edges {
            body.push_str(&format!(
                "- `{}` includes prerequisite `{}` in the same draft PR.\n",
                edge.ticket, edge.prerequisite
            ));
        }
    }
    body.push_str("\n## Group validation\n\n");
    body.push_str(&format!(
        "Outcome: **{}** for `{}`.\n\n",
        validation.outcome, validation.commit
    ));
    for check in &validation.checks {
        body.push_str(&format!(
            "- {}: {} (exit {})\n",
            check.name,
            if check.passed { "passed" } else { "failed" },
            check
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "none".into())
        ));
    }
    for evidence in &validation.criteria {
        body.push_str(&format!(
            "- Requirement `{}` ({}): {}\n",
            evidence.requirement,
            evidence.criterion,
            if evidence.check.passed {
                "passed"
            } else {
                "failed"
            }
        ));
    }
    if let Some(failure) = &validation.failure {
        body.push_str(&format!("\nValidation finding: {failure}\n"));
    }
    if let Some(ci) = ci {
        body.push_str(&format!(
            "\n## Required GitHub checks\n\nOutcome: **{}** for commit `{}`.\n\n",
            ci.status, ci.commit
        ));
        for check in &ci.checks {
            body.push_str(&format!(
                "- {}: {} {}{}\n",
                check.name,
                check.status,
                check.conclusion.as_deref().unwrap_or(""),
                check
                    .url
                    .as_deref()
                    .map(|url| format!(" ([evidence]({url}))"))
                    .unwrap_or_default()
            ));
        }
        if let Some(failure) = &ci.failure {
            body.push_str(&format!("\nCI finding: {failure}\n"));
        }
    }
    body.push_str(&format!(
        "\n## Delivery policy\n\n- Merge into `{}`: {}.\n- Deployment: {}.\n",
        settings.target_branch,
        if settings.merge {
            "authorized by project policy; this command does not merge"
        } else {
            "not authorized"
        },
        if settings.deploy {
            "authorized by project policy; this command does not deploy"
        } else {
            "not authorized"
        },
    ));
    run.config.isolation.redact(&body)
}

impl Engine {
    pub fn block_delivery_groups(&self, id: &str, reason: &str) -> Result<Run> {
        self.transact(id, |run| {
            for group in &mut run.delivery_groups {
                if group.status == "integrated" {
                    group.status = "delivery-blocked".into();
                    group.reason = Some(reason.into());
                }
            }
            Ok(run.clone())
        })
    }

    fn apply_delivery_control(&self, id: &str) -> Result<Option<Run>> {
        let Some(action) = self.requested_control(id)? else {
            return Ok(None);
        };
        let run = self.transact(id, |run| {
            run.status = if action == "pause" {
                "paused"
            } else {
                "cancelled"
            }
            .into();
            for group in &mut run.delivery_groups {
                if group.status == "awaiting-ci" {
                    group.status = "ci-pending".into();
                }
            }
            Ok(run.clone())
        })?;
        self.clear_control(id)?;
        Ok(Some(run))
    }

    /// Publish every independently verified dependency group in a whole-snapshot
    /// backlog. Each group gets its own branch and draft PR; one failed group
    /// never suppresses publication of unrelated verified groups.
    pub fn publish_delivery_groups(&self, id: &str, host: &dyn PullRequestHost) -> Result<Run> {
        self.publish_delivery_groups_with_repair(id, host, None)
    }

    pub fn publish_delivery_groups_with_repair(
        &self,
        id: &str,
        host: &dyn PullRequestHost,
        repair: Option<(
            &dyn crate::correction::CorrectionAgent,
            &dyn crate::review::ReviewAgent,
        )>,
    ) -> Result<Run> {
        let _owner = self.own_run(id)?;
        let initial = self.inspect(id)?;
        let settings = PublicationSettings::from_config(&initial.config)?.with_context(|| {
            "publication is not configured; add publication.github_repository and publication.target_branch"
        })?;
        if initial
            .backlog
            .as_ref()
            .is_none_or(|backlog| backlog.mode != "issue-graph")
        {
            bail!("delivery groups are available only for whole-snapshot backlog runs");
        }
        if initial.delivery_groups.is_empty() {
            bail!(
                "run has no recorded dependency groups; finish or resume backlog scheduling first"
            );
        }
        let timeout =
            std::time::Duration::from_secs(settings.required_checks_timeout_seconds.unwrap_or(600));
        let poll_interval =
            std::time::Duration::from_secs(settings.required_checks_poll_seconds.unwrap_or(5));
        self.transact(id, |run| {
            run.status = "running".into();
            Ok(())
        })?;
        let outcome = (|| -> Result<()> {
            for group in initial.delivery_groups.clone() {
                if self.apply_delivery_control(id)?.is_some() {
                    bail!("delivery interrupted by run control");
                }
                if !matches!(
                    group.status.as_str(),
                    "integrated"
                        | "validating"
                        | "validation-failed"
                        | "validated"
                        | "awaiting-ci"
                        | "ci-failed"
                        | "ci-pending"
                        | "ci-unavailable"
                        | "verified"
                ) {
                    continue;
                }
                let (branch, commit) =
                    crate::delivery::prepare_group_branch(self, &initial, &group)?;
                let run = self.transact(id, |latest| {
                    let current = latest
                        .delivery_groups
                        .iter_mut()
                        .find(|g| g.id == group.id)
                        .context("delivery group disappeared")?;
                    current.branch = Some(branch.clone());
                    current.commit = Some(commit.clone());
                    current.status = "validating".into();
                    Ok(latest.clone())
                })?;
                let current_group = run
                    .delivery_groups
                    .iter()
                    .find(|g| g.id == group.id)
                    .context("delivery group missing")?;
                let validation =
                    crate::delivery::validate_group(self, &run, current_group, &branch, &commit)?;
                let run = self.transact(id, |latest| {
                    let current = latest
                        .delivery_groups
                        .iter_mut()
                        .find(|g| g.id == group.id)
                        .context("delivery group disappeared")?;
                    current.validation = Some(validation.clone());
                    current.status = if validation.outcome == "verified" {
                        "validated"
                    } else {
                        "validation-failed"
                    }
                    .into();
                    current.reason = validation.failure.clone();
                    Ok(latest.clone())
                })?;
                if validation.outcome != "verified" {
                    continue;
                }

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
                    .context("push dependency-group branch")?;
                }
                let draft = PullRequestDraft {
                    title: format!("Kiln dependency group: {}", group.id),
                    head: branch.clone(),
                    base: settings.target_branch.clone(),
                    body: describe_group(&run, &group, &validation, None, &settings),
                    draft: true,
                };
                let pull_request = match host.find_open(
                    &settings.github_repository,
                    &branch,
                    &settings.target_branch,
                )? {
                    Some(mut existing) => {
                        if existing.body != draft.body || existing.title != draft.title {
                            host.update(&settings.github_repository, existing.number, &draft)?;
                        }
                        if !existing.draft {
                            if !host.convert_to_draft(&settings.github_repository, &existing)? {
                                bail!(
                                "existing delivery-group pull request was not confirmed as draft"
                            );
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
                if pull_request.head != branch
                    || pull_request.base != settings.target_branch
                    || !pull_request.draft
                {
                    bail!("GitHub returned a non-draft or mismatched delivery-group pull request");
                }
                self.transact(id, |latest| {
                    let current = latest
                        .delivery_groups
                        .iter_mut()
                        .find(|g| g.id == group.id)
                        .context("delivery group disappeared")?;
                    current.pull_request = Some(pull_request.clone());
                    current.status = "awaiting-ci".into();
                    Ok(latest.clone())
                })?;
                let mut head_commit = commit;
                let mut group_validation = validation;
                let mut pr = pull_request;
                loop {
                    let attempt_id = format!(
                        "{}-ci-{}",
                        group.id,
                        self.inspect(id)?
                            .delivery_groups
                            .iter()
                            .find(|g| g.id == group.id)
                            .map(|g| g.ci_attempts.len() + 1)
                            .unwrap_or(1)
                    );
                    let initial_attempt = crate::delivery::CiAttempt {
                        id: attempt_id.clone(),
                        pull_request: pr.number,
                        commit: head_commit.clone(),
                        observed_unix_ms: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_millis(),
                        status: "pending".into(),
                        checks: Vec::new(),
                        failure: None,
                        observations: Vec::new(),
                    };
                    self.transact(id, |latest| {
                        let current = latest
                            .delivery_groups
                            .iter_mut()
                            .find(|g| g.id == group.id)
                            .context("delivery group disappeared")?;
                        current.ci_attempts.push(initial_attempt.clone());
                        current.status = "awaiting-ci".into();
                        Ok(())
                    })?;
                    let _observed = crate::publication::wait_for_required_checks_observed(
                        host,
                        &settings.github_repository,
                        pr.number,
                        &head_commit,
                        timeout,
                        poll_interval,
                        |observation| {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)?
                                .as_millis();
                            self.transact(id, |latest| {
                                let current = latest
                                    .delivery_groups
                                    .iter_mut()
                                    .find(|g| g.id == group.id)
                                    .context("delivery group disappeared")?;
                                let attempt = current
                                    .ci_attempts
                                    .iter_mut()
                                    .find(|attempt| attempt.id == attempt_id)
                                    .context("CI attempt disappeared")?;
                                attempt.observed_unix_ms = timestamp;
                                attempt.status = observation.status.clone();
                                attempt.checks = observation.checks.clone();
                                attempt.failure = observation.failure.clone();
                                attempt.observations.push(crate::delivery::CiObservation {
                                    observed_unix_ms: timestamp,
                                    status: observation.status.clone(),
                                    checks: observation.checks.clone(),
                                    failure: observation.failure.clone(),
                                });
                                current.status = match observation.status.as_str() {
                                    "passed" | "no-required-checks" => "verified",
                                    "failed" => "ci-failed",
                                    "timed-out" => "ci-pending",
                                    _ => "awaiting-ci",
                                }
                                .into();
                                current.reason = observation.failure.clone();
                                Ok(())
                            })?;
                            if self.apply_delivery_control(id)?.is_some() {
                                bail!("delivery interrupted by run control");
                            }
                            Ok(())
                        },
                    );
                    if let Err(error) = _observed {
                        if matches!(self.inspect(id)?.status.as_str(), "paused" | "cancelled") {
                            bail!("delivery interrupted by run control: {error:#}");
                        }
                        return Err(error);
                    }
                    let run = self.transact(id, |latest| {
                        let current = latest
                            .delivery_groups
                            .iter_mut()
                            .find(|g| g.id == group.id)
                            .context("delivery group disappeared")?;
                        let attempt = current
                            .ci_attempts
                            .iter()
                            .find(|attempt| attempt.id == attempt_id)
                            .context("CI attempt disappeared")?;
                        current.status = match attempt.status.as_str() {
                            "passed" | "no-required-checks" => "verified",
                            "failed" => "ci-failed",
                            "timed-out" => "ci-pending",
                            _ => "ci-unavailable",
                        }
                        .into();
                        current.reason = attempt.failure.clone();
                        Ok(latest.clone())
                    })?;
                    let attempt = run
                        .delivery_groups
                        .iter()
                        .find(|g| g.id == group.id)
                        .and_then(|g| {
                            g.ci_attempts
                                .iter()
                                .find(|attempt| attempt.id == attempt_id)
                        })
                        .cloned()
                        .context("CI attempt disappeared")?;
                    let updated_group = run
                        .delivery_groups
                        .iter()
                        .find(|g| g.id == group.id)
                        .cloned()
                        .context("delivery group missing")?;
                    let body = describe_group(
                        &run,
                        &updated_group,
                        &group_validation,
                        Some(&attempt),
                        &settings,
                    );
                    let updated_draft = PullRequestDraft {
                        body,
                        ..draft.clone()
                    };
                    host.update(&settings.github_repository, pr.number, &updated_draft)?;
                    self.transact(id, |latest| {
                        let current = latest
                            .delivery_groups
                            .iter_mut()
                            .find(|g| g.id == group.id)
                            .context("delivery group disappeared")?;
                        current.pull_request = Some(PullRequest {
                            body: updated_draft.body.clone(),
                            ..pr.clone()
                        });
                        Ok(latest.clone())
                    })?;
                    if attempt.status != "failed" {
                        break;
                    }
                    let limits = crate::limits::RunLimits::from_config(&run.config)?;
                    if updated_group.repair_attempts >= limits.correction_cycles {
                        break;
                    }
                    let Some((corrector, reviewer)) = repair else {
                        break;
                    };
                    if self.apply_delivery_control(id)?.is_some() {
                        bail!("delivery interrupted by run control");
                    }
                    // Count only after the run has durably started a provider call.
                    let started = self.transact(id, |latest| {
                        let current = latest
                            .delivery_groups
                            .iter_mut()
                            .find(|g| g.id == group.id)
                            .context("delivery group disappeared")?;
                        current.repair_attempts += 1;
                        current.status = "repairing".into();
                        Ok(latest.clone())
                    })?;
                    let mut repair_group = started
                        .delivery_groups
                        .iter()
                        .find(|g| g.id == group.id)
                        .cloned()
                        .context("delivery group missing")?;
                    let findings: Vec<String> = attempt
                        .checks
                        .iter()
                        .map(|check| {
                            format!(
                                "{}: status={}, conclusion={}, url={}",
                                check.name,
                                check.status,
                                check.conclusion.as_deref().unwrap_or("pending"),
                                check.url.as_deref().unwrap_or("unavailable")
                            )
                        })
                        .chain(attempt.failure.iter().cloned())
                        .collect();
                    let repaired_commit = match crate::delivery::repair_group(
                        self,
                        &started,
                        &repair_group,
                        &branch,
                        &head_commit,
                        &findings,
                        corrector,
                    ) {
                        Ok(commit) => commit,
                        Err(error) => {
                            self.transact(id, |latest| {
                                let current = latest
                                    .delivery_groups
                                    .iter_mut()
                                    .find(|g| g.id == group.id)
                                    .context("delivery group disappeared")?;
                                current.status = "repair-failed".into();
                                current.reason = Some(format!("CI repair failed: {error:#}"));
                                Ok(())
                            })?;
                            break;
                        }
                    };
                    if self.apply_delivery_control(id)?.is_some() {
                        bail!("delivery interrupted by run control");
                    }
                    repair_group.commit = Some(repaired_commit.clone());
                    repair_group.status = "reviewing-repair".into();
                    let review = match crate::delivery::review_group(
                        self,
                        &started,
                        &repair_group,
                        &repaired_commit,
                        reviewer,
                    ) {
                        Ok(review) => review,
                        Err(error) => {
                            self.transact(id, |latest| {
                                let current = latest
                                    .delivery_groups
                                    .iter_mut()
                                    .find(|g| g.id == group.id)
                                    .context("delivery group disappeared")?;
                                current.status = "review-failed".into();
                                current.reason =
                                    Some(format!("fresh group review failed: {error:#}"));
                                Ok(())
                            })?;
                            break;
                        }
                    };
                    if self.apply_delivery_control(id)?.is_some() {
                        bail!("delivery interrupted by run control");
                    }
                    self.transact(id, |latest| {
                        let current = latest
                            .delivery_groups
                            .iter_mut()
                            .find(|g| g.id == group.id)
                            .context("delivery group disappeared")?;
                        current.commit = Some(repaired_commit.clone());
                        current.reviews.push(review.clone());
                        current.status = if review.passed {
                            "validating-repair"
                        } else {
                            "review-failed"
                        }
                        .into();
                        current.reason = if review.passed {
                            None
                        } else {
                            Some("fresh independent group review rejected the CI repair".into())
                        };
                        Ok(())
                    })?;
                    if !review.passed {
                        break;
                    }
                    let refreshed = self.inspect(id)?;
                    repair_group = refreshed
                        .delivery_groups
                        .iter()
                        .find(|g| g.id == group.id)
                        .cloned()
                        .context("delivery group missing")?;
                    let repaired_validation = crate::delivery::validate_group(
                        self,
                        &refreshed,
                        &repair_group,
                        &branch,
                        &repaired_commit,
                    )?;
                    self.transact(id, |latest| {
                        let current = latest
                            .delivery_groups
                            .iter_mut()
                            .find(|g| g.id == group.id)
                            .context("delivery group disappeared")?;
                        current.validation = Some(repaired_validation.clone());
                        current.status = if repaired_validation.outcome == "verified" {
                            "validated"
                        } else {
                            "validation-failed"
                        }
                        .into();
                        current.reason = repaired_validation.failure.clone();
                        Ok(())
                    })?;
                    if repaired_validation.outcome != "verified" {
                        break;
                    }
                    if self.apply_delivery_control(id)?.is_some() {
                        bail!("delivery interrupted by run control");
                    }
                    let remote_ref = format!("refs/heads/{branch}");
                    git(
                        &self.repository,
                        &[
                            "push",
                            "--porcelain",
                            "--",
                            &settings.remote,
                            &format!("{repaired_commit}:{remote_ref}"),
                        ],
                    )
                    .context("push corrected dependency-group branch")?;
                    let latest = self.inspect(id)?;
                    let mut revised = latest
                        .delivery_groups
                        .iter()
                        .find(|g| g.id == group.id)
                        .cloned()
                        .context("delivery group missing")?;
                    revised.commit = Some(repaired_commit.clone());
                    revised.validation = Some(repaired_validation.clone());
                    let pending_body = describe_group(
                        &latest,
                        &revised,
                        &repaired_validation,
                        Some(&attempt),
                        &settings,
                    );
                    let pending_draft = PullRequestDraft {
                        body: pending_body,
                        ..draft.clone()
                    };
                    host.update(&settings.github_repository, pr.number, &pending_draft)?;
                    pr.body = pending_draft.body;
                    head_commit = repaired_commit;
                    group_validation = repaired_validation;
                }
            }
            Ok(())
        })();
        let latest = self.inspect(id)?;
        if matches!(latest.status.as_str(), "paused" | "cancelled") {
            return Ok(latest);
        }
        self.transact(id, |run| {
            let mut group_results = std::collections::BTreeMap::new();
            for group in &run.delivery_groups {
                let (status, reason) = match group.status.as_str() {
                    "verified" => (
                        "completed",
                        "Implementation, independent review, local validation, and required delivery checks passed.".to_owned(),
                    ),
                    "failed" | "validation-failed" | "ci-failed" | "repair-failed" => (
                        "failed",
                        group.reason.clone().unwrap_or_else(|| {
                            format!("Delivery group ended with status '{}'.", group.status)
                        }),
                    ),
                    "blocked" | "delivery-blocked" => (
                        "blocked",
                        group.reason.clone().unwrap_or_else(|| {
                            format!("Delivery group ended with status '{}'.", group.status)
                        }),
                    ),
                    _ => (
                        "unable-to-verify",
                        group.reason.clone().unwrap_or_else(|| {
                            format!("Delivery group ended with status '{}'; delivery is not verified.", group.status)
                        }),
                    ),
                };
                for ticket in &group.tickets {
                    group_results.insert(ticket.as_str(), (status, reason.clone()));
                }
            }
            if let Some(backlog) = &mut run.backlog {
                let integrated: std::collections::BTreeSet<_> = run
                    .scheduler
                    .as_ref()
                    .into_iter()
                    .flat_map(|scheduler| &scheduler.tickets)
                    .filter(|ticket| ticket.state == "integrated")
                    .map(|ticket| ticket.id.as_str())
                    .collect();
                for disposition in &mut backlog.dispositions {
                    if disposition.kind == "container" {
                        continue;
                    }
                    if let Some((status, reason)) = group_results.get(disposition.issue.as_str()) {
                        if integrated.contains(disposition.issue.as_str()) {
                            disposition.status = (*status).into();
                            disposition.reason = reason.clone();
                        }
                    }
                }
                let actionable: Vec<_> = backlog
                    .dispositions
                    .iter()
                    .filter(|disposition| disposition.kind != "container")
                    .collect();
                run.status = if actionable.iter().all(|disposition| {
                    matches!(disposition.status.as_str(), "completed" | "skipped")
                }) {
                    "completed".into()
                } else if actionable
                    .iter()
                    .any(|disposition| disposition.status == "failed")
                    && actionable
                        .iter()
                        .any(|disposition| disposition.status == "blocked")
                {
                    "blocked".into()
                } else if actionable
                    .iter()
                    .any(|disposition| disposition.status == "completed")
                {
                    "partial".into()
                } else if actionable
                    .iter()
                    .any(|disposition| disposition.status == "blocked")
                {
                    "blocked".into()
                } else if actionable
                    .iter()
                    .any(|disposition| disposition.status == "failed")
                {
                    "failed".into()
                } else {
                    "unable-to-verify".into()
                };
                backlog.outcome = run.status.clone();
                backlog.evidence.push(format!(
                    "Delivery verification finalized with run status '{}'.",
                    run.status
                ));
            }
            Ok(())
        })?;
        if let Err(error) = outcome {
            return Err(error);
        }
        self.inspect(id)
    }

    /// Push the verified integration branch and open (or reconcile) its pull request.
    pub fn publish(&self, id: &str, host: &dyn PullRequestHost) -> Result<Run> {
        let mut run = self.inspect(id)?;
        if run
            .backlog
            .as_ref()
            .is_some_and(|backlog| backlog.mode == "issue-graph")
        {
            return self.publish_delivery_groups(id, host);
        }
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

        let backlog_draft = run.backlog.is_some();
        let draft = PullRequestDraft {
            title: format!("Kiln verified delivery: run {}", run.id),
            head: branch.clone(),
            base: settings.target_branch.clone(),
            body: describe(&run, &report, &settings),
            draft: backlog_draft,
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
                if backlog_draft && !existing.draft {
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
        if backlog_draft && !pull_request.draft {
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
