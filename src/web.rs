//! Local monitoring and control interface. Pages are rendered from durable run
//! state; workflow execution uses the same CLI start/resume orchestration.
use crate::{Engine, Run};
use anyhow::{bail, Result};
use std::{
    fmt::Write as _,
    fs,
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
use tiny_http::{Header, Method, Request, Response, Server};

/// Seconds between automatic reloads of a run that is still recorded as running.
const REFRESH_SECONDS: u32 = 3;

/// Fixed local configuration for web-launched whole-backlog workflows. Request
/// data selects only a fixed action and run id; it cannot replace any setting.
#[derive(Debug, Clone)]
pub struct BacklogLaunch {
    pub config: PathBuf,
    pub github_repo: String,
    pub issue_fixture: Option<PathBuf>,
    pub planning_fixture: Option<PathBuf>,
    pub run_fixture: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    pub claude: Option<PathBuf>,
    pub publication_fixture: Option<PathBuf>,
    pub gh: Option<PathBuf>,
    pub repair_fixture: Option<PathBuf>,
}

/// Number of successive ports tried after a taken one.
const PORT_ATTEMPTS: u16 = 100;
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; img-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// What a web server shows at `/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// The server-rendered runs list (`kiln serve`).
    Runs,
    /// The dashboard page (`kiln dashboard`, `kiln start`); the runs list
    /// stays at `/runs`.
    Dashboard,
}

/// A bound loopback web server, not yet serving requests.
pub struct WebServer {
    server: Server,
    address: SocketAddr,
    actions: Option<crate::actions::Fixtures>,
}
impl WebServer {
    /// Bind exactly `bind`, which must be a loopback address.
    pub fn bind(bind: SocketAddr) -> Result<Self> {
        Self::bind_with(bind, 1)
    }
    /// Bind `bind`, or the next free port after it when it is taken.
    pub fn bind_next_free(bind: SocketAddr) -> Result<Self> {
        Self::bind_with(bind, PORT_ATTEMPTS)
    }
    fn bind_with(bind: SocketAddr, attempts: u16) -> Result<Self> {
        if !bind.ip().is_loopback() {
            bail!("the local web view must bind to a loopback address");
        }
        let mut address = bind;
        let mut tried = 0;
        loop {
            match Server::http(address) {
                Ok(server) => {
                    let address = server.server_addr().to_ip().unwrap_or(address);
                    return Ok(Self {
                        server,
                        address,
                        actions: None,
                    });
                }
                Err(error) => {
                    tried += 1;
                    let in_use = error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
                    if !in_use || tried >= attempts || address.port() == 0 {
                        bail!("cannot start local web view: {error}");
                    }
                    let Some(next) = address.port().checked_add(1) else {
                        bail!("cannot start local web view: no free port after {bind}");
                    };
                    address.set_port(next);
                }
            }
        }
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }
    /// Enable the dashboard action buttons; launched commands get `fixtures`.
    pub fn with_actions(mut self, fixtures: crate::actions::Fixtures) -> Self {
        self.actions = Some(fixtures);
        self
    }
    /// Handle requests until `stop` is set (checked at least every 200 ms).
    pub fn serve(
        self,
        engine: Engine,
        backlog: Option<BacklogLaunch>,
        home: Home,
        stop: &AtomicBool,
    ) -> Result<()> {
        let site = Site {
            actions: self
                .actions
                .map(|fixtures| crate::actions::Actions::new(engine.clone(), fixtures)),
            engine,
            backlog,
            home,
            port: self.address.port(),
        };
        while !stop.load(Ordering::Relaxed) {
            if let Some(request) = self.server.recv_timeout(Duration::from_millis(200))? {
                site.handle(request);
            }
        }
        Ok(())
    }
}

pub fn serve(engine: Engine, bind: SocketAddr) -> Result<()> {
    serve_with_backlog(engine, bind, None)
}

pub fn serve_with_backlog(
    engine: Engine,
    bind: SocketAddr,
    backlog: Option<BacklogLaunch>,
) -> Result<()> {
    let server = WebServer::bind(bind)?;
    println!("Kiln web view: {}", server.url());
    std::io::stdout().flush()?;
    let _announcement = crate::dashboard::Announcement::new(&engine.repository, &server.url())?;
    server.serve(engine, backlog, Home::Runs, &AtomicBool::new(false))
}

