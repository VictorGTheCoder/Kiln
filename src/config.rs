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
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                    .map(|p| repository.join(p).join(program))
                    .collect()
            };
            if !candidate.iter().any(|p| executable(p)) {
                bail!("{name} executable '{program}' is unavailable; install it or correct the configured command");
            }
        }
        for argv in [&config.build, &config.test, &config.startup] {
            if !config.isolation.commands.contains(argv) {
                bail!("isolation denied unauthorized configured command");
            }
        }
        config.isolation.validate(repository)?;
        Ok(config)
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
