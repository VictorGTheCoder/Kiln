# Preparing a workflow run

Kiln is a separate Rust project. Install Rust and Git, then run `cargo build`.
Preparation validates readiness and records inputs; it does not launch agents or
execute build, test, or startup commands.

Create `kiln.json` in the target Git repository:

```json
{
  "build": ["cargo", "build"],
  "test": ["cargo", "test"],
  "startup": ["cargo", "run"],
  "acceptance_criteria": ["The combined application fulfills both approved specs"],
  "isolation": {
    "network": "none",
    "runtime": "system",
    "commands": [["cargo", "build"], ["cargo", "test"], ["cargo", "run"]],
    "secrets": {}
  }
}
```

Commands are argument arrays, with the executable first. Relative executable
paths are resolved against the repository root. Preparation checks that each
executable exists and can execute, not that running the command would pass.
Acceptance criteria must contain at least one nonempty entry. Unknown JSON
fields are preserved for future project policy configuration; they do not grant
execution permissions in this preparation-only version.

```sh
kiln --repo /path/to/project prepare --config kiln.json --spec docs/first.md --spec docs/second.md
kiln --repo /path/to/project inspect
kiln --repo /path/to/project inspect run-REPLACE-WITH-RETURNED-ID
kiln --repo /path/to/project serve --bind 127.0.0.1:3000
```

`prepare` outputs the complete run as JSON, including its identity. `inspect`
without an identity lists existing identities. Invalid input exits unsuccessfully
with an actionable English error, without creating a prepared run. Spec paths
must identify nonempty UTF-8 Markdown files inside the repository; duplicate
paths are rejected. Providing a spec signifies it was already approved.

The web root lists runs with their status; `/runs/<id>` is a read-only monitoring
page rendered from the recorded run: frozen specs and verified spec revisions,
ticket dependencies with why waiting or blocked work cannot start, active
sessions, limits with usage, cost (shown as unavailable, never zero, when no
provider reported it) and stop reasons, implementation sessions, Standards and
Spec review findings, correction history, integration outcomes, replanning and
decisions, global validation evidence per acceptance criterion, recovery
decisions, publication and issue synchronization. Verified, failed and
unable-to-verify outcomes carry distinct colors, symbols and text. A run
recorded as running reloads every few seconds; when no scheduler process holds
its ownership lock it is shown as interrupted with the resume command. All
recorded text is HTML-escaped and configured secret values are redacted. The page
shows the current input version, each session and validation report's input
version, and the history of approved spec replans with affected work and
invalidated validation reports.
`/api/runs` and `/api/runs/<id>` return the same JSON as CLI inspection. The
server accepts loopback addresses only and reads state for every request.

# Durable engine interface

`Engine::open`, `prepare`, `inspect`, `list`, and `save` in `src/engine.rs` are
shared by the CLI and web presentation. `ProjectConfig` is in `src/config.rs`.
`Run` and `FrozenSpec` in `src/state.rs` define version 1 of the serialized
contract. Later workflow slices extend these types and engine operations.

State lives under the target repository's `.kiln/runs/<id>.json`. Writes stage,
flush, atomically rename, and flush the containing directory. Exclude `.kiln/`
from version control in target projects. The run retains its repository path,
configuration, preparation status, timestamp, and each spec's exact contents,
repository-relative path, SHA-256 identity, and observed HEAD revision (null for
repositories without a commit). The content identity covers uncommitted inputs;
HEAD alone does not assert that a spec matches its committed version. External
edits never mutate an existing run's frozen inputs.

# Generating a verified plan

Approved frozen specs must contain an `Acceptance criteria` Markdown heading
with nonempty bullet or numbered criteria. Kiln derives IDs such as
`docs/first.md#ac-1` in source order and records their frozen spec SHA-256.
Missing explicit criteria blocks planning; agent-supplied coverage is never the
source of requirements. External file edits do not affect planning inputs.

```sh
kiln --repo /path/to/project plan RUN-ID --fixture planning-agent.json
kiln --repo /path/to/project inspect RUN-ID
```