/// Request routing shared by every request of one server.
struct Site {
    engine: Engine,
    backlog: Option<BacklogLaunch>,
    home: Home,
    port: u16,
    /// Dashboard action buttons, when enabled.
    actions: Option<crate::actions::Actions>,
}
impl Site {
    /// Answer one request. Requests are answered one at a time on the server
    /// thread, except event streams, which move to their own thread.
    fn handle(&self, request: Request) {
        if let Some(stream) = self.event_stream(&request) {
            let cursor = crate::event_stream::Cursor::parse(
                request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Last-Event-ID"))
                    .map(|h| h.value.as_str()),
            );
            let out = request.into_writer();
            thread::spawn(move || {
                let headers = [
                    ("Cache-Control", "no-store"),
                    ("X-Content-Type-Options", "nosniff"),
                    ("Content-Security-Policy", CONTENT_SECURITY_POLICY),
                ];
                let _ = stream.serve(out, cursor, &headers, redact_for_web);
            });
            return;
        }
        let (status, content_type, body) = self.respond(&request);
        let response = Response::from_string(body)
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", content_type).unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
            .with_header(Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap())
            .with_header(
                Header::from_bytes("Content-Security-Policy", CONTENT_SECURITY_POLICY).unwrap(),
            );
        let _ = request.respond(response);
    }
    /// The event stream a valid `GET /api/dashboard/runs/<id>/events` asks
    /// for; `None` for any other request, which [`Self::respond`] answers
    /// (with 400 or 404 for an invalid stream request).
    fn event_stream(&self, request: &Request) -> Option<crate::event_stream::EventStream> {
        let path = request.url().split('?').next().unwrap_or("/");
        let id = path
            .strip_prefix("/api/dashboard/runs/")?
            .strip_suffix("/events")?;
        if request.method() != &Method::Get || !self.valid_host(request) {
            return None;
        }
        validate_id(id).ok()?;
        crate::event_stream::EventStream::open(&self.engine, id).ok()
    }
    fn valid_host(&self, request: &Request) -> bool {
        let expected_hosts = [
            format!("localhost:{}", self.port),
            format!("127.0.0.1:{}", self.port),
            format!("[::1]:{}", self.port),
        ];
        request.headers().iter().any(|header| {
            header.field.equiv("Host")
                && expected_hosts
                    .iter()
                    .any(|host| header.value.as_str().eq_ignore_ascii_case(host))
        })
    }
    fn respond(&self, request: &Request) -> (u16, &'static str, String) {
        let engine = &self.engine;
        let backlog = self.backlog.as_ref();
        let valid_host = self.valid_host(request);
        let request_host = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("Host"))
            .map(|h| h.value.as_str());
        if !valid_host {
            (
                400,
                "text/plain; charset=utf-8",
                "Invalid Host header".to_owned(),
            )
        } else if request.method() == &Method::Post {
            let origin_ok = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Origin"))
                .is_some_and(|h| {
                    request_host.is_some_and(|host| {
                        format!("http://{host}").eq_ignore_ascii_case(h.value.as_str())
                    })
                });
            let content_type_ok = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Type"))
                .is_some_and(|h| {
                    h.value
                        .as_str()
                        .eq_ignore_ascii_case("application/x-www-form-urlencoded")
                });
            let empty_body = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Length"))
                .is_some_and(|h| h.value.as_str() == "0");
            if !origin_ok {
                (
                    403,
                    "text/plain; charset=utf-8",
                    "Invalid Origin".to_owned(),
                )
            } else if !content_type_ok || !empty_body {
                (
                    400,
                    "text/plain; charset=utf-8",
                    "Invalid request body".to_owned(),
                )
            } else if let Some((status, body)) = self.act(request.url()) {
                (status, "application/json", body.to_string())
            } else {
                match mutate(engine, backlog, request.url().split('?').next().unwrap_or("/")) {
                    Ok(body) => (200, "text/html; charset=utf-8", body),
                    Err(error) => (409, "text/html; charset=utf-8", format!("<!doctype html><title>Action unavailable</title><p>{}</p><p><a href=\"/\">Back to runs</a></p>", esc(&format!("{error:#}")))),
                }
            }
        } else if request.method() != &Method::Get {
            (
                405,
                "text/plain; charset=utf-8",
                "Only GET and POST are supported".to_owned(),
            )
        } else {
            let path = request.url().split('?').next().unwrap_or("/");
            let offered = (path == "/api/dashboard/actions").then(|| match &self.actions {
                Some(actions) => actions
                    .offered()
                    .map(|offered| ("application/json", offered.to_string())),
                None => Ok((
                    "application/json",
                    serde_json::json!({"available": [], "runs": {}, "last": null}).to_string(),
                )),
            });
            let rendered = match offered.or_else(|| dashboard_route(engine, self.home, path)) {
                Some(result) => result,
                None => render(engine, path, backlog),
            };
            match rendered {
                Ok((kind, body)) => (200, kind, body),
                Err(error) => (404, "text/plain; charset=utf-8", format!("Kiln: {error:#}")),
            }
        }
    }
    /// A dashboard action POST, answered with JSON; `None` for other paths.
    fn act(&self, url: &str) -> Option<(u16, serde_json::Value)> {
        let path = url.split('?').next().unwrap_or("/");
        if !path.starts_with("/api/dashboard/") {
            return None;
        }
        Some(match &self.actions {
            Some(actions) => actions.perform(path).unwrap_or_else(|| {
                (
                    404,
                    serde_json::json!({"error": "unknown dashboard action"}),
                )
            }),
            None => (
                409,
                serde_json::json!({"error": "dashboard actions are not enabled on this server; run `kiln dashboard`"}),
            ),
        })
    }
}

/// Dashboard page, script and JSON endpoints; `None` for other paths.
fn dashboard_route(
    engine: &Engine,
    home: Home,
    path: &str,
) -> Option<Result<(&'static str, String)>> {
    use crate::dashboard;
    let page = || Ok(("text/html; charset=utf-8", dashboard::PAGE.to_owned()));
    Some(match path {
        "/dashboard" => page(),
        "/" if home == Home::Dashboard => page(),
        "/dashboard.js" => Ok((
            "text/javascript; charset=utf-8",
            dashboard::SCRIPT.to_owned(),
        )),
        "/api/dashboard/runs" => dashboard::runs(engine)
            .and_then(|runs| Ok(("application/json", serde_json::to_string(&runs)?))),
        _ => {
            let id = path.strip_prefix("/api/dashboard/runs/")?;
            validate_id(id).and_then(|()| {
                let run = engine.inspect(id)?;
                let body = serde_json::to_string(&dashboard::Board::of(&run))?;
                Ok(("application/json", redact_for_web(&run, &body)))
            })
        }
    })
}
fn render(
    engine: &Engine,
    path: &str,
    backlog: Option<&BacklogLaunch>,
) -> Result<(&'static str, String)> {
    if let Some(id) = path.strip_prefix("/api/runs/") {
        validate_id(id)?;
        let run = engine.inspect(id)?;
        let json = serde_json::to_string_pretty(&run)?;
        return Ok(("application/json", redact_for_web(&run, &json)));
    }
    if path == "/api/runs" {
        let mut values = Vec::new();
        for id in engine.list()? {
            if let Ok(run) = engine.inspect(&id) {
                let json = serde_json::to_string(&run)?;
                values.push(redact_for_web(&run, &json));
            }
        }
        return Ok(("application/json", format!("[{}]", values.join(","))));
    }
    let (title, refresh, content) = if path == "/" || path == "/runs" {
        let mut content = "<h1>Kiln workflow runs</h1><ul>".to_owned();
        if backlog.is_some() {
            content = "<h1>Kiln workflow runs</h1><form method=\"post\" action=\"/actions/start\"><button type=\"submit\">Start backlog run</button><p class=\"muted\">Plans and runs all open issues using the configured local workflow.</p></form><ul>".to_owned();
        }
        for id in engine.list()? {
            let status = engine
                .inspect(&id)
                .map(|run| run.status)
                .unwrap_or_else(|_| "unreadable".into());
            let _ = write!(
                content,
                "<li><a href=\"/runs/{id}\">{id}</a> {}</li>",
                badge(&status),
                id = esc(&id)
            );
        }
        content.push_str("</ul>");
        (
            "Kiln workflow runs".to_owned(),
            backlog.is_some() && web_worker_active(engine),
            content,
        )
    } else if let Some(id) = path.strip_prefix("/runs/") {
        validate_id(id)?;
        let run = engine.inspect(id)?;
        let live = liveness(engine, &run.id);
        // Recorded state is already redacted; redact again in case a secret was
        // registered after the state was written.
        let content = redact_for_web(&run, &run_page(&run, live, backlog.is_some()));
        (
            format!("Kiln run {}", run.id),
            should_refresh(&run) || run.backlog.is_some() && web_worker_active(engine),
            content,
        )
    } else {
        bail!("page was not found");
    };
    let refresh = if refresh {
        format!("<meta http-equiv=\"refresh\" content=\"{REFRESH_SECONDS}\">")
    } else {
        String::new()
    };
    Ok((
        "text/html; charset=utf-8",
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">{refresh}<title>{}</title><style>{STYLE}</style></head><body>{content}</body></html>",
            esc(&title)
        ),
    ))
}

