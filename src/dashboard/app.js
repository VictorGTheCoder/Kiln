// Kiln dashboard: runs list and the selected run's ticket kanban, rendered
// from /api/dashboard/runs and /api/dashboard/runs/<id>. The selected run's
// journal is followed live over /api/dashboard/runs/<id>/events (Server-Sent
// Events); each event refreshes the kanban without reloading the page. Text is
// always set with textContent, never parsed as HTML.
"use strict";

const STAGE_LABELS = {
  planned: "Planned",
  implementing: "Implementing",
  review: "Review",
  integrating: "Integrating",
  pr: "PR",
  ci: "CI",
};
const ATTENTION_LABELS = { failure: "Failure", divergence: "Divergence", decision: "Decision" };
const POLL_MS = 3000;
// A stream that ended is reopened at most this often while its run is still
// recorded as running (e.g. a run resumed by another process).
const REOPEN_MS = 10000;
const MAX_ACTIVITY = 300;
let selected = decodeURIComponent((location.hash.match(/^#run=(.+)$/) || [])[1] || "");
let selectedStatus = "";
// The selected run's event stream: { id, source, open, endedAt }.
let stream = null;
let boardTimer = null;

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
  if (["failed", "blocked", "ci-failed", "cancelled", "limit_exhausted", "rejected", "delivery-blocked"].includes(status)) return "bad";
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

function select(id) {
  selected = id;
  location.hash = "run=" + encodeURIComponent(id);
  refresh();
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
    if (run.id === selected) selectedStatus = run.status;
    const button = el(
      "button",
      { type: "button", "aria-current": String(run.id === selected), "data-run": run.id },
      el("div", { class: "id" }, run.id),
      el("div", null, status(run.status), " ",
        el("span", { class: "muted" }, new Date(Number(run.created_unix_ms)).toLocaleString() +
          " · " + run.tickets + " tickets")),
    );
    button.addEventListener("click", () => select(run.id));
    list.append(el("li", null, button));
  }
}

function renderCard(ticket, flags) {
  const blocked = ticket.state === "blocked" || ticket.state === "stopped";
  const failing = flags.some((f) => f.kind === "failure");
  const classes = "card" + (blocked || failing ? " blocked" : "") + (flags.length ? " flagged" : "");
  const card = el("div", { class: classes, "data-ticket": ticket.id },
    el("div", { class: "title" }, ticket.title || ticket.id),
    el("div", { class: "meta id" }, ticket.id),
    el("div", { class: "meta" }, status(ticket.state)));
  for (const flag of flags) {
    card.append(el("div", { class: "flag", title: flag.message }, "⚠ " + (ATTENTION_LABELS[flag.kind] || flag.kind)));
  }
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

function renderAttention(items) {
  if (!items.length) return null;
  const list = el("ul", { id: "attention", "aria-label": "Needs attention" });
  for (const item of items) {
    list.append(el("li", { class: item.kind, "data-kind": item.kind },
      el("span", { class: "kind" }, ATTENTION_LABELS[item.kind] || item.kind),
      item.ticket ? el("span", { class: "id" }, item.ticket + ": ") : null,
      item.message));
  }
  return el("div", null, el("h2", null, "Needs attention (" + items.length + ")"), list);
}

function renderBoard(board) {
  const section = document.getElementById("board");
  const repo = board.github_repository ? " · " + board.github_repository : "";
  const attention = board.attention || [];
  const columns = el("div", { class: "columns" });
  for (const column of board.stages) {
    const box = el("div", { class: "column", "data-stage": column.stage },
      el("h3", null, STAGE_LABELS[column.stage] || column.stage, el("span", null, String(column.tickets.length))));
    for (const ticket of column.tickets) {
      box.append(renderCard(ticket, attention.filter((a) => a.ticket === ticket.id)));
    }
    columns.append(box);
  }
  section.replaceChildren(
    el("h2", null, "Run"),
    el("p", { id: "title" }, el("span", { class: "id" }, board.id), repo, " ", status(board.status)),
    renderAttention(attention),
    columns,
  );
}

async function refreshBoard() {
  if (!selected) return;
  try {
    renderBoard(await getJson("/api/dashboard/runs/" + encodeURIComponent(selected)));
  } catch (error) {
    document.getElementById("updated").textContent = "Cannot reach Kiln: " + error.message;
  }
}
// Coalesce bursts of events into one board request. Run transitions also
// change which actions apply, so the action buttons follow the stream too.
function scheduleBoard() {
  clearTimeout(boardTimer);
  boardTimer = setTimeout(() => {
    refreshBoard();
    refreshActions();
  }, 150);
}

function addActivity(event) {
  const list = document.getElementById("activity");
  const clock = (event.ts || "").slice(11, 19);
  const text = [clock, event.ticket, event.stage, event.status].filter(Boolean).join("  ") +
    (event.message ? ": " + event.message : "");
  list.prepend(el("li", { class: statusClass(event.status) }, text));
  while (list.children.length > MAX_ACTIVITY) list.lastChild.remove();
}
function setLive(text) {
  document.getElementById("live").textContent = text;
}

// Follow the selected run's event stream. The server first replays the
// journal, then sends live events, then `end` when no process writes it.
function follow() {
  if (!selected || typeof EventSource === "undefined") return;
  if (stream && stream.id === selected) {
    const reopen = !stream.open && selectedStatus === "running" && Date.now() - stream.endedAt > REOPEN_MS;
    if (!reopen) return;
  }
  if (stream && stream.source) stream.source.close();
  document.getElementById("activity").replaceChildren();
  const source = new EventSource("/api/dashboard/runs/" + encodeURIComponent(selected) + "/events");
  const current = { id: selected, source, open: true, endedAt: 0 };
  stream = current;
  setLive("· connecting");
  source.addEventListener("open", () => { if (stream === current) setLive("· live"); });
  source.addEventListener("journal", (message) => {
    if (stream !== current) return;
    try {
      addActivity(JSON.parse(message.data));
    } catch (_) {
      return;
    }
    scheduleBoard();
  });
  source.addEventListener("end", () => {
    source.close();
    if (stream !== current) return;
    current.open = false;
    current.endedAt = Date.now();
    setLive("· not live");
    scheduleBoard();
  });
  // On a network error EventSource reconnects by itself with Last-Event-ID.
  source.addEventListener("error", () => { if (stream === current && current.open) setLive("· reconnecting"); });
}

async function refresh() {
  try {
    const runs = await getJson("/api/dashboard/runs");
    renderRuns(runs);
    // A live stream refreshes the board on every event; poll it otherwise.
    if (!(stream && stream.id === selected && stream.open)) await refreshBoard();
    follow();
    document.getElementById("updated").textContent = "Updated " + new Date().toLocaleTimeString();
  } catch (error) {
    document.getElementById("updated").textContent = "Cannot reach Kiln: " + error.message;
  }
}

// Action buttons. Plan and Start act on the repository; Pause, Resume and
// Cancel act on the selected run. The server says which actions make sense
// now; a POST carries no body, so the server's same-origin checks apply.
const RUN_ACTIONS = ["pause", "resume", "cancel"];
let actionPending = false;

function showAction(text, bad) {
  const node = document.getElementById("action-status");
  node.textContent = text;
  node.className = bad ? "bad" : "";
}

function describeLast(last) {
  if (!last) return null;
  const target = last.run ? " of run " + last.run : "";
  if (last.state === "running") return [last.action + target + " is running…", false];
  if (last.state === "failed") return [last.action + target + " failed: " + (last.error || "unknown error"), true];
  return [last.action + target + " finished.", false];
}

async function refreshActions() {
  let offered;
  try {
    offered = await getJson("/api/dashboard/actions");
  } catch (error) {
    return;
  }
  const runActions = (selected && offered.runs[selected]) || [];
  for (const button of document.querySelectorAll("#actions button[data-action]")) {
    const action = button.dataset.action;
    const allowed = RUN_ACTIONS.includes(action) ? runActions : offered.available;
    button.disabled = actionPending || !allowed.includes(action);
  }
  const last = describeLast(offered.last);
  if (last && !actionPending) showAction(last[0], last[1]);
}

async function act(action) {
  const path = RUN_ACTIONS.includes(action)
    ? "/api/dashboard/runs/" + encodeURIComponent(selected) + "/" + action
    : "/api/dashboard/actions/" + action;
  actionPending = true;
  showAction(action + " requested…", false);
  try {
    const response = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "application/json" },
    });
    const body = await response.json().catch(() => ({}));
    if (response.ok) {
      showAction(action + (body.state === "requested" ? " requested." : " started."), false);
      if (action === "plan" || action === "start") selected = "";
    } else {
      showAction(action + " failed: " + (body.error || response.status), true);
    }
  } catch (error) {
    showAction("Cannot reach Kiln: " + error.message, true);
  } finally {
    actionPending = false;
    refresh();
    refreshActions();
  }
}

for (const button of document.querySelectorAll("#actions button[data-action]")) {
  button.addEventListener("click", () => act(button.dataset.action));
}

refresh();
refreshActions();
setInterval(refresh, POLL_MS);
setInterval(refreshActions, POLL_MS);
// Selecting another run changes which run actions apply.
window.addEventListener("hashchange", refreshActions);
