# Global validation

`kiln validate RUN [--verifier verifier.json]` assesses the current tip of
`kiln/RUN/integration` in an engine-owned detached worktree
(`.kiln/worktrees/RUN-validation-N`, retained for evidence). The caller's checkout
and dirty files are untouched. `kiln report RUN` prints the latest report grouped
into `verified`, `failed` and `unable_to_verify` criteria with their evidence.

The optional `validation` section of the project configuration maps acceptance
workflows to frozen criterion IDs (`docs/first.md#ac-1`). Every argv must be
authorized by `isolation.commands`; preparation rejects unauthorized entries.

```json
"validation": {
  "startup_probe": ["sh", "-c", "test -f .ready"],
  "timeout_ms": 120000,
  "workflows": [{"criterion": "docs/first.md#ac-1", "command": ["sh", "e2e.sh"]}]
}
```

Kiln runs build and test, then starts the configured `startup` command and waits
until the probe succeeds (without a probe: still running or exited successfully
after a short grace period). Workflows then run against the started application,
which is stopped afterwards. `timeout_ms` bounds readiness and each check. Every
command runs in its own sandbox; with `network: none` each has a private network
namespace, so workflows that need to reach the application over loopback require
`network: allow-all`.

An independent verifier may add tests derived from the specs with
`--verifier`: `{"acceptance_checks":[{"criterion":"docs/first.md#ac-2","command":[...]}]}`.
Their evidence is marked `"source": "verifier"`. Unknown criterion IDs are rejected.

Each criterion records its spec path, frozen `content_sha256`, source revision and
evidence (source, availability and the executed check with argv, exit code, stdout
and stderr); the report records the exact `integrated_commit` and global checks.

Outcomes:

- `failed`: a check ran and failed. Any failed criterion or global check (build,
  test, startup readiness) makes the whole validation fail, even when every ticket
  passed its own checks in isolation.
- `unable-to-verify`: no evidence exists for a criterion, a command could not run
  (for example an unauthorized verifier command), global checks prevented
  workflows from running, or no integrated revision exists.
- `verified`: every criterion has only passing, available evidence and all global
  checks passed.

`validate` exits unsuccessfully unless the overall outcome is `verified`; reports
accumulate in `Run.validation_reports`, visible through `inspect` and the web API.