fn should_refresh(run: &Run) -> bool {
    if run.status == "running" {
        return true;
    }
    run.backlog.is_some()
        && !matches!(
            run.status.as_str(),
            "paused"
                | "cancelled"
                | "limit_exhausted"
                | "blocked"
                | "failed"
                | "published"
                | "completed"
        )
}

fn web_worker_active(engine: &Engine) -> bool {
    let dir = engine.repository.join(".kiln");
    dir.join("web-backlog-start.lock").exists() || dir.join("web-backlog-resume.lock").exists()
}

pub(crate) fn validate_run_id(id: &str) -> Result<()> {
    validate_id(id)
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 100
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        bail!("invalid run id");
    }
    Ok(())
}

fn mutate(engine: &Engine, launch: Option<&BacklogLaunch>, path: &str) -> Result<String> {
    if path == "/actions/start" {
        let launch = launch.ok_or_else(|| {
            anyhow::anyhow!("backlog start is not configured for this web server")
        })?;
        let executable = std::env::current_exe()?;
        let lock_path = engine.repository.join(".kiln/web-backlog-start.lock");
        fs::create_dir_all(lock_path.parent().unwrap())?;
        let lock = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|error| anyhow::anyhow!("a web backlog start is already active ({error})"))?;
        let mut command = Command::new(executable);
        command
            .arg("--repo")
            .arg(&engine.repository)
            .arg("start-backlog")
            .arg("--config")
            .arg(&launch.config)
            .arg("--github-repo")
            .arg(&launch.github_repo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        append_opt(
            &mut command,
            "--issue-fixture",
            launch.issue_fixture.as_ref(),
        );
        append_opt(
            &mut command,
            "--planning-fixture",
            launch.planning_fixture.as_ref(),
        );
        append_opt(&mut command, "--run-fixture", launch.run_fixture.as_ref());
        append_opt(&mut command, "--codex", launch.codex.as_ref());
        append_opt(&mut command, "--claude", launch.claude.as_ref());
        append_opt(
            &mut command,
            "--publication-fixture",
            launch.publication_fixture.as_ref(),
        );
        append_opt(&mut command, "--gh", launch.gh.as_ref());
        append_opt(
            &mut command,
            "--repair-fixture",
            launch.repair_fixture.as_ref(),
        );
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                drop(lock);
                let _ = fs::remove_file(lock_path);
                return Err(error.into());
            }
        };
        reap_start(lock_path, lock, child);
        return Ok("<!doctype html><meta http-equiv=\"refresh\" content=\"3;url=/\"><title>Backlog run started</title><h1>Backlog run started</h1><p>The complete open-issue graph is being planned and scheduled using the configured Kiln workflow.</p><p><a href=\"/\">View runs</a></p>".into());
    }
    if let Some(rest) = path.strip_prefix("/runs/") {
        let Some((id, action)) = rest.split_once("/control/") else {
            bail!("unknown action")
        };
        validate_id(id)?;
        match action {
            "pause" | "cancel" => engine.request_control(id, action)?,
            "resume" => {
                let run = engine.inspect(id)?;
                if run.status == "cancelled"
                    || run.status == "running" && liveness(engine, id) == Some(true)
                {
                    bail!("run cannot be resumed in its current state");
                }
                let executable = std::env::current_exe()?;
                let lock_path = engine.repository.join(".kiln/web-backlog-resume.lock");
                fs::create_dir_all(lock_path.parent().unwrap())?;
                let lock = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)
                    .map_err(|error| anyhow::anyhow!("a web resume is already active ({error})"))?;
                let mut command = Command::new(executable);
                command
                    .arg("--repo")
                    .arg(&engine.repository)
                    .arg("resume")
                    .arg(id)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if let Some(launch) = launch {
                    append_opt(&mut command, "--fixture", launch.run_fixture.as_ref());
                    append_opt(&mut command, "--codex", launch.codex.as_ref());
                    append_opt(&mut command, "--claude", launch.claude.as_ref());
                    append_opt(
                        &mut command,
                        "--publication-fixture",
                        launch.publication_fixture.as_ref(),
                    );
                    append_opt(&mut command, "--gh", launch.gh.as_ref());
                    append_opt(
                        &mut command,
                        "--repair-fixture",
                        launch.repair_fixture.as_ref(),
                    );
                }
                let child = match command.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        drop(lock);
                        let _ = fs::remove_file(lock_path);
                        return Err(error.into());
                    }
                };
                reap_start(lock_path, lock, child);
            }
            _ => bail!("unknown run action"),
        }
        return Ok(format!("<!doctype html><meta http-equiv=\"refresh\" content=\"3;url=/runs/{}\"><title>Run control requested</title><h1>{}</h1><p>Control sent to the durable Kiln workflow.</p><p><a href=\"/runs/{}\">View run state</a></p>", esc(id), esc(action), esc(id)));
    }
    bail!("unknown action")
}

fn append_opt(command: &mut Command, flag: &str, value: Option<&PathBuf>) {
    if let Some(value) = value {
        command.arg(flag).arg(value);
    }
}

fn reap_start(lock_path: PathBuf, lock: fs::File, mut child: Child) {
    thread::spawn(move || {
        let _lock = lock;
        let _ = child.wait();
        let _ = fs::remove_file(lock_path);
    });
}

fn redact_for_web(run: &Run, text: &str) -> String {
    let mut result = run.config.isolation.redact(text);
    for name in run.config.isolation.secrets.keys() {
        if let Ok(secret) = std::env::var(name) {
            if !secret.is_empty() {
                result = result.replace(&secret, "[REDACTED]");
                result = result.replace(&esc(&secret), "[REDACTED]");
            }
        }
    }
    result
}
const STYLE: &str = "body{font:15px/1.45 system-ui,sans-serif;max-width:1100px;margin:32px auto;padding:0 16px;color:#1f1f1f;background:#fff}h1{font-size:1.6em}h2{margin-top:2em;border-bottom:1px solid #ddd;padding-bottom:4px}table{border-collapse:collapse;width:100%}th,td{text-align:left;vertical-align:top;padding:6px 8px;border-bottom:1px solid #eee;overflow-wrap:anywhere}code,pre{font:13px ui-monospace,monospace}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#f4f4f4;padding:12px;max-height:20em;overflow:auto}a{color:#174a8b}.badge{display:inline-block;padding:1px 8px;border-radius:10px;font-size:.85em;font-weight:600;background:#eceff3;color:#333}.good{background:#dff3e4;color:#14532d}.good::before{content:'\\2713  '}.bad{background:#fde2e1;color:#7f1d1d}.bad::before{content:'\\2717  '}.unknown{background:#fff3c4;color:#713f12;border:1px dashed #a16207}.unknown::before{content:'?  '}.notice{padding:8px 12px;border-left:4px solid #a16207;background:#fffbea}.muted{color:#666}";

