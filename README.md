# Kiln

Kiln is a Rust orchestrator for spec-driven software development. Give it approved Markdown specs and it coordinates coding agents through planning, implementation, review, correction, integration, and validation, with a GitHub pull request as the final deliverable.

The coding agent does not own the run. Kiln does.

Agent sessions stay focused on individual jobs. Kiln keeps the dependency graph, decides what can run in parallel, records what happened, checks the result, and preserves enough state to recover when a longer run is interrupted.

Kiln currently supports Codex and Claude Code as coding agents. The provider boundary stays separate from the orchestration engine so additional agents can be added without changing the workflow.

## How it works

```text
approved specs
    ↓
verified tickets
    ↓
parallel implementation
    ↓
independent review
    ↓
corrections when needed
    ↓
integration
    ↓
global validation
    ↓
pull request
```

Kiln treats autonomous coding as a workflow rather than one long agent conversation.

It can:

- turn approved specs into a dependency-aware implementation plan
- verify requirement coverage and ticket dependencies before execution
- run independent tickets concurrently in isolated Git worktrees
- review implementations in contexts separate from the coding session
- start correction cycles when required findings remain
- integrate reviewed changes and validate the combined result
- preserve frozen specs, evidence, decisions, limits, and recovery state
- import and synchronize GitHub issues
- publish a verified integration commit as a pull request

A successful agent response is not enough for Kiln to consider a run complete. The resulting code still has to pass the configured review, integration, and validation steps.

## Quick start

Kiln currently builds from source.

You need Rust and Git. On Linux, workflow execution also uses `bubblewrap` (`bwrap`) for process isolation.

```sh
cargo build
cargo test
```

A target repository defines its build, test, startup, acceptance, and isolation policy in `kiln.json`.

For example:

```json
{
  "build": ["cargo", "build"],
  "test": ["cargo", "test"],
  "startup": ["cargo", "run"],
  "acceptance_criteria": [
    "The combined application fulfills the approved specs"
  ],
  "isolation": {
    "network": "none",
    "runtime": "system",
    "commands": [
      ["cargo", "build"],
      ["cargo", "test"],
      ["cargo", "run"]
    ],
    "secrets": {}
  }
}
```

Prepare a run from one or more approved Markdown specs:

```sh
kiln --repo /path/to/project prepare \
  --config kiln.json \
  --spec docs/first.md \
  --spec docs/second.md
```

Preparation validates the inputs and records an immutable snapshot of the specs. It does not start coding agents yet.

Inspect recorded runs from the CLI:

```sh
kiln --repo /path/to/project inspect
```

Or open the local read-only monitoring interface:

```sh
kiln --repo /path/to/project serve --bind 127.0.0.1:3000
```

See [configuration and CLI usage](docs/configuration.md) for the complete workflow.

## One-command issue run

For one clear, independent GitHub issue, `start-issue` snapshots the repository's open issues, derives run-scoped requirements from the selected issue and repository, then runs planning, implementation, independent review, configured validation, and draft pull request publication:

```sh
kiln --repo /path/to/project start-issue \
  --config kiln.json \
  --github-repo owner/name \
  --issue 123 \
  --codex /path/to/codex
```

The issue snapshot and generated requirements are recorded with the run. Issue bodies, labels, assignees, comments, and state are read-only. Issues with dependencies or a rejected/unverifiable plan stop before implementation; successful output remains a draft pull request. See [one-issue backlog runs](docs/backlog.md) for deterministic adapter options and the recorded skill version.

## Planning from specs

In the approved-spec workflow, version-controlled Markdown files remain the source of truth for a run. The `start-issue` workflow is an explicit exception: it derives run-scoped requirements from one read-only issue and repository context without requiring an approved spec.

Each approved spec contains explicit acceptance criteria. Kiln freezes the supplied files, derives requirement identities from those criteria, and gives the planning agent that fixed input.