The deterministic adapter supports account-free engine testing. It is explicitly
simulated, not evidence of real model quality. Its JSON protocol is:

```json
{
  "tickets": [{
    "id": "feature-a",
    "title": "Deliver feature A",
    "description": "Complete observable behavior",
    "acceptance_criteria": ["The feature is demonstrably usable"],
    "covers": ["docs/first.md#ac-1"],
    "blocked_by": []
  }],
  "verification": {"outcome": "verified", "findings": []}
}
```

Verification outcomes are `verified`, `failed`, and `unable-to-verify`.
Findings contain `code` and `message`; any finding or nonverified outcome blocks
execution. The engine independently rejects missing coverage, unknown criterion
IDs, unknown blockers, dependency cycles, duplicate identities, and tickets
without observable acceptance criteria. A favorable fixture opinion does not
bypass these checks. Semantic decomposition quality remains the responsibility
of the separate verifier adapter; structural checks alone do not prove it.

`PlanningAgent` in `src/planning.rs` is the provider boundary. Generation receives
only frozen specs, requirements, and adapted to-tickets instructions. Verification
receives a separate context identity, frozen specs, requirements, generated
tickets, and independent verifier instructions. Provider implementations must
launch fresh sessions for these distinct requests. The fixture supplies distinct
simulated role responses; future real providers implement the same boundary.
`validate_plan` is reusable for imported ticket plans.

The durable `Run.plan` contains requirements, tickets, dependency graph,
verification outcome, findings, context identities, and executable status.
Accepted plans set `status` to `planned`; rejected plans set `plan_rejected`,
persist evidence, and exit unsuccessfully. Rejected plans may be regenerated.
Existing CLI inspection and web run/API pages show the same persisted plan.

# Importing GitHub issues

Import selected existing issues into a prepared run with approved frozen specs:

```sh
kiln --repo /path/to/project import RUN-ID --github-repo owner/project --issue 7 --issue 8 --verification-fixture verifier.json
```

The real issue source uses authenticated `gh api` GET requests for issue details
and paginated native `/dependencies/blocked_by` relationships. It never creates,
closes, labels, or otherwise mutates issues. Failed reads are reported rather than
silently dropping native dependencies. A `## Blocked by` body section also accepts
`- #7: description` and issue URLs; native and body relationships are deduplicated.
Missing selected prerequisites and cycles block execution, even if the blocker is
closed externally; include prerequisites explicitly or reconcile the backlog.

Issue bodies must carry `## Acceptance criteria` bullets and `## Spec coverage`
bullets with exact frozen criterion IDs, such as `- docs/first.md#ac-1`.
Imported acceptance criteria must retain associated approved criterion text.
Additional semantic differences are checked by a distinct verifier context.
Unknown associations, missing coverage, divergence and failed verification remain
inspectable findings; imported text never rewrites frozen repository specs.
Repository specs are always the source of requirements.

`--fixture issues.json` substitutes deterministic issue snapshots with an
`issues` array. Each item contains `number`, `url`, `title`, `body`, `labels`
(string array), and `blocked_by` (canonical `github:owner/project#number` strings).
The verifier fixture follows the planning fixture protocol; its generation
`tickets` field is ignored, and only the separate verification response is used.
A fixture verdict is simulated evidence, not real external review or approval.
The reusable `Engine::import_issues` accepts an `IssueSource` and independent
`PlanningAgent` verifier for real providers.

The imported snapshot remains in `Run.imported_issues`; plan ticket identities
are canonical GitHub identities and retain criterion associations and dependency
edges. Repeating import replaces the selected snapshot and plan without duplicates;
select the entire intended backlog each time. Imports are allowed before execution
and never launch implementation, Git worktrees or project commands.
Rejected imports exit unsuccessfully after recording their evidence.
Live read-only GitHub contract responses are documented in
`tests/fixtures/github/README.md` and replayed through the CLI provider boundary.

# Implementing one ticket

```sh
kiln --repo /path/to/project implement RUN-ID TICKET-ID --fixture implementation-agent.json
```

