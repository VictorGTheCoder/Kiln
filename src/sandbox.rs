//! Mandatory OS isolation. Runtime authorization delegates every executable in /usr.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsolationPolicy {
    pub network: String,
    pub runtime: String,
    pub commands: Vec<Vec<String>>,
    #[serde(default)]
    pub secrets: BTreeMap<String, Vec<String>>,
}
impl IsolationPolicy {
    pub fn validate(&self, worktree: &Path) -> Result<()> {
        if !matches!(self.network.as_str(), "none" | "allow-all") {
            bail!(
                "isolation.network must be none or allow-all; domain restrictions are unsupported"
            );
        }
        if self.runtime != "system" {
            bail!("isolation.runtime must explicitly authorize system interpreters and tools");
        }
        for (name, roles) in &self.secrets {
            if name.is_empty()
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                || roles
                    .iter()
                    .any(|r| !matches!(r.as_str(), "agent" | "build" | "test" | "startup"))
            {
                bail!("invalid secret reference or role");
            }
            if std::env::var(name).map_or(true, |v| v.is_empty()) {
                bail!("configured secret {name} is unavailable");
            }
        }
        let output = Sandbox::base(self, worktree, &[])?
            .args(["--", "/usr/bin/true"])
            .output()
            .context("mandatory bwrap isolation is unavailable")?;
        if !output.status.success() {
            bail!(
                "mandatory bwrap isolation readiness failed: {}",
                self.redact(&String::from_utf8_lossy(&output.stderr))
            );
        }
        Ok(())
    }
    pub fn redact(&self, text: &str) -> String {
        let mut result = text.to_owned();
        for name in self.secrets.keys() {
            if let Ok(value) = std::env::var(name) {
                if !value.is_empty() {
                    result = result.replace(&value, "[REDACTED]");
                    let escaped = serde_json::to_string(&value).unwrap();
                    result = result.replace(&escaped[1..escaped.len() - 1], "[REDACTED]");
                }
            }
        }
        result
    }
}
/// Additional mounts are trusted adapter-owned scoped runtime/auth paths, never user argv.
pub struct Mount {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub writable: bool,
}
pub struct Sandbox;
impl Sandbox {
    fn base(policy: &IsolationPolicy, worktree: &Path, mounts: &[Mount]) -> Result<Command> {
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .env_clear()
            .args(["--unshare-all", "--die-with-parent", "--new-session"]);
        if policy.network == "allow-all" {
            command.arg("--share-net");
        }
        for root in ["/usr", "/lib", "/lib64", "/bin"] {
            if Path::new(root).exists() {
                command.args(["--ro-bind", root, root]);
            }
        }
        command.args([
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--dir",
            "/home",
            "--dir",
            "/home/kiln",
            "--dir",
            "/etc",
        ]);
        for file in ["/etc/ssl", "/etc/resolv.conf", "/etc/hosts"] {
            if Path::new(file).exists() {
                command.arg("--ro-bind").arg(file).arg(file);
            }
        }
        command.arg("--bind").arg(worktree).arg(worktree);
        let git_file = worktree.join(".git");
        if git_file.is_file() {
            let content = std::fs::read_to_string(&git_file)?;
            let dir = PathBuf::from(
                content
                    .trim()
                    .strip_prefix("gitdir: ")
                    .context("invalid worktree Git metadata")?,
            );
            let common = std::fs::canonicalize(
                dir.join(std::fs::read_to_string(dir.join("commondir"))?.trim()),
            )?;
            command.arg("--ro-bind").arg(&common).arg(&common);
            command.arg("--ro-bind").arg(&dir).arg(&dir);
        }
        for mount in mounts {
            command
                .arg(if mount.writable {
                    "--bind"
                } else {
                    "--ro-bind"
                })
                .arg(&mount.source)
                .arg(&mount.destination);
        }
        command.arg("--chdir").arg(worktree).args([
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "HOME",
            "/home/kiln",
            "--setenv",
            "LANG",
            "C.UTF-8",
            "--setenv",
            "GIT_CONFIG_COUNT",
            "1",
            "--setenv",
            "GIT_CONFIG_KEY_0",
            "safe.directory",
            "--setenv",
            "GIT_CONFIG_VALUE_0",
            "*",
        ]);
        Ok(command)
    }
    pub fn command(
        policy: &IsolationPolicy,
        worktree: &Path,
        role: &str,
        argv: &[String],
        mounts: &[Mount],
    ) -> Result<Command> {
        if argv.is_empty() || !policy.commands.contains(&argv.to_vec()) {
            bail!("isolation denied unauthorized {role} command argv");
        }
        let mut command = Self::base(policy, worktree, mounts)?;
        for (name, roles) in &policy.secrets {
            if roles.iter().any(|r| r == role) {
                command.env(
                    name,
                    std::env::var(name)
                        .with_context(|| format!("configured secret {name} is unavailable"))?,
                );
            }
        }
        command.arg("--").args(argv);
        Ok(command)
    }
}
