# Importing issues with independent Codex verification

`kiln import` can verify selected GitHub issue tickets with either the existing deterministic fixture adapter or a fresh Codex context. This preserves the imported issue's canonical `github:owner/repository#number` identity and dependencies for later progress synchronization, while keeping the approved frozen specs authoritative.

Prepare the run with the approved repository specs and a project configuration containing Codex authentication settings. Then choose exactly one verification provider:

```sh
kiln --repo /path/to/project import RUN \
  --github-repo owner/name \
  --issue 41 --issue 42 \
  --codex /absolute/path/to/codex
```

The `--codex` option reads `codex.auth`, `codex.timeout_seconds`, the optional model, and the isolation policy from the prepared run configuration. The supplied executable path overrides `codex.installation`, matching the other real Codex CLI commands. `network: "allow-all"` must be explicit. The executable and private authentication copy are mounted by the adapter into a fresh isolated context.

The existing offline option remains supported:

```sh
kiln --repo /path/to/project import RUN \
  --github-repo owner/name \
  --issue 41 --issue 42 \
  --verification-fixture verification.json
```

Exactly one of `--codex` and `--verification-fixture` is required. Supplying both is rejected. Codex independently checks the imported issue tickets against the exact frozen specs, criterion coverage, dependency graph, and ticket granularity. Findings are reserved for blocking defects; a verified response must return an empty findings array. A rejected, unable-to-verify, or finding-bearing result is recorded and prevents the imported plan from becoming executable. Existing issue URLs and dependency identities remain attached to the run for `kiln sync` after execution.

This provider choice changes only import verification. The ticket proposals still come from the selected GitHub issue bodies; it does not call Codex to generate a second ticket plan. The current `plan --codex` command is a separate path for runs prepared without imported issues. Provider token usage from planning verification is not retained in the plan record, so this path provides a real verification outcome and context identity but no planning cost estimate. Do not describe imported ticket text as Codex-generated.

The command only reads GitHub when using the real issue source. It does not create, edit, label, close, or comment on issues. Progress comments are a separate explicit `kiln sync RUN` operation.