The deterministic fixture makes actual file changes and returns a simulated result:

```json
{"files":{"src/feature.txt":"Implemented content\n"},"outcome":"completed","log":"Session evidence"}
```

`ImplementationAgent` in `src/execution.rs` is the provider boundary. Every request
has a fresh context identity, relevant frozen specs, ticket, repository AGENTS.md,
and integrated prerequisite session evidence. Requests are retained in
`.kiln/contexts/`. The engine creates a dedicated integration branch from committed
HEAD and a separate branch/worktree per session under `.kiln/worktrees/`. Dirty
files in the developer checkout are never copied or modified. Git worktree isolation is combined with the mandatory execution policy below.

Only independently verified executable plans can execute. Prerequisites must have
an `integrated` session with passing verification and a commit reachable from the
run integration branch. Implementation does not integrate commits: successful
sessions remain `implemented`, awaiting subsequent independent review gates.

`Run.sessions` records branch, worktree, base revision, context identity, actual
binary Git diff, agent result/log, commit, and configured build/test exit codes,
stdout and stderr. Startup commands are not run because they can be long-lived.
Failed agent results, empty changes, and failed checks persist a `failed` session
and make the CLI exit unsuccessfully. None release dependent tickets. Worktrees
remain available for inspection, correction and later integration.

# Mandatory execution policy

Linux `/usr/bin/bwrap` is required. Preparation runs an actual isolated readiness
probe and rejects unavailable isolation; subprocess checks have no host fallback.
`isolation.commands` authorizes exact argv arrays launched by the engine, including
configured build/test/startup and provider launch argv. A denied command is rejected
before launch. `runtime: "system"` explicitly delegates all tools and interpreters
under read-only `/usr`, `/bin`, `/lib`, `/lib64`; it is not a child-executable
allowlist. Authorized Python/shell/compiler programs may execute arbitrary code
within the mounted filesystem and selected network policy. Agent adapters must use
`Sandbox::command` and the request's policy. The fixture adapter is trusted engine
code: its deterministic file writes validate relative paths and symlink escape;
it does not launch an untrusted process.

The sandbox has private process, user, IPC and network namespaces, private HOME,
/tmp and /dev, read-only system runtimes and certificate/resolver data, a writable
assigned worktree and read-only Git metadata. The original checkout, host home,
engine state and sockets are absent. Provider-owned additional mounts are scoped
runtime or authentication paths; arbitrary project-configured mounts are unsupported.
`network: "none"` denies network connectivity. `allow-all` explicitly shares host
networking and permits arbitrary destinations, including localhost. Domain
allowlists are rejected because no enforcing gateway is implemented.

Environment inheritance is cleared. `secrets` maps host variable names to authorized
roles (`agent`, `build`, `test`, `startup`), for example
`{"PRIVATE_TOKEN":["agent"]}`. Only names and roles are persisted; values must exist
before preparation and launch. Captured check output, agent log, failure evidence
and recorded diff redact exact and JSON-escaped values before persistence or CLI/web
observation. This does not prevent an authorized arbitrary program from encoding or
exfiltrating its secret; do not authorize a secret to a program you do not trust.
Secrets must not be supplied literally in argv, public configuration or frozen specs.

Preparation rejects registered secret literals in public configuration or specs,
retaining exact approved frozen inputs rather than silently changing their content.
Durable state serialization also redacts registered values as defense for new provider
fields. Provider adapters must sanitize any newly returned observable fields before
returning a Run; persistence redaction alone cannot sanitize an in-memory CLI response.


## Codex provider

`plan RUN --codex /absolute/path/to/codex` and `implement RUN TICKET --codex /absolute/path/to/codex` use real fresh ephemeral Codex contexts. The fixture option remains available for offline contract checks. Configure a pinned standalone installation and scoped authentication file:

```json
"codex": {
  "installation": "/opt/codex/0.160.1/codex",
  "auth": "/private/codex/auth.json",
  "model": "gpt-6-luna",
  "reasoning_effort": "high",
  "timeout_seconds": 1800
}
```

