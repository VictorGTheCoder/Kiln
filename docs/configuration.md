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
