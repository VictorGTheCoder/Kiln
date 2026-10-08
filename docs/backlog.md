# Backlog runs

## Whole-snapshot graph runs

`kiln start-backlog` freezes and plans every actionable issue in one repository snapshot. It requires an independent plan verifier before it executes any ticket:

```sh
kiln --repo /path/to/project start-backlog \
  --config kiln.json \
  --github-repo owner/name \
  --codex /path/to/codex
```

Epic and tracking labels identify containers. Their open actionable issues remain independent plan candidates. Native GitHub dependencies and supported `Blocked by` issue-body references become plan edges. If acceptance criteria are absent, Kiln infers run-scoped criteria; ambiguous issues and issues with unsupported blockers stay in the durable run with a concrete `unable-to-verify` reason. Dependents of issues without an executable plan remain blocked. The scheduler continues unrelated eligible work and the run records completed, blocked, skipped, failed, and unable-to-verify issue outcomes.

The deterministic graph scenario uses `--issue-fixture`, `--planning-fixture`, and `--run-fixture`. Add `--plan-only` to freeze and independently verify a graph without starting implementation or delivery.

After scheduling, Kiln records dependency-connected delivery groups. Every edge
keeps its prerequisite in the same group, so independent groups can be delivered
without duplicating shared prerequisite work. A normal `start-backlog` run schedules
the graph, then validates and publishes every eligible group and waits for required
CI in the same command. Supply `--publication-fixture github.json` and
`--repair-fixture repair.json` for deterministic CLI runs (or `--gh PATH` for a
specific GitHub CLI). If verified groups exist without publication settings, Kiln
persists a `delivery-blocked` reason and exits non-zero. `kiln publish RUN` remains
available to retry or reconcile delivery manually. It rebuilds a branch
for each fully integrated group, runs the configured build/test checks and the
configured `validation.workflows` for every covered criterion, then opens or
reconciles one draft PR for that group. Groups that are blocked, failed, or missing
criterion evidence remain out of publication; they do not hold up independent
verified groups. Group branches are based on the frozen run base and contain only
their group's reviewed ticket commits.

Kiln discovers required status checks from GitHub branch protection and waits for
checks bound to the exact PR head commit. `publication.required_checks_timeout_seconds`
and `publication.required_checks_poll_seconds` set the wait bound and polling rate
(defaults: 600 seconds and 5 seconds). CI observations, evidence links, local group
validation, repair counts, and PR identity are included in `inspect` and `report`,
and the latest outcome is written to the draft PR description.

When a required check fails, `publish` uses the configured
`correction_cycles` budget and a correction/review provider to make a bounded repair.
Pass `--repair-fixture repair.json` for deterministic runs; its shape is
`{"corrections":[{"files":{},"outcome":"completed"}],"reviews":[{"standards":{"outcome":"approved","evidence":"..."},"spec":{"outcome":"approved","evidence":"..."}}]}`.
Every repair creates a new group head and must pass fresh independent Standards and
Spec review, configured local validation for all group criteria, and required CI
for that exact commit. Failed or interrupted attempts stay in the run and the PR
remains draft. `pause` and `cancel` work during required-check polling or repair;
the latest CI evidence and group stage remain durable. `resume RUN --fixture
scenario.json --publication-fixture github.json --repair-fixture repair.json`
reconciles scheduler state and resumes delivery on the existing group branch and
PR, without repeating completed ticket work. Kiln does not merge, deploy, or write
issue content or state.

`kiln start-issue` takes one open, independent GitHub issue through Kiln's existing workflow from a single CLI invocation:

```sh
kiln --repo /path/to/project start-issue \
  --config kiln.json \
  --github-repo owner/name \
  --issue 123 \
  --codex /path/to/codex
```

Kiln reads all open issues at start and freezes that snapshot in `.kiln/runs/<id>.json`. The selected issue must be present in that snapshot and have no blockers. Issue title, body, labels, assignee, comments, state, and dependency evidence remain read-only. Kiln derives run-scoped acceptance criteria from the issue text, includes its discussion as planning context, and asks a fresh planning context to create and independently verify an executable ticket plan against the repository checkout.

An executable plan runs through the existing isolated implementation scheduler, independent Standards and Spec reviews, correction and integration gates, project validation, and publication. Publication opens a draft pull request only after validation passes. When reconciling a matching existing pull request, Kiln updates its description and converts it to draft, confirming the GitHub draft state before recording delivery. Kiln does not merge or deploy. The run records the source snapshot, generated requirements, planning findings, skill version, execution evidence, validation result, and publication outcome.

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

## Local web controls

To start whole-graph runs from the local monitor, configure the server at launch:

```sh
kiln --repo /path/to/project serve \
  --backlog-config kiln.json \
  --github-repo owner/name \
  --codex /path/to/codex
```

The page offers one action for all open issues; issue selection and approval of
generated criteria are not required. Optional issue, planning, run, publication,
and repair fixtures can be supplied as `serve` flags for deterministic local
scenarios. The server passes these fixed settings to the same `start-backlog` and
`resume` CLI workflows, so scheduling, recovery, delivery validation, CI polling,
and draft PR publication use the same durable state and gates. Run pages show the
recorded issue dispositions and delivery evidence and expose pause, resume, and
cancel controls. Mutating requests require a loopback Host and matching same-origin
Origin; the HTTP request cannot provide command, provider, configuration, or fixture
paths. The server remains loopback-only and never writes to GitHub issues.
