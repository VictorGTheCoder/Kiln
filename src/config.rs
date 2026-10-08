use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// Commands are argv arrays, never implicit shell programs. Extra project policy
/// fields are preserved for later execution and limit configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub isolation: crate::sandbox::IsolationPolicy,
    pub build: Vec<String>,
    pub test: Vec<String>,
    pub startup: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}
impl ProjectConfig {
    pub fn load(path: &Path, repository: &Path) -> Result<Self> {
        let config: Self = serde_json::from_slice(&std::fs::read(path).with_context(|| format!("read project configuration {}", path.display()))?)
            .context("configuration must be JSON with build, test, startup argv arrays and acceptance_criteria")?;
        config.agent()?;
        for name in ["correction_cycles", "implementation_concurrency"] {
            if let Some(limit) = config.extensions.get(name) {
                if limit.as_u64().is_none_or(|n| n == 0) {
                    bail!("{name} must be a positive integer");
                }
            }
        }
        if config.acceptance_criteria.is_empty()
            || config
                .acceptance_criteria
                .iter()
                .any(|s| s.trim().is_empty())
        {
            bail!("acceptance_criteria must contain at least one nonempty criterion");
        }
        for (name, command) in [
            ("build", &config.build),
            ("test", &config.test),
            ("startup", &config.startup),
        ] {
            if command.is_empty()
                || command[0].trim().is_empty()
                || command.iter().any(|s| s.contains('\0'))
            {
                bail!("{name} must be a nonempty argv array without NUL characters");
            }
            let program = &command[0];
            let candidate = if Path::new(program).components().count() > 1 {
                vec![repository.join(program)]
            } else {
                [Path::new("/usr/bin"), Path::new("/bin")]
                    .iter()
                    .map(|p| p.join(program))
                    .collect()
            };
            if !candidate.iter().any(|p| executable(p)) {
                bail!("{name} executable '{program}' is unavailable in the sandbox (PATH is /usr/bin:/bin); install it in a mounted system path or configure a repository-relative executable");
            }
        }
        for argv in [&config.build, &config.test, &config.startup] {
            if !config.isolation.commands.contains(argv) {
                bail!("isolation denied unauthorized configured command");
            }
        }
        crate::limits::RunLimits::from_config(&config)?;
        crate::validation::ValidationSettings::from_config(&config)?;
        crate::publication::PublicationSettings::from_config(&config)?;
        let serialized = serde_json::to_string(&config)?;
        if config.isolation.redact(&serialized) != serialized {
            bail!("public configuration contains a registered secret value; use secret variable references only");
        }
        config.isolation.validate(repository)?;
        Ok(config)
    }

    /// The optional `agent` field naming the provider family, validated.
    pub fn agent(&self) -> Result<Option<crate::defaults::Agent>> {
        match self.extensions.get("agent") {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(value) => value
                .as_str()
                .and_then(crate::defaults::Agent::parse)
                .map(Some)
                .with_context(|| format!("agent must be \"codex\" or \"claude\", found {value}")),
        }
    }

    /// The `installation` pinned in a provider section (`codex` / `claude`), if any.
    pub fn provider_installation(&self, agent: crate::defaults::Agent) -> Option<&Path> {
        self.extensions
            .get(agent.name())?
            .get("installation")?
            .as_str()
            .map(Path::new)
    }
}
fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}