/// Whether a scheduler process is known to own the run right now.
pub(crate) fn is_live(engine: &Engine, id: &str) -> bool {
    liveness(engine, id) == Some(true)
}

/// Whether a scheduler process currently holds the run's ownership lock (the
/// same lock `kiln resume` checks). `None` when the system does not report it.
fn liveness(engine: &Engine, id: &str) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let lock = engine
        .repository
        .join(".kiln/runs")
        .join(format!("{id}.owner.lock"));
    let Ok(metadata) = std::fs::metadata(lock) else {
        return Some(false);
    };
    let locks = std::fs::read_to_string("/proc/locks").ok()?;
    let suffix = format!(":{}", metadata.ino());
    Some(locks.lines().any(|line| {
        line.contains("FLOCK") && line.split_whitespace().any(|f| f.ends_with(&suffix))
    }))
}

fn run_page(run: &Run, live: Option<bool>, can_resume: bool) -> String {
    let mut html = String::new();
    let _ = write!(
        html,
        "<p><a href=\"/\">All runs</a> · <a href=\"/api/runs/{id}\">Recorded state (JSON)</a></p><h1>Workflow run <code>{id}</code></h1><p>Status: {}</p>",
        badge(&run.status),
        id = esc(&run.id)
    );
    html.push_str(&match live {
        Some(true) => "<p>Live: a scheduler process owns this run.</p>".to_owned(),
        Some(false) if run.status == "running" => format!(
            "<p class=\"notice\" data-interrupted>Interrupted: the run is recorded as running, but no scheduler process owns it. Resume it with <code>kiln resume {}</code>.</p>",
            esc(&run.id)
        ),
        Some(false) => {
            "<p class=\"muted\">Not live: no scheduler process owns this run.</p>".to_owned()
        }
        None => "<p class=\"muted\">Process liveness is unknown on this system.</p>".to_owned(),
    });
    if live == Some(true) && run.status == "running" {
        html.push_str(&format!("<form method=\"post\" action=\"/runs/{}/control/pause\"><button>Pause run</button></form><form method=\"post\" action=\"/runs/{}/control/cancel\"><button>Cancel run</button></form>", esc(&run.id), esc(&run.id)));
    } else if can_resume
        && (matches!(
            run.status.as_str(),
            "paused" | "limit_exhausted" | "interrupted"
        ) || run.status == "running" && live == Some(false))
    {
        html.push_str(&format!("<form method=\"post\" action=\"/runs/{}/control/resume\"><button>Resume run</button></form>", esc(&run.id)));
    }

    let _ = write!(
        html,
        "<h2>Frozen specs</h2><p data-input-version>Current input version: <strong>{}</strong> (0 = frozen specs; verified revisions increment the version).</p><table><tr><th>Path</th><th>Content SHA-256</th><th>Source revision</th></tr>",
        run.input_version()
    );
    for spec in &run.specs {
        let _ = write!(
            html,
            "<tr><td><code>{}</code><details><summary>Frozen content</summary><pre>{}</pre></details></td><td><code>{}</code></td><td><code>{}</code></td></tr>",
            esc(&spec.path),
            esc(&spec.content),
            esc(&spec.content_sha256),
            esc(spec.source_revision.as_deref().unwrap_or("none (no commit)"))
        );
    }
    html.push_str("</table>");
    if !run.spec_revisions.is_empty() {
        html.push_str("<p>Verified revisions are part of the current input version; frozen inputs remain recorded above.</p><table><tr><th>Revision</th><th>Path</th><th>Status</th><th>Content SHA-256</th><th>Recorded by</th></tr>");
        for r in &run.spec_revisions {
            let source = if let Some(replan_id) = &r.replan_id {
                format!("Recorded by spec replan <code>{}</code>", esc(replan_id))
            } else if !r.decision_id.is_empty() {
                format!("Decision <code>{}</code>", esc(&r.decision_id))
            } else {
                "No decision or replan recorded".into()
            };
            let _ = write!(
                html,
                "<tr><td>Revision {}</td><td><code>{}</code><details><summary>Revised content</summary><pre>{}</pre></details></td><td>{}</td><td><code>{}</code><br><span class=\"muted\">replaces <code>{}</code></span></td><td>{}</td></tr>",
                r.version,
                esc(&r.path),
                esc(&r.content),
                badge(&r.status),
                esc(&r.content_sha256),
                esc(&r.base_sha256),
                source
            );
        }
        html.push_str("</table>");
    }
    if !run.spec_replans.is_empty() {
        html.push_str("<h2>Spec replan history</h2><table><tr><th>Replan</th><th>Outcome</th><th>Input version</th><th>Changed requirements</th><th>Affected tickets</th><th>Dependent tickets</th><th>Invalidated validation</th><th>Revalidations</th></tr>");
        for replan in &run.spec_replans {
            let changed = replan
                .changed_requirements
                .iter()
                .map(|item| esc(item))
                .collect::<Vec<_>>()
                .join(", ");
            let affected = replan
                .affected_tickets
                .iter()
                .map(|item| esc(item))
                .collect::<Vec<_>>()
                .join(", ");
            let dependents = replan
                .dependent_tickets
                .iter()
                .map(|item| esc(item))
                .collect::<Vec<_>>()
                .join(", ");
            let invalidated = replan
                .invalidated_validation_reports
                .iter()
                .map(|item| esc(item))
                .collect::<Vec<_>>()
                .join(", ");
            let revalidations = replan
                .revalidations
                .iter()
                .map(|item| {
                    format!(
                        "{}: {} (input version {})",
                        esc(&item.ticket_id),
                        esc(&item.outcome),
                        item.input_version
                    )
                })
                .collect::<Vec<_>>()
                .join("<br>");
            let _ = write!(
                html,
                "<tr data-spec-replan=\"{}\"><td><code>{}</code></td><td>{}</td><td>Previous input version {} → {}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&replan.id),
                esc(&replan.id),
                badge(&replan.outcome),
                replan.previous_input_version,
                replan.input_version,
                if changed.is_empty() { "none" } else { &changed },
                if affected.is_empty() { "none" } else { &affected },
                if dependents.is_empty() { "none" } else { &dependents },
                if invalidated.is_empty() { "none" } else { &invalidated },
                if revalidations.is_empty() { "none" } else { &revalidations }
            );
        }
        html.push_str("</table>");
    }

    html.push_str("<h2>Active sessions</h2>");
    let active = run
        .scheduler
        .as_ref()
        .map(|s| s.active.clone())
        .unwrap_or_default();
    if active.is_empty() {
        html.push_str("<p class=\"muted\">No active sessions.</p>");
    } else {
        html.push_str("<ul>");
        for ticket in &active {
            let _ = write!(
                html,
                "<li data-active=\"{t}\">Ticket <code>{t}</code> holds an implementation slot</li>",
                t = esc(ticket)
            );
        }
        html.push_str("</ul>");
    }

    html.push_str(&tickets(run));
    html.push_str(&backlog_issues(run));
    html.push_str(&limits(run));
    html.push_str(&sessions(run));
    html.push_str(&reviews(run));
    html.push_str(&corrections(run));
    html.push_str(&integrations(run));
    html.push_str(&replans(run));
    html.push_str(&decisions(run));
    html.push_str(&validation(run));
    html.push_str(&recoveries(run));
    html.push_str(&delivery_groups(run));
    html.push_str(&delivery(run));
    html
}

