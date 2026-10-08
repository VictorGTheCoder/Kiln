# Publishing a verified delivery

`kiln publish RUN` pushes `kiln/RUN/integration` to the configured remote and opens a
pull request against the configured target branch. Only the exact commit recorded as
`verified` by the latest validation report can be published: a missing, `failed` or
`unable-to-verify` report, or an integration branch that moved after validation, is
refused before any remote effect.

```json
"publication": {
  "github_repository": "owner/name",
  "target_branch": "main",
  "remote": "origin",
  "required_checks_timeout_seconds": 600,
  "required_checks_poll_seconds": 5,
  "merge": false,
  "deploy": false
}
```

For whole-snapshot backlog runs, `publish` creates one draft PR per dependency-connected
delivery group. It validates only the group's covered criteria and keeps independent
verified groups moving when another group is blocked or failed. Required GitHub
checks are discovered from branch protection and polled against the exact pushed
head commit. A failed check can start a bounded group repair using the run's
`correction_cycles` allowance; the corrected commit must receive fresh independent
review, configured local validation, and a new successful check result before its
group is reported as verified. Every poll observation and failed check remains in
the run report, and the latest outcome is refreshed in the open draft PR body.

`merge` and `deploy` default to `false`. Opening a pull request never authorizes
merging into the primary branch or deployment; Kiln only records the explicit
project policy (`merge_authorized`, `deploy_authorized`) and states it in the PR.
Unknown fields and non-boolean values are rejected when the run is prepared.

The PR description (English) contains delivered behavior (integrated tickets and their
criteria), frozen spec paths and hashes, global checks and per-criterion validation
evidence, and limitations (unintegrated tickets, evidence scope, merge/deploy policy).

`Run.publication` durably records status (`pushing`, `pushed`, `published`), the
validation report, published commit, remote, branch, target branch and PR number/URL.
Retries skip a push when the remote branch already holds the commit, adopt an existing
open PR for the same head/base (refreshing its description, `reconciled: true`)
instead of creating a duplicate, and return immediately once the same commit is
`published`.

GitHub access uses `gh api` (`--gh PATH` selects the program). `--fixture github.json`
simulates GitHub with `{"pull_requests":[...], "interrupt":"before_create"|"after_create"}`
for offline tests; `interrupt` fails the next publication once at that point.
Delivery-group check fixtures may add `required_check_names` and exact-head
`check_runs` entries (`pull_request`, `commit`, `name`, `status`, `conclusion`,
`id`, `url`). `check_snapshots` supplies successive polling responses; fixture
`commit: "current"` binds a response to the exact SHA requested by that poll.

## Synchronizing progress to imported issues

`kiln sync RUN` reflects recorded progress in one Kiln-owned progress comment
(marked `<!-- kiln:progress run=RUN ticket=ID -->`) on every imported issue. Issue
titles, bodies and other comments are never modified.

States are derived only from recorded results: `not-started`, `blocked` (unintegrated
prerequisites, blocked integration or exhausted correction), `failed`, `in-progress`
(an agent exit or a pushed branch is never completion), `awaiting-validation`,
`validation-failed`, `unable-to-verify`, `verified` (not yet published),
`verified-on-pull-request` (links the PR, its validation evidence and the commit; not
merged) and `merged` (the verified commit is reachable from the remote target branch,
the only state reported as completed).

`Run.synchronization` records per issue the state, comment id, confirmed body hash and
a pending hash written before every remote write. Retries adopt an existing marked
comment instead of posting a duplicate, and unchanged progress writes nothing. An
issue edited since import is surfaced as divergence (the approved repository spec
stays authoritative); a Kiln comment edited on GitHub is surfaced as a `conflict`, is
not overwritten, and makes the command exit non-zero.

Only comments authored by the identity Kiln writes as (`gh api user`, resolved once
per sync) can be adopted as its progress comment. A marked comment by any other
author is ignored, Kiln posts its own, and the forgery is surfaced as divergence.
Every state write is a short locked transaction (`Engine::transact`) that changes only
that issue's synchronization record, so concurrent Kiln commands keep their writes.

GitHub access uses `gh api` (`--gh PATH`); `--fixture github.json` simulates issues with
`{"user":"LOGIN", "issues":[{repository, number, title, body, comments:[{id, author, body}]}],
"interrupt":"before_comment"|"after_comment"}`.
