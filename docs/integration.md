# Verified integration

`kiln integrate RUN TICKET` requires an exact reviewed, clean implementation commit.
Kiln serializes integrations using `.kiln/integration.lock`, creates a detached
engine-owned candidate worktree, merges the reviewed commit, and runs the configured
build and test commands on the combined result. Only successful, unchanged candidates
advance `kiln/RUN/integration` using a compare-and-swap Git ref update. The caller's
checkout, primary branch, and dirty files remain untouched.

Conflicts are recorded in `Run.integrations`. Supply `--fixture corrections.json`
or `--codex /path/to/codex` for bounded correction, configured checks, and fresh
independent standards/spec review of the resolved result. Unresolved conflict markers
cannot be accepted through fixture approval. Failed candidates remain inspectable and
do not mark implementation sessions integrated or release dependent tickets.

For scheduler consumers, `Engine::integrate_ticket(id, ticket, provider)` returns the
recorded run. `provider` is optional and pairs `CorrectionAgent` with `ReviewAgent`.
Require an attempt with `status == "integrated"`; its `reviewed_commit`, `base_commit`,
`candidate_commit`, `integrated_commit`, session identity, worktree, conflict paths,
and combined executable check evidence connect the result to frozen ticket/spec inputs.

An interrupted process may leave the integration lock and a `running` attempt. Inspect
its candidate and durable state before removing a stale lock; never infer integration
success from the presence of a Git commit alone. Recovery scheduling belongs to the
run controller. Candidate worktrees are retained for evidence and run-level cleanup.