fn backlog_issues(run: &Run) -> String {
    let Some(backlog) = &run.backlog else {
        return String::new();
    };
    let mut html = "<h2>Backlog issues</h2><table><tr><th>Issue</th><th>Disposition</th><th>Dependencies</th><th>Reason and criteria</th></tr>".to_owned();
    for disposition in &backlog.dispositions {
        let issue = backlog.issue_snapshot.iter().find(|issue| {
            format!("github:{}#{}", backlog.github_repository, issue.number) == disposition.issue
        });
        let label = issue
            .map(|issue| format!("{} — {}", disposition.issue, issue.title))
            .unwrap_or_else(|| disposition.issue.clone());
        let url = issue.map(|issue| issue.url.as_str()).filter(|url| {
            url.strip_prefix("https://github.com/")
                .is_some_and(|rest| !rest.is_empty())
        });
        let issue_label = url
            .map(|url| format!("<a href=\"{}\">{}</a>", esc(url), esc(&label)))
            .unwrap_or_else(|| esc(&label));
        let deps = if disposition.dependencies.is_empty() {
            "None".into()
        } else {
            esc(&disposition.dependencies.join(", "))
        };
        let criteria = if disposition.inferred_criteria.is_empty() {
            String::new()
        } else {
            format!(
                "<br>Criteria: {}",
                esc(&disposition.inferred_criteria.join("; "))
            )
        };
        let _ = write!(
            html,
            "<tr><td>{issue_label}</td><td>{} · {}</td><td>{deps}</td><td>{}{criteria}</td></tr>",
            badge(&disposition.kind),
            badge(&disposition.status),
            esc(&disposition.reason)
        );
    }
    html.push_str("</table>");
    if !backlog.evidence.is_empty() {
        html.push_str("<h3>Backlog evidence</h3><ul>");
        for evidence in &backlog.evidence {
            let _ = write!(html, "<li>{}</li>", esc(evidence));
        }
        html.push_str("</ul>");
    }
    html
}

fn validation(run: &Run) -> String {
    let mut html = "<h2>Global validation</h2>".to_owned();
    if run.validation_reports.is_empty() {
        html.push_str("<p class=\"muted\">The integrated result has not been validated.</p>");
        return html;
    }
    for (index, report) in run.validation_reports.iter().enumerate().rev() {
        let latest = index + 1 == run.validation_reports.len();
        let _ = write!(
            html,
            "<h3>{} report <code>{}</code> {}</h3><p>Input version {} · Integrated commit <code>{}</code></p>",
            if latest { "Latest" } else { "Earlier" },
            esc(&report.id),
            badge(&report.outcome),
            report.input_version,
            esc(report
                .integrated_commit
                .as_deref()
                .unwrap_or("none (nothing integrated)"))
        );
        if let Some(failure) = &report.failure {
            let _ = write!(html, "<p>Failure: {}</p>", esc(failure));
        }
        if !report.checks.is_empty() {
            let _ = write!(html, "<p>Checks:{}</p>", checks(&report.checks));
        }
        html.push_str("<table><tr><th>Criterion</th><th>Outcome</th><th>Evidence</th></tr>");
        for c in &report.criteria {
            let mut evidence = String::new();
            if c.evidence.is_empty() {
                evidence.push_str("No evidence was recorded; missing evidence is never a pass.");
            }
            for e in &c.evidence {
                let _ = write!(
                    evidence,
                    "<div>{} {} check{}: <code>{}</code> (exit {})",
                    badge(if e.check.passed { "passed" } else { "failed" }),
                    esc(&e.source),
                    if e.available {
                        ""
                    } else {
                        " (could not execute)"
                    },
                    esc(&e.check.command.join(" ")),
                    e.check.exit_code.map_or("none".into(), |c| c.to_string())
                );
                if !e.check.stderr.trim().is_empty() {
                    let _ = write!(evidence, "<pre>{}</pre>", esc(&e.check.stderr));
                }
                evidence.push_str("</div>");
            }
            let _ = write!(
                html,
                "<tr data-criterion=\"{id}\"><td><code>{id}</code> <span class=\"muted\">{} @ <code>{}</code></span><br>{}</td><td>{}</td><td>{evidence}</td></tr>",
                esc(&c.spec_path),
                esc(&c.content_sha256),
                esc(&c.criterion),
                badge(&c.outcome),
                id = esc(&c.id),
            );
        }
        html.push_str("</table>");
    }
    html
}

fn delivery(run: &Run) -> String {
    let mut html = "<h2>Publication and issue synchronization</h2>".to_owned();
    match &run.publication {
        None => html.push_str("<p class=\"muted\">The run has not been published.</p>"),
        Some(p) => {
            let _ = write!(
                html,
                "<p>Publication {} of <code>{}</code> to branch <code>{}</code> of <code>{}</code> (target <code>{}</code>).</p>",
                badge(&p.status),
                esc(&p.published_commit),
                esc(&p.branch),
                esc(&p.github_repository),
                esc(&p.target_branch)
            );
            if let Some(pr) = &p.pull_request {
                let _ = write!(
                    html,
                    "<p>Pull request #{}: <code>{}</code></p>",
                    pr.number,
                    esc(&pr.url)
                );
            }
        }
    }
    let issues = run
        .synchronization
        .as_ref()
        .map(|s| s.issues.as_slice())
        .unwrap_or_default();
    if !issues.is_empty() {
        html.push_str(
            "<table><tr><th>Ticket</th><th>Issue</th><th>Progress</th><th>Status</th></tr>",
        );
        for i in issues {
            let _ = write!(
                html,
                "<tr><td><code>{}</code></td><td><code>{}#{}</code></td><td>{}</td><td>{}{}</td></tr>",
                esc(&i.ticket_id),
                esc(&i.repository),
                i.number,
                esc(&i.state),
                badge(&i.status),
                if i.divergence.is_empty() { String::new() } else { format!("<br>Divergence: {}", esc(&i.divergence.join("; "))) }
            );
        }
        html.push_str("</table>");
    }
    html
}