An optional `model` selects the project's chosen model. An optional `reasoning_effort` sets the Codex CLI `model_reasoning_effort` override; choose a value supported by the selected model. No override is otherwise supplied. The CLI installation path takes precedence. Authorize the exact `CodexConfig::argv()` in `isolation.commands` (the executable inside the namespace is `/codex/codex`):

```json
["/codex/codex", "exec", "--json", "--ephemeral", "--ignore-user-config", "--ignore-rules", "--color", "never", "--sandbox", "danger-full-access", "-c", "approval_policy=\"never\"", "-c", "features.code_mode=false", "-"]
```

If configuring a model, insert `"--model", "your-model"` before the final `"-"`. If configuring reasoning effort, also insert `"-c", "model_reasoning_effort=\\\"high\\\""` before the final `"-"`. Prompts travel through stdin so authorized argv remains stable. `network=allow-all` is required explicitly for cloud model access; `none` fails before launching. Authentication errors and missing capabilities fail explicitly. A sibling `codex-code-mode-host`, when present, is mounted as a scoped read-only runtime binary because recent installations require it even when the code_mode feature flag is disabled.

Only the executable, optional companion and a private ephemeral copy of auth.json are mounted. No host home or full Codex configuration is exposed. The auth copy is writable for refresh, protected with private directory/file permissions, and removed when the session finishes. Auth string fields and configured secret values are redacted before event data is returned or persisted. This protects observable artifacts; it does not prevent a deliberately hostile process from encoding or transmitting credentials it is authorized to use with unrestricted network access.

The adapter observes JSONL thread identity, completed messages, actionable provider/capability errors and token usage. Unknown event kinds are tolerated; malformed JSONL and missing completed turns fail. Subscription monetary cost is null, not zero. Implementation logs contain a serialized observation with `usage` and `cost`. Zero exit status never proves ticket completion: the engine requires a usable Git diff and independently passing configured checks. Adapted implement-spec/TDD instructions assign exactly one ticket; Rust owns staging, commits and progression.

Planning generation and verification use separate fresh Codex invocations in disposable clones. `CodexPlanningAgent::structured<T>` also supports independently validated structured review responses. `CodexAdapter::invoke` is the shared launch/observation/error boundary and `stop_handle`/`stop` cancel an invocation. Timeout or cancellation terminates the sandbox process group; the PID namespace also contains descendants. There is no host execution fallback.

Launching, isolation, observation, redaction, cancellation and every agent seam live in the provider-neutral `agent` module (`Adapter<P>`, `PlanningContexts<P>`, `scheduler::AgentProviders<P>`); a `Provider` supplies only its argv, credential layout and event vocabulary. `CodexAdapter`, `CodexPlanningAgent` and `CodexProviders` are aliases of those types.

## Claude Code provider

Every command that accepts `--codex PATH` also accepts `--claude PATH` (`import`, `plan`, `implement`, `review`, `correct`, `decide`, `replan`, `integrate`, `run`, `resume`); the two are mutually exclusive and both exclude the fixture options. Claude Code then implements, reviews, corrects, verifies plans and imports, decides, replans and revises specs, with the same engine-owned state and transitions. Configure a `claude` section:

```json
"claude": {
  "installation": "/home/me/.local/share/claude/versions/2.1.292",
  "credentials": "/home/me/.claude/.credentials.json",
  "model": "sonnet",
  "timeout_seconds": 1800
}
```

`model` is optional; without it Claude Code's default model applies. The CLI path takes precedence over `installation` (a symlink such as `~/.local/bin/claude` is resolved to its pinned native binary). Authorize the exact `ClaudeConfig::argv()` in `isolation.commands`; the executable inside the namespace is `/claude/claude`:

```json
["/claude/claude", "-p", "--output-format", "stream-json", "--verbose", "--no-session-persistence", "--permission-mode", "bypassPermissions", "--strict-mcp-config"]
```