A separate verification context reviews the generated tickets. Kiln also performs structural checks itself, including requirement coverage, unknown dependencies, dependency cycles, duplicate ticket identities, and missing acceptance criteria.

An agent saying a plan looks good is not enough to make it executable.

## Parallel implementation

Once a plan has been verified, tickets whose prerequisites are satisfied can run independently.

Kiln creates an integration branch for the run and a separate branch and worktree for each implementation session. Dirty files in the developer checkout are not copied into those sessions.

Dependent work waits until its prerequisites have actually been integrated. A blocked ticket does not stop unrelated parts of the plan.

Concurrency and correction cycles can be limited so a run does not expand indefinitely.

## Review and correction

Implementation and review use separate agent contexts.

Kiln currently checks two review axes:

- **Standards** checks the repository's own engineering rules and conventions.
- **Spec** checks the implementation against the approved requirements and acceptance criteria.

Reviews use the actual implementation commit and diff. Build and test commands are run independently as part of the evidence.

If required findings remain, Kiln can start a correction session and send the new commit through review again.

## Integration and validation

Reviewed tickets can be integrated onto the run's integration branch.

Kiln checks the combined state after integration because failures can appear only when independently developed changes meet. Global validation then evaluates the application against the configured acceptance criteria.

Only the exact commit covered by the latest successful validation report can be published.

Kiln distinguishes between `verified`, `failed`, and `unable-to-verify`. Missing evidence is not treated as success.

## Recovery and replanning

Kiln stores run state under `.kiln/` in the target repository.

That state includes frozen inputs, tickets, sessions, reviews, corrections, integrations, validation reports, decisions, and publication progress.

After an interruption, Kiln reconciles recorded state with observable Git and process state instead of blindly repeating completed work.

Specs remain frozen during a run. If an approved spec changes, replanning is explicit and Kiln records which existing work and validation evidence are affected.

## Isolation

Coding agents and project commands do not run directly against the developer's normal checkout.

On Linux, Kiln uses `bubblewrap` to isolate processes and filesystems. Each project explicitly configures which commands may run, whether network access is available, and which secrets may be exposed to a given role.

The execution environment does not silently fall back to unrestricted host execution when isolation is unavailable.

See [configuration and CLI usage](docs/configuration.md) for the full security model and its limitations.

## Coding agents

Kiln currently supports Codex and Claude Code as real coding-agent providers.

Planning, implementation, and review use fresh agent contexts rather than carrying a single conversation through the whole run. Kiln remains responsible for Git operations, state transitions, validation, and deciding what happens next.

The provider interface keeps agent-specific integration separate from orchestration, so additional coding agents can be added without handing control of the workflow to the provider.

## GitHub integration

The approved-spec import workflow can use GitHub issues as implementation tickets while keeping repository specs authoritative. The `start-issue` workflow instead derives a run-scoped spec from one issue and repository context.

It can synchronize progress back to those issues without rewriting their titles or descriptions.

Once a run has a verified integration commit, `kiln publish` can push the integration branch and open a pull request containing delivered behavior, spec references, validation evidence, and known limitations.

Merging and deployment are separate project policies. Opening a pull request does not implicitly authorize either one.

## Documentation

- [Configuration and CLI usage](docs/configuration.md)
- [Product specification](docs/spec.md)
- [Implementation tickets](docs/implementation-tickets.md)
- [Integration](docs/integration.md)
- [Validation](docs/validation.md)
- [Publication](docs/publication.md)
- [Codex smoke testing](docs/codex-smoke.md)
- [Claude Code smoke testing](docs/claude-smoke.md)

## Project status

Kiln is under active development.

The Rust engine currently covers the core workflow from frozen specs through planning, isolated implementation, independent review, corrections, dependency-aware integration, validation, recovery, and GitHub publication.

Deterministic adapters are used to test the workflow without depending on a live provider account. Real Codex and Claude Code integrations are supported separately so deterministic engine tests are not presented as evidence of model quality.
