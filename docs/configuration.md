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
  "acceptance_criteria": ["The combined application fulfills both approved specs"]
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

The web root lists runs; `/runs/<id>` displays recorded state.
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
files in the developer checkout are never copied or modified. This Git isolation
is not an operating-system sandbox or an execution authorization policy.

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