With a model, append `"--model", "your-model"`. The prompt travels on stdin. `bypassPermissions` is safe only because the external bwrap sandbox is the boundary; `--strict-mcp-config` keeps repository MCP servers out; `--no-session-persistence` leaves no resumable transcript. `network=allow-all` is required, as for Codex.

Only the executable and a private copy of the credentials are mounted, at `/home/kiln/.claude/.credentials.json` in a 0700 directory with a 0600 file that is removed with the session. The copy keeps only the `claudeAiOauth` login: unrelated stored tokens (for example `mcpOAuth`) never enter the session. Every credential string, including refreshed values found in the copy after the session, is redacted from messages, logs, stderr, failures, run state and contexts.

The adapter reads `stream-json` events: `system/init` gives the session identity; the final `result` event gives the response text, `usage` and `total_cost_usd`. A `result` with `is_error: true` or an `error*` subtype fails with `Claude Code provider failure: <event>` carrying the provider event intact (redacted), so usage-limit and authentication messages stay classifiable; exit status 0 never hides it. Malformed events, a missing `result`, and responses that are not the requested JSON value fail exactly as for Codex, so structured reviews and verifications become visible unable-to-verify outcomes with redacted evidence. A single surrounding Markdown code fence around a structured response is accepted for both providers.

Token usage is recorded as reported (`input_tokens`, `output_tokens`, plus cache fields); limits count input plus output. `total_cost_usd` is an API-price estimate computed by the CLI, never the subscription's cost: it is recorded as `cost_estimate` while `cost` stays null, so `limits` reports it as `estimated` and never enforces a monetary ceiling against it. Stop and timeout tear down the session process group exactly like Codex.

A smoke check against the real CLI is recorded in `docs/claude-smoke.md`. Known limitations: the credential copy is discarded after each session, so if Claude Code refreshes and rotates the OAuth tokens inside the sandbox, the configured file can go stale (re-login with `claude` or point `credentials` at a dedicated login); cache-read tokens are not counted against `usage_token_limit`.

## Independent review gate

```sh
kiln --repo /path/to/project review RUN-ID TICKET-ID --fixture review-agent.json
kiln --repo /path/to/project review RUN-ID TICKET-ID --codex /absolute/path/to/codex
```

Standards and Spec each receive a fresh context, relevant frozen specs, ticket,
repository standards committed at the implementation base (root/nested AGENTS.md,
CLAUDE.md, CONTRIBUTING.md, CODE_STYLE.md and standards.md), concrete binary diff, implementation check evidence and
exact commit identity. Each provider invocation runs in a separate disposable
clone checked out at that commit, rather than the original checkout's HEAD.
`CodexAdapter::structured_at` launches structured responses at the supplied
snapshot; the Codex review adapter uses it independently for both axes.

A deterministic fixture supplies both simulated axis results:

```json
{
  "standards": {"outcome":"approved", "findings":[], "evidence":"Observed changed files follow AGENTS.md", "log":""},
  "spec": {"outcome":"approved", "findings":[], "evidence":"Observed acceptance behavior in the concrete diff", "log":"", "acceptance_checks":[]}
}
```

Outcomes are `approved`, `rejected`, or `unable-to-verify`. Findings have `code`,
`message`, `evidence`, and `required` (defaults to true). Required findings block
approval; corrections require a fresh review of the new commit. Agent approval
never overrides missing observed evidence, unavailable verification, failed
implementation checks, or a failed required build/test check. Engine build and
test checks execute independently in each reviewed snapshot and retain actual
commands, exit codes, stdout and stderr. Optional verifier `acceptance_checks`
are argv arrays executed as the test role only when explicitly authorized by the
project isolation policy. Their actual results are retained and failures block
the gate. Reviewers may propose further tests as findings; edits to reviewed
content are rejected and must pass through implementation and fresh review.

`Run.reviews` retains both outcomes, findings, provider logs, context identities,
reviewed commit/diff and executable evidence. `.kiln/contexts` retains the exact
review inputs. The CLI exits unsuccessfully on a rejected gate, preserving state
for `inspect` and the web API. `Engine::review_gate(run, session)` is the public
integration/correction seam: it requires the latest review of that session to
approve its exact commit, and checks that its worktree is clean and HEAD still
matches. Uncommitted edits and later commits invalidate approval. Review does not
change the implementation session status or merge commits.

