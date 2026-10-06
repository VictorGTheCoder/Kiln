use crate::{FrozenSpec, ProjectConfig, Run};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Engine {
    pub repository: PathBuf,
}
impl Engine {
    pub fn open(repository: &Path) -> Result<Self> {
        let repository = fs::canonicalize(repository).context("repository path is inaccessible")?;
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&repository)
            .output()
            .context("Git is required to access the repository")?;
        if !output.status.success() {
            bail!("repository must be an accessible Git worktree");
        }
        let root = fs::canonicalize(String::from_utf8(output.stdout)?.trim())?;
        Ok(Self { repository: root })
    }
    pub fn prepare(&self, config_path: &Path, spec_paths: &[PathBuf]) -> Result<Run> {
        let config = ProjectConfig::load(&self.repository.join(config_path), &self.repository)?;
        if spec_paths.is_empty() {
            bail!("provide at least one approved Markdown spec");
        }
        let revision = Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(&self.repository)
            .output()?;
        let revision = revision
            .status
            .success()
            .then(|| String::from_utf8_lossy(&revision.stdout).trim().to_owned());
        let mut specs = Vec::new();
        let mut paths = std::collections::HashSet::new();
        for path in spec_paths {
            let source = fs::canonicalize(self.repository.join(path))
                .with_context(|| format!("read spec {}", path.display()))?;
            if !source.starts_with(&self.repository) {
                bail!("spec {} must be inside the repository", path.display());
            }
            if !matches!(
                source.extension().and_then(|s| s.to_str()),
                Some("md" | "markdown")
            ) {
                bail!(
                    "spec {} must be Markdown (.md or .markdown)",
                    path.display()
                );
            }
            if !paths.insert(source.clone()) {
                bail!("duplicate spec {}", path.display());
            }
            let content =
                fs::read_to_string(&source).context("spec must be readable UTF-8 Markdown")?;
            if config.isolation.redact(&content) != content {
                bail!("approved spec contains a registered secret value; remove it before freezing inputs");
            }
            if content.trim().is_empty() {
                bail!("spec {} is empty", path.display());
            }
            specs.push(FrozenSpec {
                path: source
                    .strip_prefix(&self.repository)?
                    .to_string_lossy()
                    .into_owned(),
                content_sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
                content,
                source_revision: revision.clone(),
            });
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let id = format!("run-{}-{}", now.as_nanos(), std::process::id());
        let run = Run {
            schema_version: 1,
            id,
            repository: self.repository.to_string_lossy().into_owned(),
            created_unix_ms: now.as_millis(),
            status: "prepared".into(),
            config,
            specs,
            plan: None,
            imported_issues: Vec::new(),
            integration_branch: None,
            sessions: Vec::new(),
        };
        self.save(&run)?;
        Ok(run)
    }
    fn runs_dir(&self) -> PathBuf {
        self.repository.join(".kiln/runs")
    }
    pub fn save(&self, run: &Run) -> Result<()> {
        validate_id(&run.id)?;
        fs::create_dir_all(self.runs_dir())
            .context("cannot create durable state; check repository write access")?;
        let target = self.runs_dir().join(format!("{}.json", run.id));
        let staging = self
            .runs_dir()
            .join(format!("{}.{}.tmp", run.id, std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .context("create atomic state staging file")?;
        let serialized = serde_json::to_string_pretty(run)?;
        file.write_all(run.config.isolation.redact(&serialized).as_bytes())?;
        file.sync_all()?;
        fs::rename(&staging, target)?;
        fs::File::open(self.runs_dir())?.sync_all()?;
        Ok(())
    }
    pub fn inspect(&self, id: &str) -> Result<Run> {
        validate_id(id)?;
        let run: Run = serde_json::from_slice(
            &fs::read(self.runs_dir().join(format!("{id}.json")))
                .with_context(|| format!("run '{id}' was not found or cannot be read"))?,
        )
        .context("recorded run state is invalid")?;
        if run.schema_version != 1 {
            bail!("unsupported run state schema {}", run.schema_version);
        }
        Ok(run)
    }
    pub fn list(&self) -> Result<Vec<String>> {
        if !self.runs_dir().exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.runs_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(id.to_owned());
                }
            }
        }
        ids.sort();
        Ok(ids)
    }
}
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("invalid run identity; use an identity returned by prepare or inspect");
    }
    Ok(())
}
