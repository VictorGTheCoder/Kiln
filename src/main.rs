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
        verification_fixture: PathBuf,
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
        #[arg(required_unless_present = "codex", conflicts_with = "codex")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
    },
    /// Execute one eligible ticket in an isolated worktree.
    Implement {
        id: String,
        ticket: String,
        #[arg(long)]
        #[arg(required_unless_present = "codex", conflicts_with = "codex")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
    },
    /// Review an exact implementation commit in independent contexts.
    Review {
        id: String,
        ticket: String,
        #[arg(long, required_unless_present = "codex", conflicts_with = "codex")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
    },
    Correct {
        id: String,
        ticket: String,
        #[arg(long, required_unless_present = "codex", conflicts_with = "codex")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
    },
    /// Integrate an exact reviewed change and verify the combined result.
    Integrate {
        id: String,
        ticket: String,
        #[arg(long, conflicts_with = "codex")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
    },
    /// Schedule every accepted ticket: concurrent independent sessions, prerequisite
    /// ordering across specs, serialized verified integration.
    Run {
        id: String,
        #[arg(long, required_unless_present = "codex", conflicts_with = "codex")]
        fixture: Option<PathBuf>,
        #[arg(long)]
        codex: Option<PathBuf>,
    },
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
fn run() -> Result<()> {
    let cli = Cli::parse();
    let engine = Engine::open(&cli.repo)?;
    let value = match cli.command {
        Commands::Import {
            id,
            github_repo,
            issue,
            fixture,
            verification_fixture,
        } => {
            let source: Box<dyn kiln::import::IssueSource> = match fixture {
                Some(path) => Box::new(kiln::import::FixtureIssues::load(
                    &engine.repository.join(path),
                )?),
                None => Box::new(kiln::import::GitHubIssues),
            };
            let verifier =
                kiln::planning::FixtureAgent::load(&engine.repository.join(verification_fixture))?;
            let run =
                engine.import_issues(&id, &github_repo, &issue, source.as_ref(), &verifier)?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            if !run.plan.as_ref().is_some_and(|p| p.executable) {
                anyhow::bail!("import rejected; inspect recorded findings");
            }
            return Ok(());
        }
        Commands::Prepare { config, spec } => {
            serde_json::to_value(engine.prepare(&config, &spec)?)?
        }
        Commands::Plan { id, fixture, codex } => {
            let agent: Box<dyn kiln::planning::PlanningAgent> = if let Some(path) = fixture {
                Box::new(kiln::planning::FixtureAgent::load(
                    &engine.repository.join(path),
                )?)
            } else {
                let config = engine.inspect(&id)?.config;
                Box::new(kiln::codex::CodexPlanningAgent {
                    adapter: kiln::codex::CodexAdapter::new(
                        kiln::codex::CodexConfig::from_project(&config, codex)?,
                    ),
                    repository: engine.repository.clone(),
                    isolation: config.isolation,
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
        } => {
            let agent: Box<dyn kiln::execution::ImplementationAgent> = if let Some(path) = fixture {
                Box::new(kiln::execution::FixtureImplementationAgent::load(
                    &engine.repository.join(path),
                )?)
            } else {
                Box::new(kiln::codex::CodexAdapter::new(
                    kiln::codex::CodexConfig::from_project(&engine.inspect(&id)?.config, codex)?,
                ))
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
        } => {
            let agent: Box<dyn kiln::review::ReviewAgent> = if let Some(path) = fixture {
                Box::new(kiln::review::FixtureReviewAgent::load(
                    &engine.repository.join(path),
                )?)
            } else {
                Box::new(kiln::codex::CodexAdapter::new(
                    kiln::codex::CodexConfig::from_project(&engine.inspect(&id)?.config, codex)?,
                ))
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
        } => {
            let run = if let Some(path) = fixture {
                let a =
                    kiln::correction::FixtureCorrectionAgent::load(&engine.repository.join(path))?;
                engine.correct_ticket(&id, &ticket, &a, &a)?
            } else {
                let a = kiln::codex::CodexAdapter::new(kiln::codex::CodexConfig::from_project(
                    &engine.inspect(&id)?.config,
                    codex,
                )?);
                engine.correct_ticket(&id, &ticket, &a, &a)?
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
        Commands::Integrate {
            id,
            ticket,
            fixture,
            codex,
        } => {
            let run = if let Some(path) = fixture {
                let a =
                    kiln::correction::FixtureCorrectionAgent::load(&engine.repository.join(path))?;
                engine.integrate_ticket(&id, &ticket, Some((&a, &a)))?
            } else if let Some(path) = codex {
                let a = kiln::codex::CodexAdapter::new(kiln::codex::CodexConfig::from_project(
                    &engine.inspect(&id)?.config,
                    Some(path),
                )?);
                engine.integrate_ticket(&id, &ticket, Some((&a, &a)))?
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
        Commands::Run { id, fixture, codex } => {
            let run = if let Some(path) = fixture {
                let scenario = kiln::scheduler::FixtureScenario::load(
                    &engine.repository.join(path),
                    &engine.repository,
                )?;
                engine.run_tickets(&id, &scenario)?
            } else {
                let providers = kiln::scheduler::CodexProviders(
                    kiln::codex::CodexConfig::from_project(&engine.inspect(&id)?.config, codex)?,
                );
                engine.run_tickets(&id, &providers)?
            };
            println!("{}", serde_json::to_string_pretty(&run)?);
            if run.status == "blocked" {
                anyhow::bail!("run blocked; inspect scheduler blockers and ticket evidence");
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
        Commands::Report { id } => {
            let run = engine.inspect(&id)?;
            let report = run.validation_reports.last().ok_or_else(|| {
                anyhow::anyhow!("run {id} has no validation report; run `kiln validate {id}` first")
            })?;
            report.summary(&run.id)
        }
        Commands::Inspect { id: Some(id) } => serde_json::to_value(engine.inspect(&id)?)?,
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