Providers initialize credential redactors through `prepare_redaction` before
Kiln persists an implementation or review input context. Missing or invalid Codex
authentication fails explicitly before that write. Review artifacts are sanitized
as a complete serialized value, including verifier argv, findings and failures;
registered environment values and scoped auth strings are removed before state
persistence and CLI return. Regression providers inspect contexts while invocation
is running, including provider failures, rather than only after completion.

## Autonomous correction

`kiln correct RUN TICKET --codex /path/to/codex` recovers an attempted implementation or rejected review. It supplies frozen applicable specs, exact ticket, unresolved axis findings, checks, and current Git diff to a fresh correction context. Every correction runs build/test and new independent Standards and Spec reviews. It never integrates.

`correction_cycles` is an optional positive integer (default 3). Each attempt is retained in `Run.corrections` with before/after identity, findings, checks, review, and `approved`, `retry`, `no-progress`, `exhausted`, or uncharged `provider_limit` outcome. Repeated invocation cannot reset the allowance. A fixture contains `corrections` (implementation fixture sequence) and `reviews` (independent axis fixture sequence). No unchanged correction is retried indefinitely.

## Run limits

`kiln run` enforces limits configured as optional top-level project keys and
presents them, with observed usage and any exhaustion, in `scheduler.limits`
(`kiln inspect RUN`, web view):

| Key | Default | Meaning |
| --- | --- | --- |
| `correction_cycles` | 3 | Correction cycles per ticket. |
| `replanning_attempts` | 1 | Replanning attempts per ticket after its correction cycles are exhausted; `0` disables. |
| `implementation_concurrency` | 3 | Tickets simultaneously in their implementation phase. |
| `duration_limit_seconds` | none | Wall-clock budget of one `kiln run` invocation. |
| `usage_token_limit` | none | Cumulative provider tokens observed across the run. |
| `cost_limit_usd` | none | Monetary ceiling; there is no default ceiling. |
| `limit_policy` | `settle` | `settle` lets active work finish; `stop` cancels provider invocations and supervised sandbox build/test checks, including their child processes. |

Usage is accounted from recorded provider observations (`usage`, `cost`,
`cost_estimate` in session and review logs), once per provider context.
`scheduler.limits.usage.cost_data` is `measured`, `partial`, `estimated` or
`unavailable`; unavailable cost is null, never zero. The ceiling is enforced only
against measured cost (`cost_ceiling`: `enforced`, `enforced_on_measured_subset`,
`not_enforced_estimate_only`, `not_enforced_cost_unavailable`, `not_configured`).

Limits are checked before each ticket starts and before each review, correction
cycle and integration. On run-wide exhaustion nothing further starts, active
sessions settle or stop per policy, `scheduler.limits.exhausted` records the
limit and reason, interrupted tickets become `stopped` (unstarted ones stay
`waiting`, unblocked), and the run status is `limit_exhausted`; the CLI exits
unsuccessfully. Completed integrations, sessions and evidence are preserved for
resume, and correction cycles not started are not charged. Repeating `kiln run`
cannot reset cumulative usage.

### Pause and cancel

While `kiln run RUN` is active, `kiln pause RUN` stops dispatching new tickets
and lets active ticket pipelines finish. The run is saved as `paused`; use
`kiln resume RUN --fixture scenario.json` (or `--codex`) to reconcile recorded
Git state and continue without repeating completed integrations. `kiln cancel RUN`
stops active provider sessions and supervised project checks, prevents new
tickets from starting, and saves the run as `cancelled`. Cancellation is
terminal: a cancelled run cannot be started or resumed again. Both controls
communicate with the owning scheduler and do not acquire its state lock.

### Provider usage limits

