//! Local, read-only monitoring interface. Every page is rendered from the durable
//! run state that `kiln inspect` exposes, re-read on each request; the interface
//! holds no workflow truth of its own and exposes no execution control.
use crate::{Engine, Run};
use anyhow::{bail, Result};
use std::{fmt::Write as _, io::Write, net::SocketAddr};
use tiny_http::{Header, Method, Response, Server};

/// Seconds between automatic reloads of a run that is still recorded as running.
const REFRESH_SECONDS: u32 = 3;

pub fn serve(engine: Engine, bind: SocketAddr) -> Result<()> {
    if !bind.ip().is_loopback() {
        bail!("the local web view must bind to a loopback address");
    }
    let server =
        Server::http(bind).map_err(|e| anyhow::anyhow!("cannot start local web view: {e}"))?;
    let bound = server.server_addr().to_ip().unwrap_or(bind);
    println!("Kiln web view: http://{}", server.server_addr());
    std::io::stdout().flush()?;
    for request in server.incoming_requests() {
        let expected_hosts = [
            format!("localhost:{}", bound.port()),
            format!("127.0.0.1:{}", bound.port()),
            format!("[::1]:{}", bound.port()),
        ];
        let valid_host = request.headers().iter().any(|header| {
            header.field.equiv("Host")
                && expected_hosts
                    .iter()
                    .any(|host| header.value.as_str().eq_ignore_ascii_case(host))
        });
        let (status, content_type, body) = if !valid_host {
            (
                400,
                "text/plain; charset=utf-8",
                "Invalid Host header".to_owned(),
            )
        } else if request.method() != &Method::Get {
            (
                405,
                "text/plain; charset=utf-8",
                "Only GET is supported".to_owned(),
            )
        } else {
            let path = request.url().split('?').next().unwrap_or("/");
            match render(&engine, path) {
                Ok((kind, body)) => (200, kind, body),
                Err(error) => (404, "text/plain; charset=utf-8", format!("Kiln: {error:#}")),
            }
        };
        let response = Response::from_string(body)
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", content_type).unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
            .with_header(Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap())
            .with_header(
                Header::from_bytes(
                    "Content-Security-Policy",
                    "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
                )
                .unwrap(),
            );
        let _ = request.respond(response);
    }
    Ok(())
}
fn render(engine: &Engine, path: &str) -> Result<(&'static str, String)> {
    if let Some(id) = path.strip_prefix("/api/runs/") {
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
    let (title, refresh, content) = if path == "/" {
        let mut content = "<h1>Kiln workflow runs</h1><ul>".to_owned();
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
        ("Kiln workflow runs".to_owned(), false, content)
    } else if let Some(id) = path.strip_prefix("/runs/") {
        let run = engine.inspect(id)?;
        let live = liveness(engine, &run.id);
        // Recorded state is already redacted; redact again in case a secret was
        // registered after the state was written.
        let content = redact_for_web(&run, &run_page(&run, live));
        (
            format!("Kiln run {}", run.id),
            run.status == "running",
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

fn run_page(run: &Run, live: Option<bool>) -> String {
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
    html.push_str(&limits(run));
    html.push_str(&sessions(run));
    html.push_str(&reviews(run));
    html.push_str(&corrections(run));
    html.push_str(&integrations(run));
    html.push_str(&replans(run));
    html.push_str(&decisions(run));
    html.push_str(&validation(run));
    html.push_str(&recoveries(run));
    html.push_str(&delivery(run));
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
