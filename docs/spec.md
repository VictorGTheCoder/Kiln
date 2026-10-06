# Kiln — Autonomous Spec Orchestrator

Status: ready-for-agent

## Problem Statement

When building an application with many specs, the developer must repeatedly launch agents to implement tickets, request reviews, trigger corrections, and integrate changes. Even after documenting the product and approving its scope, this manual supervision prevents continuous execution. TFT-Simulator is the motivating use case.

The developer needs to hand an orchestrator a collection of approved specs and receive an integrated, verified, traceable result, with the ability to resume after interruptions.

## Solution

Kiln is a new Rust project, independent of the existing Node.js Auto Implements CLI. Its engine drives a workflow run from approved specs to a verified integration branch and pull request. A CLI prepares, starts, inspects, and resumes runs; a local web interface displays their state.

Kiln explicitly orchestrates ticket generation, plan verification, dependencies, implementation sessions, review gates, debug cycles, integration, and global validation. Matt Pocock's skills provide instructions to agents; the Rust engine owns durable state and controls transitions. Codex is the first supported agent engine. An adapter interface allows Claude Code to be added later.

The first version demonstrates this workflow using multiple related specs in an existing repository. The long-term goal is to implement an entire application. A run must not be declared complete merely because agents have stopped or individual ticket tests pass.

## User Stories

1. As a developer, I want to provide multiple approved specs, so that I can start a run covering a coherent set of features.
2. As a developer, I want to keep my specs in version-controlled Markdown, so that I know exactly which requirements were used.
3. As a developer, I want to import and synchronize GitHub issues, so that I can work with my existing tracker.
4. As a developer, I want repository specs to remain authoritative when content diverges, so that synchronization does not silently change requirements.
5. As a developer, I want independently verifiable tickets and their dependencies generated automatically, so that I do not have to prepare every agent instruction manually.
6. As a developer, I want a separate agent to verify coverage and dependencies, so that omissions are caught before implementation.
7. As a developer, I want independent tickets to run concurrently, so that I can reduce waiting time.
8. As a developer, I want dependencies across specs to be respected, so that work does not start before its prerequisites exist.
9. As a developer, I want each implementation session's Git changes isolated, so that agents do not interfere with one another.
10. As a developer, I want to use Codex as the first engine, so that I can use my usual environment.
11. As a developer, I want to be able to add Claude Code through an adapter, so that the orchestrator is not tied to one engine.
12. As a developer, I want reviews to run in contexts separate from implementation, so that the author does not judge its own result alone.
13. As a developer, I want repository standards and spec compliance checked, so that both technical defects and functional deviations are caught.
14. As a developer, I want Kiln to launch corrections and follow-up reviews, so that I do not have to restart every debug cycle.
15. As a developer, I want validated changes integrated and their combination rechecked, so that incompatibilities between tickets are detected.
16. As a developer, I want workflows spanning multiple specs verified, so that I know whether the application works as a whole.
17. As a developer, I want validation evidence for each spec, so that I can understand what supports the result.
18. As a developer, I want verified, failed, and unable-to-verify outcomes distinguished, so that missing evidence is not mistaken for success.
19. As a developer, I want ambiguities resolved without intervention, so that execution can remain autonomous.
20. As a developer, I want decisions and their rationale recorded, so that I can understand revisions to specs.
21. As a developer, I want specs frozen during a run, so that documentation changes do not implicitly change work in progress.
22. As a developer, I want explicit replanning after a spec changes, so that valid work is preserved and obsolete results are invalidated.
23. As a developer, I want to resume after a crash or interruption, so that completed work is not lost.
24. As a developer, I want concurrency and correction cycles limited, so that resources and unproductive loops remain controlled.
25. As a developer, I want independent tickets to continue when another ticket is blocked, so that a local failure does not stop the entire project.
26. As a developer, I want configurable duration, usage, and available cost limits, so that runs stop cleanly when those limits are exhausted.
27. As a developer, I want to see active agents, dependencies, reviews, limits, and blockers in a local web interface, so that I can monitor execution without constant intervention.
28. As a developer, I want to configure authorized commands, network access, and secrets before launch, so that execution can occur in an isolated environment.
29. As a developer, I want a pull request containing the integrated result, so that I have a reviewable deliverable.
30. As a developer, I want to explicitly choose whether Kiln may merge or deploy, so that those actions follow my project's policy.

## Implementation Decisions