A provider session that fails because the provider account's usage or rate limit
is reached (Codex: "You've hit your usage limit … try again at 10:05 AM"; Claude
Code: "usage limit reached", "5-hour limit reached ∙ resets 3pm", HTTP 429
`rate_limit_error`) is a run-wide exhaustion, never a ticket blocker.
`scheduler.limits.exhausted` records `limit: "provider_usage"`, the `provider`,
its message in `reason`, and `reset_at` (verbatim, when the provider reports
it). The same limit applies to implementation, review, correction and replanning
sessions: the cut-short step is recorded with `provider_limit` evidence
(implementation session `interrupted`; review axis `provider_limit`; correction
cycle and replanning attempt with outcome `provider_limit`). Those records carry
no verdict and are not charged against `correction_cycles` or
`replanning_attempts`. Nothing further starts, in-flight sessions settle or stop
per `limit_policy`, the ticket becomes `stopped` and the run `limit_exhausted`.
After the reset, `kiln resume` (or `kiln run`) retries the interrupted step only:
completed implementation, review, correction cycles and integrations are not
repeated.

Detection is provider-agnostic. The shared `agent::Adapter::invoke` classifies
the failure message (`message`, `error.message`, `error` or `result` field) of
any event a `Provider` reports as `Signal::Failure`, so Codex (`provider:
"codex"`) and Claude Code (`provider: "claude"`) are both covered without
provider-specific code. Other adapters pass only the provider's own failure
message (never the agent transcript) to `kiln::limits::provider_failure(provider,
message)` and return the resulting error; it carries a typed
`kiln::limits::ProviderLimit` when the text reports an exhausted usage or rate
limit (`ProviderLimit::detect` exposes the classifier). The engine recognises it
anywhere in an error chain (`ProviderLimit::in_error`). Fixture implementation,
review, correction and replanning entries accept `"provider_failure": "<text>"`
to simulate it.

Ticket correction exhaustion is ticket-scoped: the ticket is `blocked` with
`exhaustion: "correction_cycles"` while `scheduler.limits.exhausted` stays null,
so later bounded replanning can apply only when run-wide resources remain.

## Bounded replanning

When a ticket exhausts its correction cycles, `kiln run` makes at most
`replanning_attempts` (default one) replanning attempts for it, only while
run-wide limits remain (otherwise the ticket is `stopped`, resumable). A fresh
replanning context receives the ticket, its unresolved findings and the effective
specs and returns one revised ticket with the same id. The whole plan containing
it is independently reverified in a separate context (coverage, dependencies,
verifier outcome) before any revised work starts. A verified revision replaces
the ticket in `plan`, marks its earlier sessions `superseded`, and the ticket is
implemented and reviewed again. Each attempt is recorded in `replans` (`attempt`,
`failures`, `previous`, `revised`, `verification`, `findings`, `outcome`:
`replanned`, `rejected`, `failed` or uncharged `provider_limit`, and `result`: `integrated` or `blocked`).

Persistent failure after replanning, or a replan that fails reverification,
blocks the ticket with `exhaustion: "replanning"`. Its descendants never start;
independent tickets continue. No further attempts or developer prompts follow.

## Autonomous decisions

`kiln decide RUN --ambiguity FILE (--fixture FILE | --codex PATH)` resolves a
conflicting or ambiguous requirement without developer intervention. The
ambiguity is `{id, question, positions: [{source, reference, statement}]}` where
`source` is `product_objective`, `architectural_decision` or `spec`. Objectives
and architectural decisions are configured as top-level project keys:

```json
"product_objectives": [{"id": "PO-1", "statement": "..."}],
"architectural_decisions": [{"id": "ADR-1", "statement": "..."}]
```

A `spec` reference is a frozen spec path or requirement id. Unrecorded
references are refused. The engine ranks positions (product objectives, then
architectural decisions, then specs) and accepts a proposal only when it is
governed by the highest-ranked position present and preserves rationale and
evidence; otherwise the decision is recorded as `rejected` with a
`hierarchy_violation` or `missing_rationale` finding. Every decision is kept in
`decisions`.

