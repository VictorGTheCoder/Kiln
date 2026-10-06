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
    /// Validate configuration and freeze approved Markdown specs. No commands are executed.
    Prepare {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, required = true)]
        spec: Vec<PathBuf>,
    },
    /// Generate and independently verify tickets using a deterministic adapter.
    Plan {
        id: String,
        #[arg(long)]
        fixture: PathBuf,
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
        Commands::Prepare { config, spec } => {
            serde_json::to_value(engine.prepare(&config, &spec)?)?
        }
        Commands::Plan { id, fixture } => {
            let agent = kiln::planning::FixtureAgent::load(&engine.repository.join(fixture))?;
            let run = engine.plan(&id, &agent)?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            if !run.plan.as_ref().is_some_and(|p| p.executable) {
                anyhow::bail!("plan rejected; inspect the recorded findings");
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