- Kiln is a new Rust project, not an incremental rewrite of Auto Implements. The existing ADR describing sequential Claude Code orchestration belongs to the previous product and does not determine Kiln's architecture. The terms spec, ticket, workflow run, implementation session, review gate, and debug cycle remain useful.
- The engine is shared by the CLI and local web interface. The interface exposes run state and the evidence supporting its outcome.
- The engine owns durable state, the dependency graph, and transitions. It does not delegate the entire run to an opaque implement-spec invocation.
- Instructions from to-spec, to-tickets, and implement-spec are adapted to the autonomous execution contract. The first version starts from approved specs; product grilling and initial product approval are not automated.
- A separate agent verifies generated tickets instead of requiring human approval of every decomposition. Each ticket is a verifiable slice, and dependencies include relationships between specs.
- Version-controlled repository Markdown specs are authoritative. GitHub references are retained for import and synchronization; divergence must be visible and must not silently replace approved requirements.
- Every workflow run preserves the exact input version. Revisions made during the run are tracked; external edits require explicit replanning.
- The scheduler starts only tickets whose prerequisites are satisfied. Independent tickets may run concurrently in isolated worktrees and branches. A blocked prerequisite prevents its descendants from starting without preventing independent work.
- Codex is the first adapter. The adapter contract supports launch, result observation, stopping, and errors while leaving progression decisions with the Rust engine.
- Independent reviews examine Standards and Spec. Corrections are rechecked before integration. Post-integration checks and global workflows determine overall success.
- A verifier may add tests derived from acceptance criteria. Executable evidence complements reviews; favorable agent opinions do not replace required checks.
- Full autonomy permits decisions according to the hierarchy of product objectives, architectural decisions, and specs. Decisions and resulting spec revisions are preserved and verified.
- Limits are configurable. Initial defaults are at most three concurrent implementation agents and three correction cycles per ticket, followed by one replanning attempt before blocking if the problem persists. The implementation-agent limit is not itself a limit on every agent process.
- Duration and usage can be limited according to available data. Exact monetary cost is not assumed to be available with a subscription; estimates and unavailable data must be identified honestly. No default monetary ceiling was agreed during product discussion.
- Exhausted limits preserve resumable state. Resume reconciles recorded state with observable processes and Git changes before proceeding and avoids repeating completed effects.
- Before launch, each project configures build, test, and startup commands, acceptance criteria, and the commands, network access, and secrets authorized within its isolated environment. Kiln checks readiness before beginning.
- Commits, pushes, and pull requests can be automated on a dedicated integration branch. Merging into the primary branch and deployment are explicit per-project options, disabled for the initial delivery scenario.
- The storage implementation, web framework, isolation mechanism, and detailed adapter protocols are implementation decisions. No specific library choice has been approved.
- Repository documentation, specifications, tickets, and user-facing project text are written in English.

## Testing Decisions

- The developer approved one primary testing boundary: the externally observable behavior of a workflow run driven through the CLI in a temporary Git repository. Tests provide multiple related specs, a deterministic agent adapter, and project verification commands; they observe results, visible state, and Git artifacts rather than internal functions.
- This boundary exercises the engine, scheduling, persistence, review gates, debug cycles, and integration through observable behavior. It can simulate an incorrect implementation, unfavorable review, correction, integration failure, and interruption followed by resume.
- Scenarios verify that tickets do not start before prerequisites, concurrency is capped, independent tickets continue after a blockage, and blocked descendants do not start.
- Resume is tested after interruptions at several externally consequential steps, including between a Git change and recording that change, to detect duplicate effects and lost progression.
- Scenarios verify correction limits, the replanning attempt, exhausted limits, and preservation of resumable state.
- Validation includes a case in which individual ticket checks pass but a workflow across multiple specs fails. That case must not produce an overall success declaration.
- Unable-to-verify outcomes, autonomous decisions, and invalidations caused by spec changes remain visible and distinct from success.
- Real Codex and GitHub integrations require targeted contract verification. Deterministic engine tests do not claim to prove real model quality.
- A web-interface scenario verifies that the interface reflects engine state, including reviews, evidence, and blockers, without duplicating every engine scenario.
- Previous CLI tests provide examples of resume and review behavior but are not existing Kiln tests. The final pilot uses multiple related specs in a real repository; the specific pilot still needs to be selected.

## Out of Scope

- Modifying or migrating the existing Node.js CLI as part of this spec.
- Automating product grilling or replacing initial human approval of specs.
- Guaranteeing that an arbitrarily large application or insufficient specs can be implemented without any blockage.
- Delivering the Claude Code adapter in the first version; adding it must remain possible.
- Providing a hosted multi-user service or a native desktop application.
- Making primary-branch merging or deployment mandatory in the first version.
- Claiming exact monetary costs when the engine does not provide them.
- Selecting Rust libraries, a storage schema, or a specific isolation technology in this product spec.

## Further Notes

- The product contract and testing boundary were confirmed after grilling. Kiln is the selected project name and the GitHub repository has been created; broader naming or trademark availability has not been assessed.
- This spec defines the product and its first-version scope. The developer approved its decomposition into 19 tickets for publication on GitHub. Publishing tickets does not start implementation.
- GitHub is now the project issue tracker. Each approved ticket is published separately with the ready-for-agent label and references to its blockers.
- This English version supersedes the initial French draft without changing the approved functional scope.
