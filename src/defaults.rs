//! Default inference for user-facing commands and dashboard actions.
//!
//! Inside a target repository the operator should not retype what Kiln can
//! find itself. Every inferred value has an explicit override:
//!
//! | value              | override        | inferred from                                  |
//! |--------------------|-----------------|------------------------------------------------|
//! | repository         | `--repo`        | the current directory (its Git worktree root)  |
//! | project config     | `--config`      | `kiln.json` in the repository                  |
//! | GitHub repository  | `--github-repo` | the `origin` remote (HTTPS or SSH form)        |
//! | provider           | `--codex` / `--claude` | the config `agent` field resolved on PATH, else codex then claude on PATH |
use anyhow::{bail, Context, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Project configuration file name looked up in the repository.
pub const CONFIG_FILE: &str = "kiln.json";

/// Values supplied explicitly on the command line; each wins over inference.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub github_repo: Option<String>,
    pub codex: Option<PathBuf>,
    pub claude: Option<PathBuf>,
}

/// Coding agent family driving provider sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Codex,
    Claude,
}
impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }
}

/// The provider a command will use and the executable that runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChoice {
    pub agent: Agent,
    pub executable: PathBuf,
}
impl ProviderChoice {
    /// `(codex, claude)` executable paths in the shape the provider flags use.
    pub fn into_flags(self) -> (Option<PathBuf>, Option<PathBuf>) {
        match self.agent {
            Agent::Codex => (Some(self.executable), None),
            Agent::Claude => (None, Some(self.executable)),
        }
    }
}

/// Resolved run inputs for a repository. The provider is resolved separately
/// (see [`Defaults::provider`]) because fixture-driven runs never need one.
#[derive(Debug, Clone)]
pub struct Defaults {
    pub repository: PathBuf,
    /// Config path as commands pass it to the engine (joined to the repository).
    pub config: PathBuf,
    pub github_repo: String,
    overrides: Overrides,
}
impl Defaults {
    /// Infer the config and GitHub repository of `repository` (a Git worktree root).
    pub fn infer(repository: &Path, overrides: Overrides) -> Result<Self> {
        let config = config_path(repository, overrides.config.as_deref())?;
        let github_repo = match &overrides.github_repo {
            Some(name) => name.clone(),
            None => origin_github_repository(repository)?,
        };
        Ok(Self {
            repository: repository.to_owned(),
            config,
            github_repo,
            overrides,
        })
    }
    /// Provider from explicit flags, else the config `agent` field resolved on
    /// PATH, else codex then claude from PATH.
    pub fn provider(&self) -> Result<ProviderChoice> {
        provider(
            &self.repository,
            &self.config,
            self.overrides.codex.clone(),
            self.overrides.claude.clone(),
            std::env::var_os("PATH").as_deref(),
        )
    }
}

fn config_path(repository: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    let config = explicit.map_or_else(|| PathBuf::from(CONFIG_FILE), Path::to_path_buf);
    if !repository.join(&config).is_file() {
        match explicit {
            Some(path) => bail!(
                "project configuration {} does not exist; check the --config path",
                repository.join(path).display()
            ),
            None => bail!(
                "no {CONFIG_FILE} found in {}; create one with `kiln init` (or write it by hand, see the README), or pass --config PATH",
                repository.display()
            ),
        }
    }
    Ok(config)
}

fn origin_github_repository(repository: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repository)
        .output()
        .context("Git is required to read the origin remote")?;
    if !output.status.success() {
        bail!(
            "cannot infer the GitHub repository: {} has no `origin` remote; add one with `git remote add origin git@github.com:OWNER/REPO.git`, or pass --github-repo OWNER/REPO",
            repository.display()
        );
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    parse_github_remote(&url).with_context(|| {
        format!(
            "cannot infer the GitHub repository: origin remote `{url}` is not a github.com repository URL; pass --github-repo OWNER/REPO"
        )
    })
}

/// `OWNER/REPO` of a github.com remote URL in HTTPS, SSH or scp-like form.
pub fn parse_github_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let (authority, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        // scp-like `[user@]host:path`
        None => url.split_once(':')?,
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}"))
}

fn provider(
    repository: &Path,
    config_path: &Path,
    codex: Option<PathBuf>,
    claude: Option<PathBuf>,
    path_env: Option<&std::ffi::OsStr>,
) -> Result<ProviderChoice> {
    match (codex, claude) {
        (Some(_), Some(_)) => bail!("--codex and --claude are mutually exclusive"),
        (Some(executable), None) => {
            return Ok(ProviderChoice {
                agent: Agent::Codex,
                executable,
            })
        }
        (None, Some(executable)) => {
            return Ok(ProviderChoice {
                agent: Agent::Claude,
                executable,
            })
        }
        (None, None) => (),
    }
    let config_path = repository.join(config_path);
    let config = crate::ProjectConfig::load(&config_path, repository)?;
    let locate = |agent: Agent| -> Option<PathBuf> {
        on_path(agent.name(), path_env).or_else(|| {
            // An installation pinned in the provider section still works without PATH.
            config
                .provider_installation(agent)
                .filter(|path| is_executable(path))
                .map(Path::to_path_buf)
        })
    };
    if let Some(agent) = config.agent()? {
        let executable = locate(agent).with_context(|| {
            format!(
                "provider `{name}` (\"agent\" in {config}) was not found on PATH; install it, or pass --{name} PATH",
                name = agent.name(),
                config = config_path.display()
            )
        })?;
        return Ok(ProviderChoice { agent, executable });
    }
    [Agent::Codex, Agent::Claude]
        .into_iter()
        .find_map(|agent| locate(agent).map(|executable| ProviderChoice { agent, executable }))
        .context(
            "no provider found: neither `codex` nor `claude` is on PATH; install one, set \"agent\": \"codex\" or \"claude\" in kiln.json, or pass --codex PATH or --claude PATH",
        )
}

fn on_path(name: &str, path_env: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path_env?)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
