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
