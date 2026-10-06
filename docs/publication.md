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
  "merge": false,
  "deploy": false
}
```

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
