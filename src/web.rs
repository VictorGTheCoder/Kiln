use crate::Engine;
use anyhow::{bail, Result};
use std::{io::Write, net::SocketAddr};
use tiny_http::{Header, Method, Response, Server};

/// Presentation reads durable engine state for every request, including state
/// written after the server starts. No execution control is exposed here.
pub fn serve(engine: Engine, bind: SocketAddr) -> Result<()> {
    if !bind.ip().is_loopback() {
        bail!("the local web view must bind to a loopback address");
    }
    let server =
        Server::http(bind).map_err(|e| anyhow::anyhow!("cannot start local web view: {e}"))?;
    println!("Kiln web view: http://{}", server.server_addr());
    std::io::stdout().flush()?;
    for request in server.incoming_requests() {
        let (status, content_type, body) = if request.method() != &Method::Get {
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
        return Ok((
            "application/json",
            serde_json::to_string_pretty(&engine.inspect(id)?)?,
        ));
    }
    if path == "/api/runs" {
        return Ok(("application/json", serde_json::to_string(&engine.list()?)?));
    }
    let content = if path == "/" {
        let mut content = "<h1>Kiln workflow runs</h1><ul>".to_owned();
        for id in engine.list()? {
            content.push_str(&format!("<li><a href=\"/runs/{id}\">{id}</a></li>"));
        }
        content.push_str("</ul>");
        content
    } else if let Some(id) = path.strip_prefix("/runs/") {
        format!(
            "<h1>Workflow run</h1><a href=\"/\">All runs</a><pre>{}</pre>",
            escape(&serde_json::to_string_pretty(&engine.inspect(id)?)?)
        )
    } else {
        bail!("page was not found");
    };
    Ok(("text/html; charset=utf-8", format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>Kiln workflow runs</title><style>body{{font:16px system-ui;max-width:960px;margin:40px auto;padding:0 24px;color:#202020}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#f4f4f4;padding:24px}}a{{color:#174a8b}}</style><body>{content}</body></html>")))
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
