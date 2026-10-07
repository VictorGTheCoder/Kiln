# Competitive pilot readiness

Status: **blocked from scored runs**. This record captures preparation under issues [#35](https://github.com/VictorGTheCoder/Kiln/issues/35) and [#36](https://github.com/VictorGTheCoder/Kiln/issues/36). The repository and base commit are selected; task packets are not frozen, and no provider trial has started.

## Selected repository

- **Repository:** [VictorGTheCoder/tft-improvement-system](https://github.com/VictorGTheCoder/tft-improvement-system), a private TypeScript application for post-game TFT learning and decision review.
- **Authorization:** the owner authorized use of this repository for the pilot in the conversation. Evaluation copies must be isolated from each other and from existing PR implementations.
- **Frozen base commit:** `bb4806e17b842f19a83de2113f1ec9677a1be6aa` (`master`, inspected from a clean read-only clone on 2026-10-07).
- **Repository checks:** root `npm test` runs `node test-core.js`, Overwolf capture checks, and site validation. `ui-v2` has its own Vite build and engine integration check; the exact commands for each packet still need to be fixed after selecting its code path.
- **Existing source of real needs:** open issue [#28](https://github.com/VictorGTheCoder/tft-improvement-system/issues/28) names an unconnected NextBoard analysis path and loss of duplicate champion instances in presentation. These are candidate task sources, not frozen packets yet.

## Frozen study shape

- **Systems:** Kiln using its Codex primary-provider workflow, Devin, and GitHub Copilot cloud agent.
- **Coverage:** three real tasks and two fresh attempts per task per system (18 initial attempts): T1 reproducible bug, T2 incremental feature, T3 task with a follow-up revision. Revisions are part of the corresponding T3 attempt, not extra attempts.
- **First trial:** T1 attempt 1 is the matched setup trial across all three systems. Keep its record. Count it among the eighteen scored attempts only if the frozen procedure remains valid; otherwise mark it `excluded_setup` and document why, then preserve the gap in the final report.
- **Wall-clock limit:** 45 minutes per attempt. The clock starts when the provider begins work and ends at delivery or stop. Any extension requires a recorded deviation and applies consistently to affected attempts.
- **Change permissions:** only reversible development, checks, commits, and pull request delivery in an authorized isolated copy. No merging, deployment, production changes, purchases, or external messages.
- **Spec preparation:** Kiln receives an approved spec with the same functional requirements supplied to the other systems. Record original request and spec-preparation minutes; the current pilot measures Kiln's spec-driven workflow, not automatic prompt interpretation.
- **Decision rule:** unresolved. The 30% active-time improvement threshold in the research document is a proposal, not a frozen criterion. The owner must confirm or replace it before scored runs.

## Provider order

Use the following balanced launch order. Each system appears twice in each position across the six task-attempt groups. Start each attempt in a fresh session except for the deliberate T3 follow-up continuation.

| Task attempt | Launch order |
| --- | --- |
| T1-1 | Kiln, Devin, Copilot |
| T1-2 | Copilot, Devin, Kiln |
| T2-1 | Devin, Copilot, Kiln |
| T2-2 | Kiln, Copilot, Devin |
| T3-1 | Copilot, Kiln, Devin |
| T3-2 | Devin, Kiln, Copilot |

Record actual start/end timestamps and deviations. Do not expose earlier providers' output, prompts beyond the frozen shared packet, or completed solutions to later runs.

## Readiness prerequisites

| Prerequisite | Current evidence | State |
| --- | --- | --- |
| Select one real TypeScript repository | Owner selected `tft-improvement-system`; it contains a substantial TypeScript application. | **Complete** |
| Authorize isolated evaluation copies | Owner said the TFT repo may be used for the pilot. Keep copies isolated and do not merge or deploy. | **Complete for pilot scope** |
| Name three actual developer needs | Issue #28 provides two concrete needs: duplicate-champion presentation and the disconnected NextBoard path. A distinct third need and the final task/category mapping remain unidentified. | **Partially complete** |
| Freeze exact base commits and checks | Base commit is frozen as `bb4806e17b842f19a83de2113f1ec9677a1be6aa`. Per-task checks depend on finalized task scopes. | **Partially complete** |
| Confirm provider access and compatible environments | GitHub CLI access is available. Devin and Copilot access, account controls, and compatible execution environments are unverified. Local Codex availability alone does not prove the Kiln workflow is ready on the target. | **Owner confirmation and preflight required** |
| Authorize usage allowances | No paid purchase or dollar allowance is assumed. Native caps and stop behavior depend on the accounts selected. | **Owner confirmation required** |
| Freeze decision threshold | Current 30% threshold is only proposed. | **Owner decision required** |
| Freeze three task packets | Original prompts, equivalent provider instructions, approved Kiln specs, acceptance criteria, required checks, evaluator steps, and evidence targets need final task scopes. | **Pending** |

Scored attempts must not start until these prerequisites are resolved and recorded. Access failures or unavailable measurements should be reported, not replaced with assumptions.

## Repository inspection findings

Issue #28 identifies two candidate needs, and the checked-out source confirms both concerns:

- `ui-v2/src/lib/spot-analysis.ts` defines `earlyEvidence18_1 = null` and passes it to `analyzeNextBoards()`. The existing UI therefore receives `no-data` for that path. Issue #28 asks to reconnect real evidence if the feature is meant to be active, or remove/mask the inactive path. The packet must choose one observable outcome after the owner confirms the intended behavior.
- The same file builds `Map<championId, UnitState>` for current board and bench units. Multiple copies of one champion overwrite each other, so transitions can display the wrong instance or lose its star/item details. This is a reproducible bug candidate; the packet still needs a frozen fixture and evaluator steps.

The repository also has an open PR #26 containing a completed implementation for a separate Playbook 18.2 content audit. It is not selected as a benchmark task because its existing solution would create avoidable contamination.

## Attempt measurement rules

### Human attention

Record active developer minutes per attempt in separate fields:

- `prompt_minutes`: writing and translating the original request into the shared packet, plus Kiln spec preparation for Kiln. Record shared packet authoring once in the first task attempt and cross-reference it; do not duplicate that time across systems.
- `supervision_minutes`: active monitoring, clarifications, decisions, and interventions during execution. Waiting time is not active developer time.
- `review_minutes`: developer review and independent acceptance evaluation after delivery.
- `repair_minutes`: manual human code changes. Set `manual_code_repair=true` for any human code edit, even if the result is accepted afterward.
- `onboarding_minutes`: one-time setup attributable to making a provider/repository usable. Record it once in the first attempt row for that provider and report it separately from per-task effort; do not repeat it on later rows.

Use a contemporaneous timer or timestamped activity log. Record clarifications and interventions as counts and link the corresponding notes. Include all per-task active minutes, including failed or blocked attempts, in the primary total. Report one-time onboarding separately alongside the task totals.

### Outcome categories

Use `not_run` for planned attempts that have not started; `running` only while actively in progress; `completed` when the provider handed off an output; `blocked` when an external prerequisite prevents execution; `timed_out` when the frozen limit stops work; `failed` for a launched attempt that does not deliver a verifiable result within its limits; `unable_to_verify` when an output exists but evidence or environment prevents an independent determination; and `excluded_setup` only for a setup rehearsal invalidated by a documented procedure defect. Never convert missing evidence into success. The `accepted` field is true only if all criteria and required checks pass on the exact delivered commit. A manually repaired output may be accepted but is not a no-repair acceptance.

### Cost and evidence

- Record native usage quantity and unit, measured variable cost, estimates, subscription allocation, and account-level limits in their appropriate fields or linked evidence. Blank/unavailable cost is never zero. Never present an estimate as a charge or a partial meter as total cost.
- Record the effective control and observed stop behavior for each provider before launch. If no reliable cap is available, state that and use only the separately authorized allowance; stop at the 45-minute limit.
- Retain the frozen prompt/spec, repository URL and authorization note, base commit, environment/setup notes, model/settings when exposed, run timestamps, session/PR links, delivered commit, independent check output, review notes, activity log, usage/billing evidence, and final evaluator result. Store evidence in the authorized evaluation copy or a restricted project location; do not commit credentials or private logs.
- For UI behavior, retain a browser or preview interaction record where appropriate. For a bug, retain proof of the starting failure and corrected behavior. For T3, retain both initial and revised commit identities and verify prior criteria still pass after the follow-up.

## Scorecard

[`docs/research/benchmark-scorecard.csv`](research/benchmark-scorecard.csv) now contains exactly eighteen rows: T1–T3 × attempts 1–2 × Devin/Copilot/Kiln. All rows remain `not_run` until launch prerequisites are closed. Keep unsuccessful, blocked, timed-out, and unable-to-verify outcomes in the scorecard.

Report active developer minutes per accepted change as total per-task active minutes across all attempts divided by accepted outputs; explicitly report zero-success cases. Report no-repair acceptance and completion over all launched attempts. Report cost per accepted change only from measured variable charges, including unsuccessful runs; keep subscription fees, estimates, native usage, and missing data separate. Publish raw paired task results and evidence links with any recommendation.
