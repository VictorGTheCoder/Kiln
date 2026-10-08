use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use kiln::Engine;
use std::path::PathBuf;
mod interrupt;

#[derive(Parser)]
#[command(
    name = "kiln",
    about = "Plan and deliver the open GitHub issues of a repository with coding agents"
)]
struct Cli {
    /// Target repository (any directory inside its Git worktree).
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Snapshot open issues and take one independent issue through planning, delivery gates and draft PR publication.
    #[command(hide = true)]
    StartIssue {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        github_repo: String,
        #[arg(long)]
        issue: u64,
        #[arg(long)]
        issue_fixture: Option<PathBuf>,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        planning_fixture: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["claude", "planning_fixture"])]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        run_fixture: Option<PathBuf>,
        #[arg(long)]
        verifier: Option<PathBuf>,
        #[arg(long, conflicts_with = "gh")]
        publication_fixture: Option<PathBuf>,
        #[arg(long)]
        gh: Option<PathBuf>,
    },
    /// Freeze and plan the open issue graph, schedule it, then deliver verified groups and wait for CI.
    #[command(hide = true)]
    StartBacklog {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        github_repo: String,
        #[arg(long)]
        issue_fixture: Option<PathBuf>,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        planning_fixture: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["claude", "planning_fixture"])]
        codex: Option<PathBuf>,
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["codex", "claude"])]
        run_fixture: Option<PathBuf>,
        #[arg(long, conflicts_with = "gh")]
        publication_fixture: Option<PathBuf>,
        #[arg(long)]
        gh: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["codex", "claude"])]
        repair_fixture: Option<PathBuf>,
        #[arg(long)]
        plan_only: bool,
    },
    /// Read selected GitHub issues and independently verify against frozen specs.
    #[command(hide = true)]
    Import {
        id: String,
        #[arg(long)]
        github_repo: String,
        #[arg(long, required = true)]
        issue: Vec<u64>,
        #[arg(long)]
        fixture: Option<PathBuf>,
        #[arg(long)]
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        verification_fixture: Option<PathBuf>,
        #[arg(long, conflicts_with = "verification_fixture")]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with_all = ["codex", "verification_fixture"])]
        claude: Option<PathBuf>,
    },
    /// Validate configuration and freeze approved Markdown specs. No commands are executed.
    #[command(hide = true)]
    Prepare {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, required = true)]
        spec: Vec<PathBuf>,
    },
    /// Freeze and plan the open issue graph of this repository, then stop before delivery.
    Plan {
        /// Plan an already prepared spec run instead (internal pipeline step; JSON output).
        #[arg(hide = true)]
        id: Option<String>,
        /// Planning fixture for a prepared spec run (internal).
        #[arg(long, hide = true, requires = "id", conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        /// Project configuration [default: kiln.json in the repository].
        #[arg(long, conflicts_with = "id")]
        config: Option<PathBuf>,
        /// GitHub repository OWNER/REPO [default: parsed from the `origin` remote].
        #[arg(long, conflicts_with = "id")]
        github_repo: Option<String>,
        /// Codex executable [default: the config `agent`, else codex then claude on PATH].
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with = "id")]
        issue_fixture: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with_all = ["id", "codex", "claude"])]
        planning_fixture: Option<PathBuf>,
        /// Print the recorded run as JSON instead of a readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Plan the open issue graph (or reuse an unchanged plan), deliver it and wait for CI.
    Start {
        /// Project configuration [default: kiln.json in the repository].
        #[arg(long)]
        config: Option<PathBuf>,
        /// GitHub repository OWNER/REPO [default: parsed from the `origin` remote].
        #[arg(long)]
        github_repo: Option<String>,
        /// Codex executable [default: the config `agent`, else codex then claude on PATH].
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
        /// Plan anew even when the latest plan matches the open issues.
        #[arg(long)]
        fresh: bool,
        /// Print the recorded run as JSON instead of a readable summary.
        #[arg(long)]
        json: bool,
        #[arg(long, hide = true)]
        issue_fixture: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with_all = ["codex", "claude"])]
        planning_fixture: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with_all = ["codex", "claude"])]
        run_fixture: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with = "gh")]
        publication_fixture: Option<PathBuf>,
        #[arg(long, hide = true)]
        gh: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with_all = ["codex", "claude"])]
        repair_fixture: Option<PathBuf>,
    },
    /// Execute one eligible ticket in an isolated worktree.
    #[command(hide = true)]
    Implement {
        id: String,
        ticket: String,
        #[arg(long)]
        #[arg(required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Review an exact implementation commit in independent contexts.
    #[command(hide = true)]
    Review {
        id: String,
        ticket: String,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    #[command(hide = true)]
    Correct {
        id: String,
        ticket: String,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Resolve an ambiguity autonomously by the decision hierarchy (product
    /// objectives > architectural decisions > specs) and record the decision.
    #[command(hide = true)]
    Decide {
        id: String,
        /// Ambiguity description: {id, question, positions:[{source, reference, statement}]}.
        #[arg(long)]
        ambiguity: PathBuf,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Apply revised approved specs explicitly: capture them as a new input version,
    /// independently reverify the revised plan, invalidate affected work and evidence,
    /// and keep unaffected work. Spec edits never reach a run without this operation.
    #[command(hide = true)]
    Replan {
        id: String,
        /// Approved spec (a frozen input of the run) whose current content to apply.
        #[arg(long, required = true)]
        spec: Vec<PathBuf>,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Integrate an exact reviewed change and verify the combined result.
    #[command(hide = true)]
    Integrate {
        id: String,
        ticket: String,
        #[arg(long, conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Schedule every accepted ticket: concurrent independent sessions, prerequisite
    /// ordering across specs, serialized verified integration.
    #[command(hide = true)]
    Run {
        id: String,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Continue the latest paused or interrupted run without repeating completed work.
    Resume {
        /// Resume this run instead (internal pipeline form; JSON output).
        #[arg(hide = true)]
        id: Option<String>,
        #[arg(long, hide = true, alias = "run-fixture", conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with = "gh")]
        publication_fixture: Option<PathBuf>,
        #[arg(long, hide = true)]
        gh: Option<PathBuf>,
        #[arg(long, hide = true, conflicts_with_all = ["codex", "claude"])]
        repair_fixture: Option<PathBuf>,
        /// Print the recorded run as JSON instead of a readable summary.
        #[arg(long, conflicts_with = "id")]
        json: bool,
    },
    /// Pause the latest active run once its active ticket work settles.
    Pause {
        #[arg(hide = true)]
        id: Option<String>,
    },
    /// Stop the latest active run's work safely; no further tickets start.
    Cancel {
        #[arg(hide = true)]
        id: Option<String>,
    },
    /// Validate the integrated revision against acceptance workflows per criterion.
    #[command(hide = true)]
    Validate {
        id: String,
        /// Independent verifier acceptance tests: {"acceptance_checks":[{criterion, command}]}.
        #[arg(long)]
        verifier: Option<PathBuf>,
    },
    /// Push the verified integration branch and open or reconcile its pull request.
    /// Merging and deployment stay disabled unless the project configures them.
    #[command(hide = true)]
    Publish {
        id: String,
        /// Simulated GitHub state file (no network access).
        #[arg(long, conflicts_with = "gh")]
        fixture: Option<PathBuf>,
        /// gh program used for real GitHub access (default: gh on PATH).
        #[arg(long)]
        gh: Option<PathBuf>,
        /// Deterministic correction and fresh-review sequence for delivery-group CI failures.
        #[arg(long, conflicts_with_all = ["codex", "claude"])]
        repair_fixture: Option<PathBuf>,
        /// Codex executable used for bounded delivery-group CI repair.
        #[arg(long, conflicts_with = "claude")]
        codex: Option<PathBuf>,
        /// Claude Code executable used for bounded delivery-group CI repair.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Reflect recorded, verified progress in a Kiln progress comment on each imported
    /// issue. Issue titles, bodies and other comments are never modified.
    #[command(hide = true)]
    Sync {
        id: String,
        /// Simulated GitHub state file (no network access).
        #[arg(long, conflicts_with = "gh")]
        fixture: Option<PathBuf>,
        /// gh program used for real GitHub access (default: gh on PATH).
        #[arg(long)]
        gh: Option<PathBuf>,
    },
    /// Show verified, failed and unable-to-verify criteria of the latest validation.
    #[command(hide = true)]
    Report { id: String },
    /// Read a recorded run, or list all run identities.
    #[command(hide = true)]
    Inspect { id: Option<String> },
    /// Show recorded state on a loopback-only local web server.
    #[command(hide = true)]
    Serve {
        #[arg(long, default_value = "127.0.0.1:3000")]
        bind: std::net::SocketAddr,
        /// Enable the local start-backlog action with this repository config.
        #[arg(long, requires = "github_repo")]
        backlog_config: Option<PathBuf>,
        /// GitHub repository whose complete open issue graph the UI will run.
        #[arg(long, requires = "backlog_config")]
        github_repo: Option<String>,
        #[arg(long, requires = "backlog_config")]
        issue_fixture: Option<PathBuf>,
        #[arg(long, requires = "backlog_config", conflicts_with_all = ["codex", "claude"])]
        planning_fixture: Option<PathBuf>,
        #[arg(long, requires = "backlog_config", conflicts_with_all = ["codex", "claude"])]
        run_fixture: Option<PathBuf>,
        #[arg(long, requires = "backlog_config", conflicts_with = "claude")]
        codex: Option<PathBuf>,
        #[arg(long, requires = "backlog_config", conflicts_with = "codex")]
        claude: Option<PathBuf>,
        #[arg(long, requires = "backlog_config", conflicts_with = "gh")]
        publication_fixture: Option<PathBuf>,
        #[arg(long, requires = "backlog_config")]
        gh: Option<PathBuf>,
        #[arg(long, requires = "backlog_config", conflicts_with_all = ["codex", "claude"])]
        repair_fixture: Option<PathBuf>,
    },
}
/// Real provider selected by `--codex PATH` or `--claude PATH` (mutually exclusive).
enum Provider {
    Codex(Option<PathBuf>),
    Claude(Option<PathBuf>),
}
impl Provider {
    fn select(codex: Option<PathBuf>, claude: Option<PathBuf>) -> Self {
        match claude {
            Some(path) => Self::Claude(Some(path)),
            None => Self::Codex(codex),
        }
    }
}
/// Bind `$a` to a fresh adapter of the selected provider and evaluate `$body`.
macro_rules! with_adapter {
    ($provider:expr, $config:expr, |$a:ident| $body:expr) => {
        match $provider {
            Provider::Codex(path) => {
                let $a = kiln::codex::CodexAdapter::new(kiln::codex::CodexConfig::from_project(
                    $config, path,
                )?);
                $body
            }
            Provider::Claude(path) => {
                let $a = kiln::claude::ClaudeAdapter::new(
                    kiln::claude::ClaudeConfig::from_project($config, path)?,
                );
                $body
            }
        }
    };
}
/// Bind `$a` to fresh planning contexts (disposable clones) of the selected provider.
macro_rules! with_planner {
    ($provider:expr, $engine:expr, $config:expr, |$a:ident| $body:expr) => {
        with_adapter!($provider, &$config, |adapter| {
            let $a = kiln::agent::PlanningContexts {
                adapter,
                repository: $engine.repository.clone(),
                isolation: $config.isolation.clone(),
            };
            $body
        })
    };
}
/// How a command prints the run it recorded.
enum Report {
    /// The recorded run as pretty JSON (hidden commands and `--json`).
    Json,
    /// Readable summary of a planned backlog run.
    Plan { provider: Option<&'static str> },
    /// Readable summary of a delivered (or stopped) backlog run.
    Start { provider: Option<&'static str> },
}
impl Report {
    fn emit(&self, run: &kiln::Run) -> Result<()> {
        match self {
            Self::Json => println!("{}", serde_json::to_string_pretty(run)?),
            Self::Plan { provider } => print!("{}", plan_summary(run, *provider)),
            Self::Start { provider } => print!("{}", start_summary(run, *provider)),
        }
        Ok(())
    }
}
/// Human-readable summary of a backlog run that was planned and not delivered.
fn plan_summary(run: &kiln::Run, provider: Option<&str>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let repository = run
        .backlog
        .as_ref()
        .map_or("the repository", |b| b.github_repository.as_str());
    let open = run.backlog.as_ref().map_or(0, |b| b.issue_snapshot.len());
    let by = provider.map(|p| format!(" with {p}")).unwrap_or_default();
    let _ = writeln!(out, "Run {} for {repository}{by}", run.id);
    match &run.plan {
        Some(plan) if plan.executable => {
            let _ = writeln!(
                out,
                "Planned {} ticket(s) from {open} open issue(s):",
                plan.tickets.len()
            );
            for ticket in &plan.tickets {
                let after = if ticket.blocked_by.is_empty() {
                    String::new()
                } else {
                    format!("  (after {})", ticket.blocked_by.join(", "))
                };
                let _ = writeln!(out, "  {}  {}{after}", ticket.id, ticket.title);
            }
        }
        Some(plan) => {
            let _ = writeln!(out, "Plan rejected:");
            for finding in &plan.findings {
                let _ = writeln!(out, "  {}: {}", finding.code, finding.message);
            }
        }
        None if run.status == "completed" => {
            let _ = writeln!(
                out,
                "No new actionable open issues among {open}; nothing to plan."
            );
        }
        None => {
            let _ = writeln!(out, "No plan was recorded (status: {}).", run.status);
        }
    }
    let unplanned: Vec<_> = run
        .backlog
        .iter()
        .flat_map(|b| &b.dispositions)
        .filter(|d| !d.plan_candidate && d.status != "completed")
        .collect();
    if !unplanned.is_empty() {
        let _ = writeln!(out, "Not planned:");
        for d in unplanned {
            let _ = writeln!(out, "  {}  {}: {}", d.issue, d.status, d.reason);
        }
    }
    if run.plan.as_ref().is_some_and(|plan| plan.executable) {
        let _ = writeln!(
            out,
            "Delivery was not started. Run `kiln start` to deliver this plan."
        );
    }
    out
}
/// Human-readable outcome of a backlog delivery run.
fn start_summary(run: &kiln::Run, provider: Option<&str>) -> String {
    use std::fmt::Write;
    let Some(scheduler) = &run.scheduler else {
        // Nothing was delivered: no actionable issues, or a rejected plan.
        return plan_summary(run, provider).replace(
            "Delivery was not started. Run `kiln start` to deliver this plan.\n",
            "",
        );
    };
    let mut out = String::new();
    let repository = run
        .backlog
        .as_ref()
        .map_or("the repository", |b| b.github_repository.as_str());
    let by = provider.map(|p| format!(" with {p}")).unwrap_or_default();
    let _ = writeln!(out, "Run {} for {repository}{by}: {}", run.id, run.status);
    let titles: std::collections::BTreeMap<_, _> = run
        .plan
        .iter()
        .flat_map(|plan| &plan.tickets)
        .map(|ticket| (ticket.id.as_str(), ticket.title.as_str()))
        .collect();
    let _ = writeln!(out, "Tickets:");
    for ticket in &scheduler.tickets {
        let title = titles.get(ticket.id.as_str()).copied().unwrap_or_default();
        let blocker = ticket
            .blocker
            .as_ref()
            .map(|b| format!(" ({b})"))
            .unwrap_or_default();
        let _ = writeln!(out, "  {}  {title}: {}{blocker}", ticket.id, ticket.state);
    }
    if !run.delivery_groups.is_empty() {
        let _ = writeln!(out, "Delivery:");
        for group in &run.delivery_groups {
            let pull_request = group
                .pull_request
                .as_ref()
                .map(|pr| format!("  {}", pr.url))
                .unwrap_or_default();
            let reason = group
                .reason
                .as_ref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  {}: {}{reason}{pull_request}",
                group.tickets.join(", "),
                group.status
            );
        }
    }
    out
}
/// Inputs of a whole-graph backlog run (`start-backlog`, `plan`, `start`).
struct BacklogArgs {
    config: PathBuf,
    github_repo: String,
    issue_fixture: Option<PathBuf>,
    planning_fixture: Option<PathBuf>,
    codex: Option<PathBuf>,
    claude: Option<PathBuf>,
    run_fixture: Option<PathBuf>,
    publication_fixture: Option<PathBuf>,
    gh: Option<PathBuf>,
    repair_fixture: Option<PathBuf>,
    plan_only: bool,
    /// Deliver the latest planned-but-not-started run when the open issues are unchanged.
    reuse_plan: bool,
}
/// Issue source that serves one snapshot taken up front, so the reuse check
/// and planning see the same open issues.
struct FrozenIssues(
    Vec<kiln::import::ImportedIssue>,
    Box<dyn kiln::import::IssueSource>,
);
impl kiln::import::IssueSource for FrozenIssues {
    fn selected(
        &self,
        repository: &str,
        numbers: &[u64],
    ) -> Result<Vec<kiln::import::ImportedIssue>> {
        self.1.selected(repository, numbers)
    }
    fn snapshot_open(&self, _repository: &str) -> Result<Vec<kiln::import::ImportedIssue>> {
        Ok(self.0.clone())
    }
}
/// Freeze and plan the open issue graph, schedule it, then deliver verified groups
/// and wait for CI (or stop after planning when `plan_only`).
fn start_backlog(engine: &Engine, args: BacklogArgs, report: &Report) -> Result<()> {
    let BacklogArgs {
        config,
        github_repo,
        issue_fixture,
        planning_fixture,
        codex,
        claude,
        run_fixture,
        publication_fixture,
        gh,
        repair_fixture,
        plan_only,
        reuse_plan,
    } = args;
    let completed_issues = engine.completed_backlog_issues(&github_repo)?;
    let source: Box<dyn kiln::import::IssueSource> = match issue_fixture {
        Some(path) => Box::new(kiln::import::FixtureIssues::load(
            &engine.repository.join(path),
        )?),
        None => Box::new(kiln::import::GitHubIssues),
    };
    let source: Box<dyn kiln::import::IssueSource> = if reuse_plan {
        let snapshot = source.snapshot_open(&github_repo)?;
        Box::new(FrozenIssues(snapshot, source))
    } else {
        source
    };
    let reused = if reuse_plan {
        let snapshot = source.snapshot_open(&github_repo)?;
        engine.planned_backlog_run_for_snapshot(&github_repo, &snapshot)?
    } else {
        None
    };
    let run = if let Some(run) = reused {
        eprintln!(
            "Open issues are unchanged since run {} was planned; delivering that plan (pass --fresh to plan again).",
            run.id
        );
        run
    } else {
        let mut run = if let Some(path) = &planning_fixture {
            let agent = kiln::planning::FixtureAgent::load(&engine.repository.join(path))?;
            engine.prepare_backlog_graph(
                &config,
                &github_repo,
                source.as_ref(),
                &agent,
                &completed_issues,
            )?
        } else {
            let project_config =
                kiln::ProjectConfig::load(&engine.repository.join(&config), &engine.repository)?;
            with_planner!(
                Provider::select(codex.clone(), claude.clone()),
                engine,
                project_config,
                |agent| {
                    engine.prepare_backlog_graph(
                        &config,
                        &github_repo,
                        source.as_ref(),
                        &agent,
                        &completed_issues,
                    )?
                }
            )
        };
        if run.plan.is_some()
            && run
                .backlog
                .as_ref()
                .is_some_and(|backlog| backlog.mode == "issue-graph")
        {
            report.emit(&run)?;
            return Ok(());
        }
        if run.backlog.as_ref().is_some_and(|backlog| {
            backlog.mode == "issue-graph"
                && backlog.dispositions.iter().any(|d| d.status == "completed")
                && backlog
                    .dispositions
                    .iter()
                    .all(|d| matches!(d.status.as_str(), "completed" | "skipped"))
        }) {
            if let Some(backlog) = &mut run.backlog {
                backlog.outcome = "completed".into();
                backlog.decisions.push("No new actionable open issues were present; planning, execution, and delivery were skipped.".into());
            }
            run.status = "completed".into();
            engine.save(&run)?;
            report.emit(&run)?;
            return Ok(());
        }
        let id = run.id.clone();
        run = if let Some(path) = &planning_fixture {
            let agent = kiln::planning::FixtureAgent::load(&engine.repository.join(path))?;
            let agent = kiln::backlog::BacklogPlanningAgent { inner: &agent };
            engine.plan(&id, &agent)?
        } else {
            let config = engine.inspect(&id)?.config;
            with_planner!(
                Provider::select(codex.clone(), claude.clone()),
                engine,
                config,
                |agent| {
                    let agent = kiln::backlog::BacklogPlanningAgent { inner: &agent };
                    engine.plan(&id, &agent)?
                }
            )
        };
        engine.record_backlog_plan(&id, &mut run)?;
        if !run.plan.as_ref().is_some_and(|plan| plan.executable) {
            report.emit(&run)?;
            anyhow::bail!("backlog graph plan rejected; inspect durable per-issue findings");
        }
        if plan_only {
            report.emit(&run)?;
            return Ok(());
        }
        run
    };
    let id = run.id.clone();
    if interrupt::requested() {
        // Planning finished: the first safe point of a foreground delivery. The
        // paused plan is delivered by `kiln resume`.
        let run = engine.transact(&id, |stored| {
            stored.status = "paused".into();
            Ok(stored.clone())
        })?;
        return paused(&run, report);
    }
    deliver(
        engine,
        &id,
        Delivery {
            run_fixture,
            codex,
            claude,
            publication_fixture,
            gh,
            repair_fixture,
            resume: false,
        },
        report,
    )
}
/// Inputs of the delivery phase of a backlog run: scheduling then publication.
struct Delivery {
    run_fixture: Option<PathBuf>,
    codex: Option<PathBuf>,
    claude: Option<PathBuf>,
    publication_fixture: Option<PathBuf>,
    gh: Option<PathBuf>,
    repair_fixture: Option<PathBuf>,
    /// Reconcile and continue an interrupted or paused run instead of starting it.
    resume: bool,
}
/// Report a run stopped by a pause request (Ctrl-C, `kiln pause`) and exit cleanly.
fn paused(run: &kiln::Run, report: &Report) -> Result<()> {
    report.emit(run)?;
    eprintln!(
        "Run {} paused; nothing was left half-applied. Run `kiln resume` to continue it.",
        run.id
    );
    Ok(())
}
/// Schedule (or resume) the tickets of a planned backlog run, then publish its
/// verified delivery groups and wait for CI. A pause or cancellation stops
/// before publication.
fn deliver(engine: &Engine, id: &str, args: Delivery, report: &Report) -> Result<()> {
    let Delivery {
        run_fixture,
        codex,
        claude,
        publication_fixture,
        gh,
        repair_fixture,
        resume,
    } = args;
    let id = id.to_owned();
    interrupt::watch(&id);
    let providers: Box<dyn kiln::scheduler::TicketProviders> = match run_fixture {
        Some(path) => Box::new(kiln::scheduler::FixtureScenario::load(
            &engine.repository.join(path),
            &engine.repository,
        )?),
        None => {
            let config = engine.inspect(&id)?.config;
            let repository = engine.repository.clone();
            let isolation = config.isolation.clone();
            match Provider::select(codex.clone(), claude.clone()) {
                Provider::Codex(path) => Box::new(
                    kiln::scheduler::CodexProviders::new(kiln::codex::CodexConfig::from_project(
                        &config, path,
                    )?)
                    .with_replanning(repository, isolation),
                ),
                Provider::Claude(path) => Box::new(
                    kiln::scheduler::ClaudeProviders::new(
                        kiln::claude::ClaudeConfig::from_project(&config, path)?,
                    )
                    .with_replanning(repository, isolation),
                ),
            }
        }
    };
    let mut run = if resume {
        engine.resume(&id, providers.as_ref())?
    } else {
        engine.run_tickets(&id, providers.as_ref())?
    };
    match run.status.as_str() {
        "paused" => return paused(&run, report),
        "cancelled" => {
            report.emit(&run)?;
            anyhow::bail!(
                "run cancelled; active work was stopped and no further tickets were started"
            );
        }
        _ => {}
    }
    engine.record_backlog_results(&id, &mut run)?;
    if interrupt::requested() {
        // Scheduling settled before the pause request reached it: stop here,
        // before publication starts.
        run = engine.transact(&id, |stored| {
            stored.status = "paused".into();
            Ok(stored.clone())
        })?;
        return paused(&run, report);
    }
    if kiln::publication::PublicationSettings::from_config(&run.config)?.is_some() {
        let host: Box<dyn kiln::publication::PullRequestHost> = match publication_fixture {
            Some(path) => Box::new(kiln::publication::FixturePullRequests::new(
                &engine.repository.join(path),
            )),
            None => Box::new(kiln::publication::GitHubPullRequests {
                program: gh.unwrap_or_else(|| "gh".into()),
            }),
        };
        let published = if let Some(path) = repair_fixture {
            let repair =
                kiln::correction::FixtureCorrectionAgent::load(&engine.repository.join(path))?;
            engine.publish_delivery_groups_with_repair(&id, host.as_ref(), Some((&repair, &repair)))
        } else {
            with_adapter!(
                Provider::select(codex.clone(), claude.clone()),
                &run.config,
                |agent| {
                    engine.publish_delivery_groups_with_repair(
                        &id,
                        host.as_ref(),
                        Some((&agent, &agent)),
                    )
                }
            )
        };
        run = match published {
            Ok(run) => run,
            Err(error) => {
                let stored = engine.inspect(&id)?;
                match stored.status.as_str() {
                    "paused" => return paused(&stored, report),
                    "cancelled" => {
                        report.emit(&stored)?;
                        anyhow::bail!(
                            "run cancelled during delivery; published pull requests were kept"
                        );
                    }
                    _ => return Err(error),
                }
            }
        };
    } else if run
        .delivery_groups
        .iter()
        .any(|group| group.status == "integrated")
    {
        run = engine.block_delivery_groups(
            &id,
            "publication is not configured; set publication.github_repository and publication.target_branch to deliver verified groups",
        )?;
        report.emit(&run)?;
        anyhow::bail!("backlog delivery is blocked because publication is not configured");
    }
    report.emit(&run)?;
    Ok(())
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    let engine = Engine::open(&cli.repo)?;
    let value = match cli.command {
        Commands::StartIssue {
            config,
            github_repo,
            issue,
            issue_fixture,
            planning_fixture,
            codex,
            claude,
            run_fixture,
            verifier,
            publication_fixture,
            gh,
        } => {
            let source: Box<dyn kiln::import::IssueSource> = match issue_fixture {
                Some(path) => Box::new(kiln::import::FixtureIssues::load(
                    &engine.repository.join(path),
                )?),
                None => Box::new(kiln::import::GitHubIssues),
            };
            let run = if let Some(path) = &planning_fixture {
                let agent = kiln::planning::FixtureAgent::load(&engine.repository.join(path))?;
                engine.prepare_backlog_issue(
                    &config,
                    &github_repo,
                    issue,
                    source.as_ref(),
                    &agent,
                )?
            } else {
                let project_config = kiln::ProjectConfig::load(
                    &engine.repository.join(&config),
                    &engine.repository,
                )?;
                with_planner!(
                    Provider::select(codex.clone(), claude.clone()),
                    engine,
                    project_config,
                    |agent| engine.prepare_backlog_issue(
                        &config,
                        &github_repo,
                        issue,
                        source.as_ref(),
                        &agent
                    )?
                )
            };
            let id = run.id.clone();
            let mut planned = if let Some(path) = &planning_fixture {
                let agent = kiln::planning::FixtureAgent::load(&engine.repository.join(path))?;
                let agent = kiln::backlog::BacklogPlanningAgent { inner: &agent };
                engine.plan(&id, &agent)?
            } else {
                let config = engine.inspect(&id)?.config;
                with_planner!(
                    Provider::select(codex.clone(), claude.clone()),
                    engine,
                    config,
                    |agent| {
                        let agent = kiln::backlog::BacklogPlanningAgent { inner: &agent };
                        engine.plan(&id, &agent)?
                    }
                )
            };
            engine.record_backlog_plan(&id, &mut planned)?;
            if !planned.plan.as_ref().is_some_and(|plan| plan.executable) {
                println!("{}", serde_json::to_string_pretty(&planned)?);
                anyhow::bail!("backlog issue plan rejected; inspect recorded findings");
            }
            let providers: Box<dyn kiln::scheduler::TicketProviders> =
                if let Some(path) = run_fixture {
                    Box::new(kiln::scheduler::FixtureScenario::load(
                        &engine.repository.join(path),
                        &engine.repository,
                    )?)
                } else {
                    let config = engine.inspect(&id)?.config;
                    let repository = engine.repository.clone();
                    let isolation = config.isolation.clone();
                    match Provider::select(codex, claude) {
                        Provider::Codex(path) => Box::new(
                            kiln::scheduler::CodexProviders::new(
                                kiln::codex::CodexConfig::from_project(&config, path)?,
                            )
                            .with_replanning(repository, isolation),
                        ),
                        Provider::Claude(path) => Box::new(
                            kiln::scheduler::ClaudeProviders::new(
                                kiln::claude::ClaudeConfig::from_project(&config, path)?,
                            )
                            .with_replanning(repository, isolation),
                        ),
                    }
                };
            let run = engine.run_tickets(&id, providers.as_ref())?;
            if run.status == "blocked" {
                anyhow::bail!(
                    "backlog issue blocked during implementation; inspect recorded run evidence"
                );
            }
            let checks = match verifier {
                Some(path) => {
                    kiln::validation::VerifierChecks::load(&engine.repository.join(path))?
                }
                None => Default::default(),
            };
            let run = engine.validate(&id, &checks)?;
            if !run
                .validation_reports
                .last()
                .is_some_and(|report| report.outcome == kiln::validation::VERIFIED)
            {
                anyhow::bail!("backlog issue failed validation; inspect the recorded report");
            }
            let host: Box<dyn kiln::publication::PullRequestHost> = match publication_fixture {
                Some(path) => Box::new(kiln::publication::FixturePullRequests::new(
                    &engine.repository.join(path),
                )),
                None => Box::new(kiln::publication::GitHubPullRequests {
                    program: gh.unwrap_or_else(|| "gh".into()),
                }),
            };
            let mut run = engine.publish(&id, host.as_ref())?;
            engine.transact(&id, |stored| {
                if let Some(backlog) = &mut stored.backlog {
                    backlog.outcome = stored
                        .publication
                        .as_ref()
                        .map(|p| p.status.clone())
                        .unwrap_or_else(|| stored.status.clone());
                    let selected = format!(
                        "github:{}#{}",
                        backlog.github_repository, backlog.selected_issue
                    );
                    if let Some(disposition) = backlog
                        .dispositions
                        .iter_mut()
                        .find(|disposition| disposition.issue == selected)
                    {
                        disposition.status = "completed".into();
                        disposition.reason = "Implementation passed validation and was delivered in a draft pull request.".into();
                    }
                    backlog.evidence.push(format!(
                        "Final outcome: {}; publication status: {}.",
                        stored.status,
                        stored
                            .publication
                            .as_ref()
                            .map(|p| p.status.as_str())
                            .unwrap_or("missing")
                    ));
                }
                run = stored.clone();
                Ok(())
            })?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            return Ok(());
        }
        Commands::StartBacklog {
            config,
            github_repo,
            issue_fixture,
            planning_fixture,
            codex,
            claude,
            run_fixture,
            publication_fixture,
            gh,
            repair_fixture,
            plan_only,
        } => {
            return start_backlog(
                &engine,
                BacklogArgs {
                    config,
                    github_repo,
                    issue_fixture,
                    planning_fixture,
                    codex,
                    claude,
                    run_fixture,
                    publication_fixture,
                    gh,
                    repair_fixture,
                    plan_only,
                    reuse_plan: false,
                },
                &Report::Json,
            );
        }
        Commands::Import {
            id,
            github_repo,
            issue,
            fixture,
            verification_fixture,
            codex,
            claude,
        } => {
            let source: Box<dyn kiln::import::IssueSource> = match fixture {
                Some(path) => Box::new(kiln::import::FixtureIssues::load(
                    &engine.repository.join(path),
                )?),
                None => Box::new(kiln::import::GitHubIssues),
            };
            let verifier: Box<dyn kiln::planning::PlanningAgent> =
                if let Some(path) = verification_fixture {
                    Box::new(kiln::planning::FixtureAgent::load(
                        &engine.repository.join(path),
                    )?)
                } else {
                    let config = engine.inspect(&id)?.config;
                    with_planner!(Provider::select(codex, claude), engine, config, |a| {
                        Box::new(a)
                    })
                };
            let run = engine.import_issues(
                &id,
                &github_repo,
                &issue,
                source.as_ref(),
                verifier.as_ref(),
            )?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            if !run.plan.as_ref().is_some_and(|p| p.executable) {
                anyhow::bail!("import rejected; inspect recorded findings");
            }
            return Ok(());
        }
        Commands::Prepare { config, spec } => {
            serde_json::to_value(engine.prepare(&config, &spec)?)?
        }
        Commands::Plan {
            id: None,
            config,
            github_repo,
            codex,
            claude,
            issue_fixture,
            planning_fixture,
            json,
            ..
        } => {
            let defaults = kiln::defaults::Defaults::infer(
                &engine.repository,
                kiln::defaults::Overrides {
                    config,
                    github_repo,
                    codex,
                    claude,
                },
            )?;
            let (provider, codex, claude) = if planning_fixture.is_some() {
                (None, None, None)
            } else {
                let choice = defaults.provider()?;
                let name = choice.agent.name();
                let (codex, claude) = choice.into_flags();
                (Some(name), codex, claude)
            };
            let report = if json {
                Report::Json
            } else {
                Report::Plan { provider }
            };
            return start_backlog(
                &engine,
                BacklogArgs {
                    config: defaults.config,
                    github_repo: defaults.github_repo,
                    issue_fixture,
                    planning_fixture,
                    codex,
                    claude,
                    run_fixture: None,
                    publication_fixture: None,
                    gh: None,
                    repair_fixture: None,
                    plan_only: true,
                    reuse_plan: false,
                },
                &report,
            );
        }
        Commands::Start {
            config,
            github_repo,
            codex,
            claude,
            fresh,
            json,
            issue_fixture,
            planning_fixture,
            run_fixture,
            publication_fixture,
            gh,
            repair_fixture,
        } => {
            let defaults = kiln::defaults::Defaults::infer(
                &engine.repository,
                kiln::defaults::Overrides {
                    config,
                    github_repo,
                    codex,
                    claude,
                },
            )?;
            interrupt::install(&engine);
            // Fixtures stand in for every agent call, so no provider is needed.
            let (provider, codex, claude) = if planning_fixture.is_some()
                && run_fixture.is_some()
                && repair_fixture.is_some()
            {
                (None, None, None)
            } else {
                let choice = defaults.provider()?;
                let name = choice.agent.name();
                let (codex, claude) = choice.into_flags();
                (Some(name), codex, claude)
            };
            let report = if json {
                Report::Json
            } else {
                Report::Start { provider }
            };
            return start_backlog(
                &engine,
                BacklogArgs {
                    config: defaults.config,
                    github_repo: defaults.github_repo,
                    issue_fixture,
                    planning_fixture,
                    codex,
                    claude,
                    run_fixture,
                    publication_fixture,
                    gh,
                    repair_fixture,
                    plan_only: false,
                    reuse_plan: !fresh,
                },
                &report,
            );
        }
        Commands::Plan {
            id: Some(id),
            fixture,
            codex,
            claude,
            ..
        } => {
            let agent: Box<dyn kiln::planning::PlanningAgent> = if let Some(path) = fixture {
                Box::new(kiln::planning::FixtureAgent::load(
                    &engine.repository.join(path),
                )?)
            } else {
                let config = engine.inspect(&id)?.config;
                with_planner!(Provider::select(codex, claude), engine, config, |a| {
                    Box::new(a)
                })
            };
            let run = engine.plan(&id, agent.as_ref())?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            if !run.plan.as_ref().is_some_and(|p| p.executable) {
                anyhow::bail!("plan rejected; inspect the recorded findings");
            }
            return Ok(());
        }
        Commands::Implement {
            id,
            ticket,
            fixture,
            codex,
            claude,
        } => {
            let agent: Box<dyn kiln::execution::ImplementationAgent> = if let Some(path) = fixture {
                Box::new(kiln::execution::FixtureImplementationAgent::load(
                    &engine.repository.join(path),
                )?)
            } else {
                let config = engine.inspect(&id)?.config;
                with_adapter!(Provider::select(codex, claude), &config, |a| Box::new(a))
            };
            let run = engine.implement_ticket(&id, &ticket, agent.as_ref())?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            if run
                .sessions
                .last()
                .is_none_or(|s| s.status != "implemented")
            {
                anyhow::bail!("implementation failed; inspect recorded session evidence");
            }
            return Ok(());
        }
        Commands::Review {
            id,
            ticket,
            fixture,
            codex,
            claude,
        } => {
            let agent: Box<dyn kiln::review::ReviewAgent> = if let Some(path) = fixture {
                Box::new(kiln::review::FixtureReviewAgent::load(
                    &engine.repository.join(path),
                )?)
            } else {
                let config = engine.inspect(&id)?.config;
                with_adapter!(Provider::select(codex, claude), &config, |a| Box::new(a))
            };
            let run = engine.review_ticket(&id, &ticket, agent.as_ref())?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            if !run.reviews.last().is_some_and(|r| r.passed) {
                anyhow::bail!("review rejected; inspect axis findings and executable evidence");
            }
            return Ok(());
        }
        Commands::Correct {
            id,
            ticket,
            fixture,
            codex,
            claude,
        } => {
            let run = if let Some(path) = fixture {
                let a =
                    kiln::correction::FixtureCorrectionAgent::load(&engine.repository.join(path))?;
                engine.correct_ticket(&id, &ticket, &a, &a)?
            } else {
                let config = engine.inspect(&id)?.config;
                with_adapter!(Provider::select(codex, claude), &config, |a| engine
                    .correct_ticket(&id, &ticket, &a, &a)?)
            };
            println!("{}", serde_json::to_string_pretty(&run)?);
            let session = run
                .sessions
                .iter()
                .rev()
                .find(|s| s.ticket_id == ticket)
                .ok_or_else(|| anyhow::anyhow!("missing session"))?;
            if !engine.review_gate(&run, session)? {
                anyhow::bail!("correction blocked; inspect cycle history");
            }
            return Ok(());
        }
        Commands::Decide {
            id,
            ambiguity,
            fixture,
            codex,
            claude,
        } => {
            let ambiguity = kiln::decision::Ambiguity::load(&engine.repository.join(ambiguity))?;
            let run = if let Some(path) = fixture {
                let agent =
                    kiln::decision::FixtureDecisionAgent::load(&engine.repository.join(path))?;
                engine.decide(&id, &ambiguity, &agent, &agent)?
            } else {
                let config = engine.inspect(&id)?.config;
                with_planner!(Provider::select(codex, claude), engine, config, |agent| {
                    engine.decide(&id, &ambiguity, &agent, &agent)?
                })
            };
            println!("{}", serde_json::to_string_pretty(&run)?);
            if run.decisions.last().is_none_or(|d| d.outcome != "resolved") {
                anyhow::bail!("decision not adopted; inspect the recorded findings");
            }
            return Ok(());
        }
        Commands::Replan {
            id,
            spec,
            fixture,
            codex,
            claude,
        } => {
            let run = if let Some(path) = fixture {
                let agent = kiln::spec_replanning::FixtureSpecReplanning::load(
                    &engine.repository.join(path),
                )?;
                engine.replan_specs(&id, &spec, &agent)?
            } else {
                let config = engine.inspect(&id)?.config;
                with_planner!(Provider::select(codex, claude), engine, config, |agent| {
                    engine.replan_specs(&id, &spec, &agent)?
                })
            };
            println!("{}", serde_json::to_string_pretty(&run)?);
            if run
                .spec_replans
                .last()
                .is_none_or(|r| r.outcome != "replanned")
            {
                anyhow::bail!("replan not adopted; inspect the recorded findings");
            }
            return Ok(());
        }
        Commands::Integrate {
            id,
            ticket,
            fixture,
            codex,
            claude,
        } => {
            let _owner = engine.own_run(&id)?;
            let run = if let Some(path) = fixture {
                let a =
                    kiln::correction::FixtureCorrectionAgent::load(&engine.repository.join(path))?;
                engine.integrate_ticket(&id, &ticket, Some((&a, &a)))?
            } else if codex.is_some() || claude.is_some() {
                let config = engine.inspect(&id)?.config;
                with_adapter!(Provider::select(codex, claude), &config, |a| engine
                    .integrate_ticket(&id, &ticket, Some((&a, &a)))?)
            } else {
                engine.integrate_ticket(&id, &ticket, None)?
            };
            println!("{}", serde_json::to_string_pretty(&run)?);
            if run
                .integrations
                .last()
                .is_none_or(|i| i.status != "integrated")
            {
                anyhow::bail!("integration blocked; inspect combined checks and conflict evidence");
            }
            return Ok(());
        }
        Commands::Resume {
            id: None,
            fixture,
            codex,
            claude,
            publication_fixture,
            gh,
            repair_fixture,
            json,
        } => {
            let run = engine
                .latest_resumable_run()?
                .context("no paused or interrupted run to resume; start one with `kiln start`")?;
            // Fixtures stand in for every agent call, so no provider is needed.
            let (provider, codex, claude) = if fixture.is_some() && repair_fixture.is_some() {
                (None, None, None)
            } else {
                let defaults = kiln::defaults::Defaults::infer(
                    &engine.repository,
                    kiln::defaults::Overrides {
                        config: None,
                        github_repo: run.backlog.as_ref().map(|b| b.github_repository.clone()),
                        codex,
                        claude,
                    },
                )?;
                let choice = defaults.provider()?;
                let name = choice.agent.name();
                let (codex, claude) = choice.into_flags();
                (Some(name), codex, claude)
            };
            let report = if json {
                Report::Json
            } else {
                Report::Start { provider }
            };
            eprintln!("Resuming run {} (status: {}).", run.id, run.status);
            interrupt::install(&engine);
            return deliver(
                &engine,
                &run.id,
                Delivery {
                    run_fixture: fixture,
                    codex,
                    claude,
                    publication_fixture,
                    gh,
                    repair_fixture,
                    resume: true,
                },
                &report,
            );
        }
        command @ (Commands::Run { .. } | Commands::Resume { id: Some(_), .. }) => {
            let (resume, id, fixture, codex, claude, publication_fixture, gh, repair_fixture) =
                match command {
                    Commands::Run {
                        id,
                        fixture,
                        codex,
                        claude,
                    } => (false, id, fixture, codex, claude, None, None, None),
                    Commands::Resume {
                        id: Some(id),
                        fixture,
                        codex,
                        claude,
                        publication_fixture,
                        gh,
                        repair_fixture,
                        ..
                    } => (
                        true,
                        id,
                        fixture,
                        codex,
                        claude,
                        publication_fixture,
                        gh,
                        repair_fixture,
                    ),
                    _ => unreachable!(),
                };
            let delivery_codex = codex.clone();
            let delivery_claude = claude.clone();
            let providers: Box<dyn kiln::scheduler::TicketProviders> = match fixture {
                Some(path) => Box::new(kiln::scheduler::FixtureScenario::load(
                    &engine.repository.join(path),
                    &engine.repository,
                )?),
                None => {
                    let config = engine.inspect(&id)?.config;
                    let repository = engine.repository.clone();
                    let isolation = config.isolation.clone();
                    match Provider::select(codex, claude) {
                        Provider::Codex(path) => Box::new(
                            kiln::scheduler::CodexProviders::new(
                                kiln::codex::CodexConfig::from_project(&config, path)?,
                            )
                            .with_replanning(repository, isolation),
                        ),
                        Provider::Claude(path) => Box::new(
                            kiln::scheduler::ClaudeProviders::new(
                                kiln::claude::ClaudeConfig::from_project(&config, path)?,
                            )
                            .with_replanning(repository, isolation),
                        ),
                    }
                }
            };
            let mut run = if resume {
                engine.resume(&id, providers.as_ref())?
            } else {
                engine.run_tickets(&id, providers.as_ref())?
            };
            if run
                .backlog
                .as_ref()
                .is_some_and(|backlog| backlog.mode == "issue-graph")
            {
                engine.record_backlog_results(&id, &mut run)?;
            }
            if run
                .backlog
                .as_ref()
                .is_some_and(|b| b.mode == "issue-graph")
            {
                if kiln::publication::PublicationSettings::from_config(&run.config)?.is_none() {
                    if run
                        .delivery_groups
                        .iter()
                        .any(|group| group.status == "integrated")
                    {
                        run = engine.block_delivery_groups(
                            &id,
                            "publication is not configured; set publication.github_repository and publication.target_branch to deliver verified groups",
                        )?;
                        println!("{}", serde_json::to_string_pretty(&run)?);
                        anyhow::bail!(
                            "backlog delivery is blocked because publication is not configured"
                        );
                    }
                } else {
                    let host: Box<dyn kiln::publication::PullRequestHost> =
                        match publication_fixture {
                            Some(path) => Box::new(kiln::publication::FixturePullRequests::new(
                                &engine.repository.join(path),
                            )),
                            None => Box::new(kiln::publication::GitHubPullRequests {
                                program: gh.unwrap_or_else(|| "gh".into()),
                            }),
                        };
                    if let Some(path) = repair_fixture {
                        let repair = kiln::correction::FixtureCorrectionAgent::load(
                            &engine.repository.join(path),
                        )?;
                        run = engine.publish_delivery_groups_with_repair(
                            &id,
                            host.as_ref(),
                            Some((&repair, &repair)),
                        )?;
                    } else {
                        run = with_adapter!(
                            Provider::select(delivery_codex, delivery_claude),
                            &run.config,
                            |agent| {
                                engine.publish_delivery_groups_with_repair(
                                    &id,
                                    host.as_ref(),
                                    Some((&agent, &agent)),
                                )?
                            }
                        );
                    }
                }
            }
            println!("{}", serde_json::to_string_pretty(&run)?);
            if run.status == "blocked" {
                anyhow::bail!("run blocked; inspect scheduler blockers and ticket evidence");
            }
            if run.status == "cancelled" {
                anyhow::bail!(
                    "run cancelled; active work was stopped and no further tickets were started"
                );
            }
            if let Some(exhausted) = run
                .scheduler
                .as_ref()
                .and_then(|s| s.limits.as_ref())
                .and_then(|l| l.exhausted.as_ref())
            {
                anyhow::bail!(
                    "run stopped by {} limit: {}; completed work and resumable state are preserved",
                    exhausted.limit,
                    exhausted.reason
                );
            }
            return Ok(());
        }
        Commands::Validate { id, verifier } => {
            let checks = match verifier {
                Some(path) => {
                    kiln::validation::VerifierChecks::load(&engine.repository.join(path))?
                }
                None => Default::default(),
            };
            let run = engine.validate(&id, &checks)?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            let outcome = run.validation_reports.last().map(|r| r.outcome.as_str());
            if outcome != Some(kiln::validation::VERIFIED) {
                anyhow::bail!(
                    "validation {}; inspect the report for failed and unable-to-verify criteria",
                    outcome.unwrap_or("missing")
                );
            }
            return Ok(());
        }
        Commands::Publish {
            id,
            fixture,
            gh,
            repair_fixture,
            codex,
            claude,
        } => {
            let host: Box<dyn kiln::publication::PullRequestHost> = match fixture {
                Some(path) => Box::new(kiln::publication::FixturePullRequests::new(
                    &engine.repository.join(path),
                )),
                None => Box::new(kiln::publication::GitHubPullRequests {
                    program: gh.unwrap_or_else(|| "gh".into()),
                }),
            };
            let run = engine.inspect(&id)?;
            serde_json::to_value(
                if run
                    .backlog
                    .as_ref()
                    .is_some_and(|backlog| backlog.mode == "issue-graph")
                {
                    if let Some(path) = repair_fixture {
                        let agent = kiln::correction::FixtureCorrectionAgent::load(
                            &engine.repository.join(path),
                        )?;
                        engine.publish_delivery_groups_with_repair(
                            &id,
                            host.as_ref(),
                            Some((&agent, &agent)),
                        )?
                    } else {
                        let config = run.config;
                        with_adapter!(Provider::select(codex, claude), &config, |agent| {
                            engine.publish_delivery_groups_with_repair(
                                &id,
                                host.as_ref(),
                                Some((&agent, &agent)),
                            )?
                        })
                    }
                } else {
                    engine.publish(&id, host.as_ref())?
                },
            )?
        }
        Commands::Sync { id, fixture, gh } => {
            let tracker: Box<dyn kiln::synchronization::IssueTracker> = match fixture {
                Some(path) => Box::new(kiln::synchronization::FixtureIssueTracker::new(
                    &engine.repository.join(path),
                )),
                None => Box::new(kiln::synchronization::GitHubIssueTracker {
                    program: gh.unwrap_or_else(|| "gh".into()),
                }),
            };
            let run = engine.synchronize(&id, tracker.as_ref())?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            let issues = run
                .synchronization
                .as_ref()
                .map(|s| s.issues.as_slice())
                .unwrap_or_default();
            for note in issues.iter().flat_map(|i| &i.divergence) {
                eprintln!("Kiln: divergence: {note}");
            }
            if issues.iter().any(|i| i.status == "conflict") {
                anyhow::bail!(
                    "synchronization conflict; remote edits were surfaced, not overwritten"
                );
            }
            return Ok(());
        }
        Commands::Pause { id: Some(id) } => {
            engine.request_control(&id, "pause")?;
            serde_json::json!({"id": id, "requested": "pause"})
        }
        Commands::Cancel { id: Some(id) } => {
            engine.request_control(&id, "cancel")?;
            serde_json::json!({"id": id, "requested": "cancel"})
        }
        Commands::Pause { id: None } => {
            let id = engine
                .latest_active_run()?
                .context("no active run to pause; `kiln status` shows the latest run")?;
            engine.request_control(&id, "pause")?;
            println!(
                "Pause requested for run {id}; it pauses once active ticket work settles. Run `kiln resume` to continue it."
            );
            return Ok(());
        }
        Commands::Cancel { id: None } => {
            let id = engine
                .latest_active_run()?
                .context("no active run to cancel; `kiln status` shows the latest run")?;
            engine.request_control(&id, "cancel")?;
            println!(
                "Cancellation requested for run {id}; active work is stopped and no further tickets start."
            );
            return Ok(());
        }
        Commands::Report { id } => {
            let run = engine.inspect(&id)?;
            if run
                .backlog
                .as_ref()
                .is_some_and(|backlog| backlog.mode == "issue-graph")
            {
                serde_json::json!({
                    "run_id": run.id,
                    "delivery_groups": run.delivery_groups,
                    "validation_reports": run.validation_reports,
                    "publication": run.publication,
                })
            } else {
                let report = run.validation_reports.last().ok_or_else(|| {
                    anyhow::anyhow!(
                        "run {id} has no validation report; run `kiln validate {id}` first"
                    )
                })?;
                report.summary(&run.id)
            }
        }
        Commands::Inspect { id: Some(id) } => {
            let run = engine.inspect(&id)?;
            let mut value = serde_json::to_value(&run)?;
            for name in run.config.isolation.secrets.keys() {
                if let Ok(secret) = std::env::var(name) {
                    if !secret.is_empty() {
                        value = serde_json::from_str(
                            &serde_json::to_string(&value)?.replace(&secret, "[REDACTED]"),
                        )?;
                    }
                }
            }
            value
        }
        Commands::Inspect { id: None } => serde_json::to_value(engine.list()?)?,
        Commands::Serve {
            bind,
            backlog_config,
            github_repo,
            issue_fixture,
            planning_fixture,
            run_fixture,
            codex,
            claude,
            publication_fixture,
            gh,
            repair_fixture,
        } => {
            if backlog_config.is_some() {
                if planning_fixture.is_none() && codex.is_none() && claude.is_none() {
                    anyhow::bail!(
                        "web backlog start requires --planning-fixture, --codex, or --claude"
                    );
                }
                if run_fixture.is_none() && codex.is_none() && claude.is_none() {
                    anyhow::bail!("web backlog start requires --run-fixture, --codex, or --claude");
                }
            }
            let backlog = backlog_config
                .zip(github_repo)
                .map(|(config, github_repo)| kiln::web::BacklogLaunch {
                    config,
                    github_repo,
                    issue_fixture,
                    planning_fixture,
                    run_fixture,
                    codex,
                    claude,
                    publication_fixture,
                    gh,
                    repair_fixture,
                });
            return kiln::web::serve_with_backlog(engine, bind, backlog);
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Kiln: {error:#}");
        std::process::exit(1);
    }
}
