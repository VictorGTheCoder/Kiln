# One-issue backlog runs

`kiln start-issue` takes one open, independent GitHub issue through Kiln's existing workflow from a single CLI invocation:

```sh
kiln --repo /path/to/project start-issue \
  --config kiln.json \
  --github-repo owner/name \
  --issue 123 \
  --codex /path/to/codex
```

Kiln reads all open issues at start and freezes that snapshot in `.kiln/runs/<id>.json`. The selected issue must be present in that snapshot and have no blockers. Issue title, body, labels, assignee, comments, state, and dependency evidence remain read-only. Kiln derives run-scoped acceptance criteria from the issue text, includes its discussion as planning context, and asks a fresh planning context to create and independently verify an executable ticket plan against the repository checkout.

An executable plan runs through the existing isolated implementation scheduler, independent Standards and Spec reviews, correction and integration gates, project validation, and publication. Publication opens a draft pull request only after validation passes. Kiln does not merge or deploy. The run records the source snapshot, generated requirements, planning findings, skill version, execution evidence, validation result, and publication outcome.

The pinned/adapted workflow contract is recorded as `kiln-mattpocock-workflow-1.0.0`. Its TDD implementation procedure and independent review procedure are included in the persisted implementation and review contexts.

## Deterministic CLI scenario

The same public command accepts fixture adapters for an end-to-end test in a temporary repository:

```sh
kiln start-issue \
  --config kiln.json \
  --github-repo owner/name \
  --issue 123 \
  --issue-fixture issues.json \
  --planning-fixture planning.json \
  --run-fixture scheduler.json \
  --verifier acceptance.json \
  --publication-fixture github.json
```

`--issue-fixture` contains `{"issues":[...]}` with open issue records, discussion, labels, assignee, state, and blockers. `--planning-fixture` uses the normal deterministic planning schema, and `--run-fixture` uses the scheduler scenario schema. The project still needs a Git remote and publication configuration for the final delivery. Fixture outputs exercise workflow contracts; they do not measure model quality.
