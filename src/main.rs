use anyhow::Result;
use clap::{Parser, Subcommand};
use kiln::Engine;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "kiln", about = "Prepare and inspect durable workflow runs")]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Snapshot open issues and take one independent issue through planning, delivery gates and draft PR publication.
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
    /// Read selected GitHub issues and independently verify against frozen specs.
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
    Prepare {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, required = true)]
        spec: Vec<PathBuf>,
    },
    /// Generate and independently verify tickets using fresh provider contexts.
    Plan {
        id: String,
        #[arg(long)]
        #[arg(required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Execute one eligible ticket in an isolated worktree.
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
    /// Reopen an interrupted run: reconcile recorded state with observed Git and process
    /// state, record each recovery decision, then continue scheduling without
    /// repeating completed effects.
    Resume {
        id: String,
        #[arg(long, required_unless_present_any = ["codex", "claude"], conflicts_with_all = ["codex", "claude"])]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
        /// Claude Code executable; mutually exclusive with --codex.
        #[arg(long, conflicts_with = "codex")]
        claude: Option<PathBuf>,
    },
    /// Let active ticket work settle, then pause before starting more work.
    Pause { id: String },
    /// Stop active work safely and prevent any more tickets from starting.
    Cancel { id: String },
    /// Validate the integrated revision against acceptance workflows per criterion.
    Validate {
        id: String,
        /// Independent verifier acceptance tests: {"acceptance_checks":[{criterion, command}]}.
        #[arg(long)]
        verifier: Option<PathBuf>,
    },
    /// Push the verified integration branch and open or reconcile its pull request.
    /// Merging and deployment stay disabled unless the project configures them.
    Publish {
        id: String,
        /// Simulated GitHub state file (no network access).
        #[arg(long, conflicts_with = "gh")]
        fixture: Option<PathBuf>,
        /// gh program used for real GitHub access (default: gh on PATH).
        #[arg(long)]
        gh: Option<PathBuf>,
    },
    /// Reflect recorded, verified progress in a Kiln progress comment on each imported
    /// issue. Issue titles, bodies and other comments are never modified.
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
    Report { id: String },
    /// Read a recorded run, or list all run identities.
    Inspect { id: Option<String> },
    /// Show recorded state on a loopback-only local web server.
    Serve {
        #[arg(long, default_value = "127.0.0.1:3000")]
        bind: std::net::SocketAddr,
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
            id,
            fixture,
            codex,
            claude,
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
        command @ (Commands::Run { .. } | Commands::Resume { .. }) => {
            let (resume, id, fixture, codex, claude) = match command {
                Commands::Run {
                    id,
                    fixture,
                    codex,
                    claude,
                } => (false, id, fixture, codex, claude),
                Commands::Resume {
                    id,
                    fixture,
                    codex,
                    claude,
                } => (true, id, fixture, codex, claude),
                _ => unreachable!(),
            };
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
            let run = if resume {
                engine.resume(&id, providers.as_ref())?
            } else {
                engine.run_tickets(&id, providers.as_ref())?
            };
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
        Commands::Publish { id, fixture, gh } => {
            let host: Box<dyn kiln::publication::PullRequestHost> = match fixture {
                Some(path) => Box::new(kiln::publication::FixturePullRequests::new(
                    &engine.repository.join(path),
                )),
                None => Box::new(kiln::publication::GitHubPullRequests {
                    program: gh.unwrap_or_else(|| "gh".into()),
                }),
            };
            serde_json::to_value(engine.publish(&id, host.as_ref())?)?
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
        Commands::Pause { id } => {
            engine.request_control(&id, "pause")?;
            serde_json::json!({"id": id, "requested": "pause"})
        }
        Commands::Cancel { id } => {
            engine.request_control(&id, "cancel")?;
            serde_json::json!({"id": id, "requested": "cancel"})
        }
        Commands::Report { id } => {
            let run = engine.inspect(&id)?;
            let report = run.validation_reports.last().ok_or_else(|| {
                anyhow::anyhow!("run {id} has no validation report; run `kiln validate {id}` first")
            })?;
            report.summary(&run.id)
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
        Commands::Serve { bind } => return kiln::web::serve(engine, bind),
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