A decision may revise one spec and its affected tickets. The revision is an
explicit versioned artifact in `spec_revisions` (`version`, `path`,
`base_sha256`, `content`, `content_sha256`, `decision_id`, `status`); frozen
`specs` and the repository file are never overwritten. The plan with the revised
tickets is independently reverified against the revised specs before adoption.
Only a `verified` revision becomes effective: later sessions, reviews and
corrections receive it. A `rejected` revision stays visible and leaves the
accepted plan unchanged. The CLI exits unsuccessfully unless the decision is
`resolved`.

## Replanning after approved spec changes

Editing an approved spec file never changes a run: `specs` stay frozen and
`kiln run` keeps using the current input version. Apply the edit explicitly:

```sh
kiln replan <run-id> --spec one.md [--spec two.md] --fixture replan.json   # or --codex <path>
```

Each named spec must be a frozen input of the run. Changed specs become new
`spec_revisions` (with `replan_id`); the highest verified revision version is the
run's input version (0 = frozen specs). Requirements whose criterion changed, was
added or was removed are `changed_requirements`; tickets covering them are
`affected_tickets`. A fresh session returns revised (same id) or new tickets and
lists obsolete affected tickets explicitly in `remove_ticket_ids`. The whole plan
is independently reverified against the revised specs in a separate context;
unknown blockers and dependency cycles still reject it. Fixture shape:
`{"tickets":[...], "remove_ticket_ids":[], "verification":{"outcome","findings"}}`.

Only a `replanned` outcome takes effect (`kiln replan` exits unsuccessfully
otherwise, and a `rejected` revision stays visible without changing anything):

- Affected tickets' sessions (running, implemented or integrated) and integrations
  become `superseded` and are re-executed against the new input version. A session
  that finishes after the replan stays `superseded`, so stale results are never
  integrated. Sessions record their `input_version`.
- Descendants of affected tickets (`dependent_tickets`) keep their integrated work,
  but are not treated as integrated until their evidence is revalidated: once
  their prerequisites are re-integrated, the scheduler checks the retained commit is
  still on the integration branch and reruns the configured build and test checks
  on the combined result (`revalidations`). They are never re-implemented.
- Other integrated work is `retained` with its session, commit and input version.
- Earlier validation reports are listed in `invalidated_validation_reports`;
  `kiln publish` refuses a report whose `input_version` is not the current one.
- `previous_plan`, `spec_revisions` and `decisions` keep old and new decisions
  inspectable.

Replanning requires an executable plan and refuses while a scheduler process
owns the run.

## Resume

`kiln resume RUN --fixture scenario.json` (or `--codex`) reopens an interrupted or
limit-stopped run. It refuses while another process still owns the run. Before
scheduling it reconciles recorded state with observed Git state and appends a
`recoveries` entry whose `decisions` name each ticket, subject (session, integration
attempt or lock), action and reason:

- `adopted`: a Git effect completed before its record (implementation commit
  extending the recorded base; integration ref already containing the verified
  candidate). It is not repeated; an adopted commit's lost checks are rerun.
- `completed`: a verified integration candidate whose base is unchanged gets its
  compare-and-swap ref update.
- `restarted`: interrupted implementation or integration, and sessions cancelled by a
  run-wide limit (including a provider usage limit), restart in a fresh session or
  attempt; old evidence is kept.
- `continued`: a correction cut short by a provider usage limit continues bounded
  correction on the unchanged session without charging a cycle.
- `rerun`: missing or interrupted review, including one cut short by a provider
  usage limit (or failed re-verification), is rerun.
- `unable-to-verify`: recorded integration the branch no longer contains; the ticket
  is blocked, never counted as success.
- `preserved`: integrated work, recorded blockers, frozen specs and review history.
- `released`: an integration lock left by this run's interrupted process.

Tests inject interruptions with `KILN_FAULT_INJECT=<point>@<ticket>` (points:
`implementation.after_agent`, `implementation.after_commit`, `review.after_axis`,
`integration.after_merge`, `integration.before_update_ref`,
`integration.after_update_ref`); the process exits with status 86.
