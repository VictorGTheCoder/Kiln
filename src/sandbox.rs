//! Mandatory OS isolation. Runtime authorization delegates every executable in /usr.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub(crate) struct CommandCancellation(Arc<AtomicBool>);
impl CommandCancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

thread_local! {
    static COMMAND_CANCELLATION: std::cell::RefCell<Option<CommandCancellation>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn with_command_cancellation<T>(
    cancellation: CommandCancellation,
    operation: impl FnOnce() -> T,
) -> T {
    COMMAND_CANCELLATION.with(|current| {
        let previous = current.replace(Some(cancellation));
        let result = operation();
        current.replace(previous);
        result
    })
}

pub(crate) fn command_cancellation_active() -> bool {
    COMMAND_CANCELLATION.with(|current| {
        current
            .borrow()
            .as_ref()
            .is_some_and(CommandCancellation::is_cancelled)
    })
}

fn output(mut command: Command, cancellation: Option<CommandCancellation>) -> Result<Output> {
    let Some(cancellation) = cancellation else {
        return Ok(command.output()?);
    };
    if cancellation.is_cancelled() {
        bail!("project command cancelled by run stop policy");
    }
    use std::os::unix::process::CommandExt;
    command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let pid = child.id() as i32;
    let mut stdout = child
        .stdout
        .take()
        .context("project command stdout is unavailable")?;
    let mut stderr = child
        .stderr
        .take()
        .context("project command stderr is unavailable")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let stdout_sender = sender.clone();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout_sender.send((true, stdout.read_to_end(&mut bytes).map(|_| bytes)));
    });
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = sender.send((false, stderr.read_to_end(&mut bytes).map(|_| bytes)));
    });
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;
    let mut termination_started = None;
    loop {
        while let Ok((is_stdout, result)) = receiver.try_recv() {
            if is_stdout {
                stdout = Some(result?);
            } else {
                stderr = Some(result?);
            }
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        if cancellation.is_cancelled() && termination_started.is_none() {
            unsafe { libc::kill(-pid, libc::SIGTERM) };
            termination_started = Some(Instant::now());
        }
        if termination_started
            .is_some_and(|started| started.elapsed() >= Duration::from_millis(150))
        {
            // Kill the group even when its leader has exited: descendants may still
            // be running and holding the captured output pipes open.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            if status.is_none() {
                status = Some(child.wait()?);
            }
            termination_started = None;
        }
        if status.is_some() && stdout.is_some() && stderr.is_some() {
            break;
        }
        if cancellation.is_cancelled() && termination_started.is_none() {
            bail!("project command cancelled by run stop policy");
        }
        thread::sleep(Duration::from_millis(10));
    }
    if cancellation.is_cancelled() {
        bail!("project command cancelled by run stop policy");
    }
    Ok(Output {
        status: status.expect("command status checked before leaving loop"),
        stdout: stdout.expect("stdout checked before leaving loop"),
        stderr: stderr.expect("stderr checked before leaving loop"),
    })
}

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
            if matches!(
                name.as_str(),
                "PATH" | "HOME" | "ENV" | "BASH_ENV" | "SHELLOPTS" | "TMPDIR"
            ) || name.starts_with("LD_")
                || name.starts_with("GIT_")
                || name.starts_with("BWRAP_")
                || name.is_empty()
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
    pub(crate) fn output(command: Command) -> Result<Output> {
        let cancellation = COMMAND_CANCELLATION.with(|current| current.borrow().clone());
        output(command, cancellation)
    }

    pub(crate) fn check(
        policy: &IsolationPolicy,
        worktree: &Path,
        name: &str,
        argv: &[String],
    ) -> crate::execution::CheckResult {
        let result =
            Self::supervised_command(policy, worktree, name, argv, &[]).and_then(Self::output);
        let (exit_code, stdout, stderr, passed) = match result {
            Ok(output) => (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
                output.status.success(),
            ),
            Err(error) => (None, String::new(), error.to_string(), false),
        };
        crate::execution::CheckResult {
            name: name.into(),
            command: argv.to_vec(),
            exit_code,
            stdout: policy.redact(&stdout),
            stderr: policy.redact(&stderr),
            passed,
        }
    }

    fn base(policy: &IsolationPolicy, worktree: &Path, mounts: &[Mount]) -> Result<Command> {
        Self::base_with_session(policy, worktree, mounts, true)
    }
    fn base_with_session(
        policy: &IsolationPolicy,
        worktree: &Path,
        mounts: &[Mount],
        new_session: bool,
    ) -> Result<Command> {
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .env_clear()
            .args(["--unshare-all", "--die-with-parent"]);
        if new_session {
            command.arg("--new-session");
        }
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
        Self::command_with_session(policy, worktree, role, argv, mounts, true)
    }
    /// The caller must assign a dedicated process group before spawning. Keeping
    /// the PID namespace init in that group makes cancellation tear down every
    /// sandbox descendant, even descendants that create their own sessions.
    pub(crate) fn supervised_command(
        policy: &IsolationPolicy,
        worktree: &Path,
        role: &str,
        argv: &[String],
        mounts: &[Mount],
    ) -> Result<Command> {
        Self::command_with_session(policy, worktree, role, argv, mounts, false)
    }
    fn command_with_session(
        policy: &IsolationPolicy,
        worktree: &Path,
        role: &str,
        argv: &[String],
        mounts: &[Mount],
        new_session: bool,
    ) -> Result<Command> {
        if argv.is_empty() || !policy.commands.contains(&argv.to_vec()) {
            bail!("isolation denied unauthorized {role} command argv");
        }
        let mut command = Self::base_with_session(policy, worktree, mounts, new_session)?;
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