fn delivery_groups(run: &Run) -> String {
    if run.delivery_groups.is_empty() {
        return String::new();
    }
    let mut html = "<h2>Delivery groups and CI</h2>".to_owned();
    for group in &run.delivery_groups {
        let _ = write!(
            html,
            "<section data-delivery-group=\"{}\"><h3><code>{}</code> {}</h3><p>Tickets: {}</p>",
            esc(&group.id),
            esc(&group.id),
            badge(&group.status),
            esc(&group.tickets.join(", "))
        );
        if !group.prerequisites.is_empty() {
            let _ = write!(
                html,
                "<p>Prerequisite groups: {}</p>",
                esc(&group.prerequisites.join(", "))
            );
        }
        if let Some(reason) = &group.reason {
            let _ = write!(html, "<p>Blocker or outcome: {}</p>", esc(reason));
        }
        if let Some(branch) = &group.branch {
            let _ = write!(html, "<p>Branch: <code>{}</code></p>", esc(branch));
        }
        if let Some(commit) = &group.commit {
            let _ = write!(html, "<p>Verified commit: <code>{}</code></p>", esc(commit));
        }
        if let Some(validation) = &group.validation {
            let _ = write!(
                html,
                "<p>Group validation {} for <code>{}</code>{}</p>",
                badge(&validation.outcome),
                esc(&validation.commit),
                validation
                    .failure
                    .as_ref()
                    .map(|failure| format!(" — {}", esc(failure)))
                    .unwrap_or_default()
            );
            if !validation.checks.is_empty() {
                html.push_str("<p>Build and test checks:");
                html.push_str(&checks(&validation.checks));
                html.push_str("</p>");
            }
            if !validation.criteria.is_empty() {
                html.push_str("<table><tr><th>Requirement</th><th>Criterion evidence</th><th>Result</th></tr>");
                for criterion in &validation.criteria {
                    let _ = write!(
                        html,
                        "<tr><td>{}</td><td>{}<br><code>{}</code></td><td>{}</td></tr>",
                        esc(&criterion.requirement),
                        esc(&criterion.criterion),
                        esc(&criterion.check.command.join(" ")),
                        badge(if criterion.check.passed {
                            "passed"
                        } else {
                            "failed"
                        })
                    );
                }
                html.push_str("</table>");
            }
        }
        for review in &group.reviews {
            let _ = write!(
                html,
                "<p>Delivery review for <code>{}</code>: {}</p>",
                esc(&review.commit),
                badge(if review.passed {
                    "approved"
                } else {
                    "rejected"
                })
            );
        }
        if let Some(pr) = &group.pull_request {
            let href = pr
                .url
                .strip_prefix("https://github.com/")
                .filter(|rest| !rest.is_empty());
            let title = if pr.draft { "Draft PR" } else { "Pull request" };
            if let Some(href) = href {
                let _ = write!(html, "<p>{title} #{}: <a href=\"https://github.com/{}\">{}</a> (head <code>{}</code>)</p>", pr.number, esc(href), esc(&pr.url), esc(&pr.head));
            } else {
                let _ = write!(
                    html,
                    "<p>{title} #{}: {} (head <code>{}</code>)</p>",
                    pr.number,
                    esc(&pr.url),
                    esc(&pr.head)
                );
            }
        }
        for attempt in &group.ci_attempts {
            let _ = write!(
                html,
                "<h4>CI attempt <code>{}</code> {} on commit <code>{}</code> for PR #{} </h4>",
                esc(&attempt.id),
                badge(&attempt.status),
                esc(&attempt.commit),
                attempt.pull_request
            );
            if let Some(failure) = &attempt.failure {
                let _ = write!(html, "<p>CI failure: {}</p>", esc(failure));
            }
            if !attempt.checks.is_empty() {
                html.push_str("<table><tr><th>Check</th><th>Status</th><th>Conclusion</th><th>Commit</th></tr>");
                for check in &attempt.checks {
                    let name = if let Some(url) = check
                        .url
                        .as_deref()
                        .filter(|url| url.starts_with("https://"))
                    {
                        format!("<a href=\"{}\">{}</a>", esc(url), esc(&check.name))
                    } else {
                        esc(&check.name)
                    };
                    let _ = write!(
                        html,
                        "<tr><td>{name}</td><td>{}</td><td>{}</td><td><code>{}</code></td></tr>",
                        badge(&check.status),
                        check
                            .conclusion
                            .as_deref()
                            .map(badge)
                            .unwrap_or_else(|| "—".into()),
                        esc(&check.commit)
                    );
                }
                html.push_str("</table>");
            }
            for observation in &attempt.observations {
                let checks = observation
                    .checks
                    .iter()
                    .map(|check| {
                        format!(
                            "{} {}",
                            esc(&check.name),
                            check
                                .conclusion
                                .as_deref()
                                .map(esc)
                                .unwrap_or_else(|| esc(&check.status))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = write!(
                    html,
                    "<p>CI poll {}: {} {}</p>",
                    observation.observed_unix_ms,
                    badge(&observation.status),
                    if checks.is_empty() {
                        "no check details"
                    } else {
                        &checks
                    }
                );
            }
        }
        html.push_str("</section>");
    }
    html
}

fn recoveries(run: &Run) -> String {
    let mut html = "<h2>Recovery</h2>".to_owned();
    if run.recoveries.is_empty() {
        html.push_str(
            "<p class=\"muted\">No recovery has been recorded; the run was never resumed.</p>",
        );
        return html;
    }
    for recovery in &run.recoveries {
        let _ = write!(
            html,
            "<h3>Resume <code>{}</code></h3><table><tr><th>Ticket</th><th>Subject</th><th>Decision</th><th>Reason</th></tr>",
            esc(&recovery.id)
        );
        for d in &recovery.decisions {
            let _ = write!(
                html,
                "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                esc(d.ticket_id.as_deref().unwrap_or("run")),
                esc(&d.subject),
                badge(&d.action),
                esc(&d.reason)
            );
        }
        html.push_str("</table>");
    }
    html
}

fn sessions(run: &Run) -> String {
    let mut html = "<h2>Implementation sessions</h2>".to_owned();
    if run.sessions.is_empty() {
        html.push_str("<p class=\"muted\">No implementation sessions have been recorded.</p>");
        return html;
    }
    html.push_str("<table><tr><th>Ticket</th><th>Session</th><th>Status</th><th>Ticket input version</th><th>Details</th></tr>");
    for s in &run.sessions {
        let mut details = format!("Branch <code>{}</code>", esc(&s.branch));
        if let Some(commit) = &s.commit {
            let _ = write!(details, "<br>Commit <code>{}</code>", esc(commit));
        }
        if let Some(outcome) = &s.agent_outcome {
            let _ = write!(details, "<br>Agent outcome: {}", esc(outcome));
        }
        if let Some(failure) = &s.failure {
            let _ = write!(details, "<br>Failure: {}", esc(failure));
        }
        details.push_str(&checks(&s.checks));
        let _ = write!(
            html,
            "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>Input version {}</td><td>{details}</td></tr>",
            esc(&s.ticket_id),
            esc(&s.id),
            badge(&s.status),
            s.input_version
        );
    }
    html.push_str("</table>");
    html
}

fn replans(run: &Run) -> String {
    let mut html = "<h2>Replanning</h2>".to_owned();
    if run.replans.is_empty() {
        html.push_str("<p class=\"muted\">No ticket has been replanned.</p>");
        return html;
    }
    html.push_str(
        "<table><tr><th>Ticket</th><th>Attempt</th><th>Outcome</th><th>Details</th></tr>",
    );
    for r in &run.replans {
        let mut details = String::from("Unresolved findings:<ul>");
        for f in &r.failures {
            let _ = write!(details, "<li>{}</li>", esc(f));
        }
        details.push_str("</ul>");
        let _ = write!(details, "Previous: {}", esc(&r.previous.description));
        if let Some(revised) = &r.revised {
            let _ = write!(details, "<br>Revised: {}", esc(&revised.description));
        }
        for f in &r.findings {
            let _ = write!(
                details,
                "<br>Finding <code>{}</code>: {}",
                esc(&f.code),
                esc(&f.message)
            );
        }
        if let Some(result) = &r.result {
            let _ = write!(details, "<br>Result: {}", badge(result));
        }
        let _ = write!(
            html,
            "<tr><td><code>{}</code></td><td>Attempt {} <code>{}</code></td><td>{}</td><td>{details}</td></tr>",
            esc(&r.ticket_id),
            r.attempt,
            esc(&r.id),
            badge(&r.outcome)
        );
    }
    html.push_str("</table>");
    html
}

fn decisions(run: &Run) -> String {
    let mut html = "<h2>Decisions</h2>".to_owned();
    if run.decisions.is_empty() {
        html.push_str("<p class=\"muted\">No autonomous decisions have been recorded.</p>");
        return html;
    }
    for d in &run.decisions {
        let _ = write!(
            html,
            "<h3>{} <code>{}</code></h3><p><strong>Question:</strong> {}</p><p><strong>Resolution:</strong> {}</p><p><strong>Rationale:</strong> {}</p>",
            badge(&d.outcome),
            esc(&d.id),
            esc(&d.question),
            esc(&d.resolution),
            esc(&d.rationale)
        );
        if let Some(g) = &d.governing {
            let _ = write!(
                html,
                "<p><strong>Governing position:</strong> <code>{}</code> ({}): {}</p>",
                esc(&g.reference),
                esc(&g.source),
                esc(&g.statement)
            );
        }
        html.push_str("<ol>");
        for p in &d.positions {
            let _ = write!(
                html,
                "<li><code>{}</code> ({}): {}</li>",
                esc(&p.reference),
                esc(&p.source),
                esc(&p.statement)
            );
        }
        html.push_str("</ol>");
        for e in &d.evidence {
            let _ = write!(html, "<p class=\"muted\">Evidence: {}</p>", esc(e));
        }
        for f in &d.findings {
            let _ = write!(
                html,
                "<p>Finding <code>{}</code>: {}</p>",
                esc(&f.code),
                esc(&f.message)
            );
        }
        if let Some(v) = d.spec_revision {
            let _ = write!(html, "<p>Produced spec revision {v}.</p>");
        }
    }
    html
}

fn limits(run: &Run) -> String {
    let mut html = "<h2>Limits and usage</h2>".to_owned();
    let Some(limits) = run.scheduler.as_ref().and_then(|s| s.limits.as_ref()) else {
        html.push_str(
            "<p class=\"muted\">No limits have been recorded; the run has not been scheduled.</p>",
        );
        return html;
    };
    let configured = &limits.configured;
    let or_none = |v: Option<String>| v.unwrap_or_else(|| "not configured".into());
    let usage = &limits.usage;
    let cost = match usage.measured_cost_usd {
        Some(cost) => format!("Cost: ${cost:.4} measured ({})", esc(&usage.cost_data)),
        None => match usage.estimated_cost_usd {
            Some(estimate) => format!(
                "Cost: unavailable as a measurement; providers estimated ${estimate:.4} ({})",
                esc(&usage.cost_data)
            ),
            None => "<span class=\"badge unknown state-unavailable\">Cost: unavailable</span> (no provider reported a cost; it is not zero)".to_owned(),
        },
    };
    let _ = write!(
        html,
        "<table><tr><th>Correction cycles per ticket</th><td>{}</td></tr><tr><th>Replanning attempts per ticket</th><td>{}</td></tr><tr><th>Duration limit</th><td>{}</td></tr><tr><th>Usage token limit</th><td>{}</td></tr><tr><th>Cost limit</th><td>{}</td></tr><tr><th>Limit policy</th><td>{}</td></tr><tr><th>Cost ceiling enforcement</th><td><code>{}</code></td></tr><tr><th>Tokens used</th><td>{} across {} provider contexts ({} without usage data)</td></tr><tr><th>Cost</th><td>{cost}</td></tr></table>",
        configured.correction_cycles,
        configured.replanning_attempts,
        or_none(configured.duration_limit_seconds.map(|s| format!("{s} seconds"))),
        or_none(configured.usage_token_limit.map(|t| t.to_string())),
        or_none(configured.cost_limit_usd.map(|c| format!("${c}"))),
        esc(&configured.limit_policy),
        esc(&limits.cost_ceiling),
        usage.tokens,
        usage.contexts,
        usage.contexts_without_usage,
    );
    match &limits.exhausted {
        Some(exhausted) => {
            let _ = write!(
                html,
                "<p class=\"notice\" data-stop-reason>Stop reason: {} limit exhausted — {}</p>",
                esc(&exhausted.limit),
                esc(&exhausted.reason)
            );
            if let Some(provider) = &exhausted.provider {
                let _ = write!(
                    html,
                    "<p class=\"notice\" data-provider-limit>Provider {} account usage limit reached; resets: {}. This is not a ticket failure: completed work is preserved and <code>kiln resume {}</code> retries the interrupted step.</p>",
                    esc(provider),
                    esc(exhausted.reset_at.as_deref().unwrap_or("not reported")),
                    esc(&run.id)
                );
            }
        }
        None => html.push_str("<p>No run-wide limit was exhausted.</p>"),
    }
    html
}

fn findings(axis: &crate::review::AxisReview) -> String {
    let mut html = format!(
        "<p>{} {}</p>",
        badge(&axis.result.outcome),
        esc(&axis.result.evidence)
    );
    if let Some(failure) = &axis.failure {
        let _ = write!(html, "<p>Failure: {}</p>", esc(failure));
    }
    if !axis.result.findings.is_empty() {
        html.push_str("<ul>");
        for f in &axis.result.findings {
            let _ = write!(
                html,
                "<li><code>{}</code>{} {} <span class=\"muted\">Evidence: {}</span></li>",
                esc(&f.code),
                if f.required {
                    " (required)"
                } else {
                    " (optional)"
                },
                esc(&f.message),
                esc(&f.evidence)
            );
        }
        html.push_str("</ul>");
    }
    html
}

fn reviews(run: &Run) -> String {
    let mut html = "<h2>Reviews</h2>".to_owned();
    if run.reviews.is_empty() {
        html.push_str("<p class=\"muted\">No reviews have been recorded.</p>");
        return html;
    }
    html.push_str("<table><tr><th>Ticket</th><th>Review</th><th>Standards review</th><th>Spec review</th></tr>");
    for review in &run.reviews {
        let _ = write!(
            html,
            "<tr><td><code>{}</code></td><td>{} <code>{}</code><br><span class=\"muted\">Commit <code>{}</code></span></td><td>{}</td><td>{}</td></tr>",
            esc(&review.ticket_id),
            badge(if review.passed { "approved" } else { "rejected" }),
            esc(&review.id),
            esc(&review.commit),
            findings(&review.standards),
            findings(&review.spec)
        );
    }
    html.push_str("</table>");
    html
}

fn corrections(run: &Run) -> String {
    let mut html = "<h2>Correction history</h2>".to_owned();
    if run.corrections.is_empty() {
        html.push_str("<p class=\"muted\">No corrections have been recorded.</p>");
        return html;
    }
    html.push_str("<table><tr><th>Ticket</th><th>Correction</th><th>Outcome</th><th>Findings addressed</th></tr>");
    for c in &run.corrections {
        let mut addressed = String::from("<ul>");
        for f in &c.findings {
            let _ = write!(addressed, "<li>{}</li>", esc(f));
        }
        addressed.push_str("</ul>");
        if let Some(failure) = &c.failure {
            let _ = write!(addressed, "<p>Failure: {}</p>", esc(failure));
        }
        let _ = write!(
            html,
            "<tr><td><code>{}</code></td><td><code>{}</code><br><span class=\"muted\">Session <code>{}</code> → <code>{}</code></span></td><td>{}</td><td>{addressed}</td></tr>",
            esc(&c.ticket_id),
            esc(&c.id),
            esc(&c.before.id),
            esc(&c.after.id),
            badge(&c.outcome),
        );
    }
    html.push_str("</table>");
    html
}

fn integrations(run: &Run) -> String {
    let mut html = "<h2>Integration outcomes</h2>".to_owned();
    if run.integrations.is_empty() {
        html.push_str("<p class=\"muted\">No integrations have been attempted.</p>");
        return html;
    }
    html.push_str(
        "<table><tr><th>Ticket</th><th>Attempt</th><th>Outcome</th><th>Details</th></tr>",
    );
    for i in &run.integrations {
        let mut details = format!("Reviewed <code>{}</code>", esc(&i.reviewed_commit));
        if let Some(commit) = &i.integrated_commit {
            let _ = write!(details, "<br>Integrated <code>{}</code>", esc(commit));
        }
        if !i.conflicts.is_empty() {
            let _ = write!(details, "<br>Conflicts: {}", esc(&i.conflicts.join(", ")));
        }
        if let Some(failure) = &i.failure {
            let _ = write!(details, "<br>Failure: {}", esc(failure));
        }
        details.push_str(&checks(&i.checks));
        let _ = write!(
            html,
            "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{details}</td></tr>",
            esc(&i.ticket_id),
            esc(&i.id),
            badge(&i.status),
        );
    }
    html.push_str("</table>");
    html
}

fn checks(checks: &[crate::execution::CheckResult]) -> String {
    let mut html = String::new();
    for c in checks {
        let _ = write!(
            html,
            "<br>{} {}: <code>{}</code>",
            badge(if c.passed { "passed" } else { "failed" }),
            esc(&c.name),
            esc(&c.command.join(" "))
        );
    }
    html
}

fn tickets(run: &Run) -> String {
    let mut html = "<h2>Tickets</h2>".to_owned();
    let planned = run
        .plan
        .as_ref()
        .map(|p| p.tickets.as_slice())
        .unwrap_or_default();
    let scheduled = run
        .scheduler
        .as_ref()
        .map(|s| s.tickets.as_slice())
        .unwrap_or_default();
    if planned.is_empty() && scheduled.is_empty() {
        html.push_str("<p class=\"muted\">No tickets have been planned.</p>");
        return html;
    }
    html.push_str("<table><tr><th>Ticket</th><th>Title</th><th>State</th><th>Dependencies</th><th>Why it cannot proceed</th></tr>");
    let mut ids: Vec<&str> = planned.iter().map(|t| t.id.as_str()).collect();
    for t in scheduled {
        if !ids.contains(&t.id.as_str()) {
            ids.push(&t.id);
        }
    }
    for id in ids {
        let plan = planned.iter().find(|t| t.id == id);
        let schedule = scheduled.iter().find(|t| t.id == id);
        let blocked_by = schedule
            .map(|s| s.blocked_by.clone())
            .or_else(|| plan.map(|p| p.blocked_by.clone()))
            .unwrap_or_default();
        let dependencies = if blocked_by.is_empty() {
            "None".to_owned()
        } else {
            format!("Depends on: {}", esc(&blocked_by.join(", ")))
        };
        let mut reasons = Vec::new();
        if let Some(s) = schedule {
            if !s.waiting_on.is_empty() {
                reasons.push(format!("Waiting on: {}", esc(&s.waiting_on.join(", "))));
            }
            if let Some(blocker) = &s.blocker {
                reasons.push(format!("Blocker: {}", esc(blocker)));
            }
            if let Some(exhaustion) = &s.exhaustion {
                reasons.push(format!("Exhausted: {}", esc(exhaustion)));
            }
            if s.state == "stopped" {
                reasons.push("Stopped by an exhausted run-wide limit; resumable".to_owned());
            }
        }
        let state = schedule.map_or("not scheduled", |s| s.state.as_str());
        let _ = write!(
            html,
            "<tr data-ticket=\"{id}\"><td><code>{id}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(plan.map_or("", |p| p.title.as_str())),
            badge(state),
            dependencies,
            if reasons.is_empty() {
                "—".to_owned()
            } else {
                reasons.join("<br>")
            },
            id = esc(id),
        );
    }
    html.push_str("</table>");
    html
}

/// Visual class of a recorded state or outcome; the text always names it too.
fn badge(value: &str) -> String {
    let class = match value {
        "verified" | "integrated" | "approved" | "passed" | "published" | "synchronized" => "good",
        "failed" | "blocked" | "rejected" | "conflicted" | "exhausted" | "no-progress"
        | "limit_exhausted" | "conflict" => "bad",
        "unable-to-verify" | "interrupted" | "unavailable" | "stopped" | "provider_limit" => {
            "unknown"
        }
        _ => "",
    };
    format!(
        "<span class=\"badge {class} state-{}\">{}</span>",
        esc(value),
        esc(&label(value))
    )
}
fn label(value: &str) -> String {
    let text = value.replace(['_', '-'], " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
fn esc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
