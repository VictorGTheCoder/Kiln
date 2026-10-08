// Kiln dashboard: runs list and the selected run's ticket kanban, rendered
// from /api/dashboard/runs and /api/dashboard/runs/<id>. Text is always set
// with textContent, never parsed as HTML.
"use strict";

const STAGE_LABELS = {
  planned: "Planned",
  implementing: "Implementing",
  review: "Review",
  integrating: "Integrating",
  pr: "PR",
  ci: "CI",
};
const POLL_MS = 3000;
let selected = decodeURIComponent((location.hash.match(/^#run=(.+)$/) || [])[1] || "");

function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (key === "class") node.className = value;
    else node.setAttribute(key, value);
  }
  for (const child of children) {
    if (child == null) continue;
    node.append(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return node;
}

function statusClass(status) {
  if (["completed", "verified", "integrated", "published", "passed"].includes(status)) return "good";
  if (["failed", "blocked", "ci-failed", "cancelled", "limit_exhausted"].includes(status)) return "bad";
  if (["paused", "partial", "unable-to-verify", "stopped"].includes(status)) return "warn";
  return "";
}
function status(value) {
  return el("span", { class: "status " + statusClass(value) }, value.replace(/[_-]/g, " "));
}

async function getJson(path) {
  const response = await fetch(path, { headers: { Accept: "application/json" } });
  if (!response.ok) throw new Error(path + ": " + response.status);
  return response.json();
}

function renderRuns(runs) {
  const list = document.getElementById("run-list");
  list.replaceChildren();
  if (runs.length === 0) {
    list.append(el("li", { class: "empty" }, "No runs yet. Run kiln plan or kiln start."));
    return;
  }
  if (!selected) selected = runs[0].id;
  for (const run of runs) {
    const button = el(
      "button",
      { type: "button", "aria-current": String(run.id === selected), "data-run": run.id },
      el("div", { class: "id" }, run.id),
      el("div", null, status(run.status), " ",
        el("span", { class: "muted" }, new Date(Number(run.created_unix_ms)).toLocaleString() +
          " · " + run.tickets + " tickets")),
    );
    button.addEventListener("click", () => {
      selected = run.id;
      location.hash = "run=" + encodeURIComponent(run.id);
      refresh();
    });
    list.append(el("li", null, button));
  }
}

function renderCard(ticket) {
  const blocked = ticket.state === "blocked" || ticket.state === "stopped";
  const card = el("div", { class: "card" + (blocked ? " blocked" : ""), "data-ticket": ticket.id },
    el("div", { class: "title" }, ticket.title || ticket.id),
    el("div", { class: "meta id" }, ticket.id),
    el("div", { class: "meta" }, status(ticket.state)));
  if (ticket.blocked_by.length) card.append(el("div", { class: "meta" }, "After: " + ticket.blocked_by.join(", ")));
  if (ticket.blocker) card.append(el("div", { class: "meta" }, "Blocker: " + ticket.blocker));
  if (ticket.pull_request) {
    const link = el("a", { href: ticket.pull_request.url, rel: "noreferrer", target: "_blank" },
      "PR #" + ticket.pull_request.number);
    card.append(el("div", { class: "meta" }, link));
  }
  if (ticket.ci) card.append(el("div", { class: "meta" }, "CI: ", status(ticket.ci)));
  return card;
}

function renderBoard(board) {
  const section = document.getElementById("board");
  const repo = board.github_repository ? " · " + board.github_repository : "";
  const columns = el("div", { class: "columns" });
  for (const column of board.stages) {
    const box = el("div", { class: "column", "data-stage": column.stage },
      el("h3", null, STAGE_LABELS[column.stage] || column.stage, el("span", null, String(column.tickets.length))));
    for (const ticket of column.tickets) box.append(renderCard(ticket));
    columns.append(box);
  }
  section.replaceChildren(
    el("h2", null, "Run"),
    el("p", { id: "title" }, el("span", { class: "id" }, board.id), repo, " ", status(board.status)),
    columns,
  );
}

async function refresh() {
  try {
    const runs = await getJson("/api/dashboard/runs");
    renderRuns(runs);
    if (selected) renderBoard(await getJson("/api/dashboard/runs/" + encodeURIComponent(selected)));
    document.getElementById("updated").textContent = "Updated " + new Date().toLocaleTimeString();
  } catch (error) {
    document.getElementById("updated").textContent = "Cannot reach Kiln: " + error.message;
  }
}

refresh();
setInterval(refresh, POLL_MS);
