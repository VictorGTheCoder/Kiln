# Competitive pilot readiness

Status: **blocked from scored runs**. This record captures preparation under issues [#35](https://github.com/VictorGTheCoder/Kiln/issues/35) and [#36](https://github.com/VictorGTheCoder/Kiln/issues/36). Repository, base commit, and task packets are frozen; provider access and usage controls remain unverified. No provider trial has started.

## Selected repository

- **Repository:** [VictorGTheCoder/tft-improvement-system](https://github.com/VictorGTheCoder/tft-improvement-system), a private TypeScript application for post-game TFT learning and decision review.
- **Authorization:** the owner authorized use of this repository for the pilot in the conversation. Evaluation copies must be isolated from each other and from existing PR implementations.
- **Frozen base commit:** `bb4806e17b842f19a83de2113f1ec9677a1be6aa` (`master`, inspected from a clean read-only clone on 2026-10-07).
- **Repository checks:** root `npm test`, engine tests/typecheck, and the `ui-v2` build/integration checks are allocated by task in the packets.
- **T1 bug source:** open issue [#28](https://github.com/VictorGTheCoder/tft-improvement-system/issues/28) identifies duplicate champion instances being collapsed in transition presentation.
- **T2 feature source:** the owner's [open Playbook 18.2 change request](https://github.com/VictorGTheCoder/tft-improvement-system/pull/26) asks for explicit Master+ scope in Playbook and Meta Pulse. The packet preserves the 18.1 live default and 18.2 editorial-only boundary.
- **T3 revision source:** the same request identifies AD/Hunter as Nidalee's primary Master+ route and AP Marksman as a situational alternate. The packet adds a fixed follow-up request to make that conditional behavior observable.
- **Contamination control:** the open PR contains an implementation outside the frozen base. Provider copies must include only the pinned base commit and no PR branch/review context. Record any model/account exposure as contamination; do not use the original repository as a provider workspace.

## Frozen study shape

- **Systems:** Kiln using its Codex primary-provider workflow, Devin, and GitHub Copilot cloud agent.
- **Coverage:** three real tasks and two fresh attempts per task per system (18 initial attempts): T1 reproducible bug, T2 incremental feature, T3 task with a follow-up revision. Revisions are part of the corresponding T3 attempt, not extra attempts.
- **First trial:** T1 attempt 1 is the matched setup trial across all three systems. Keep its record. Count it among the eighteen scored attempts only if the frozen procedure remains valid; otherwise mark it `excluded_setup` and document why, then preserve the gap in the final report.
- **Wall-clock limit:** 45 minutes per attempt. The clock starts when the provider begins work and ends at delivery or stop. Any extension requires a recorded deviation and applies consistently to affected attempts.
- **Change permissions:** only reversible development, checks, commits, and pull request delivery in an authorized isolated copy. No merging, deployment, production changes, purchases, or external messages.
- **Spec preparation:** Kiln receives an approved spec with the same functional requirements supplied to the other systems. Record original request and spec-preparation minutes; the current pilot measures Kiln's spec-driven workflow, not automatic prompt interpretation.
- **Decision rule:** continue only if Kiln reduces active developer time per accepted change by at least 30% against the better competitor, has no lower no-repair acceptance rate, and does not have higher measured variable cost per accepted change. If cost is unavailable, report it as unavailable and make no cost-advantage claim. This is a small-sample product screen, not a statistical superiority claim.

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
| Name three actual developer needs | T1 comes from issue #28; T2 and T3 are bounded slices of the owner's open Playbook 18.2 request. T3's follow-up is labeled evaluator-authored. | **Complete with provenance recorded** |
| Freeze exact base commits and checks | All tasks use `bb4806e17b842f19a83de2113f1ec9677a1be6aa`; per-task checks are recorded in their packets. | **Complete** |
| Confirm provider access and compatible environments | Codex is available through this environment. Devin and Copilot entitlement, controls, and compatible execution environments have not been verified. | **Unresolved; no scored runs** |
| Authorize usage allowances | No purchase or dollar allowance is assumed. Provider-native caps and stop behavior are unknown until account access is checked. | **Unresolved; no scored runs** |
| Freeze decision threshold | At least 30% lower active time than the better competitor, no lower no-repair acceptance, and no higher measured variable cost. | **Complete** |
| Freeze three task packets | T1, T2, and T3 packets preserve source requests, shared instructions, acceptance criteria, checks, evaluator procedures, and the Kiln workflow contract. | **Complete** |

Scored attempts must not start until these prerequisites are resolved and recorded. Access failures or unavailable measurements should be reported, not replaced with assumptions.

## Repository inspection findings

Issue #28 and the checked-out source confirm the duplicate-champion concern:

- `ui-v2/src/lib/spot-analysis.ts` defines `earlyEvidence18_1 = null` and passes it to `analyzeNextBoards()`. The existing POC document records zero authorized early-game observations and an explicit no-data fallback to V1 directions. This is not a task candidate because adding fabricated early-game evidence would violate the product's source constraints.
- The same file builds `Map<championId, UnitState>` for current board and bench units. Multiple copies of one champion overwrite each other, so transitions can display the wrong instance or lose its star/item details. This is T1.

T2 and T3 are sourced from the owner's open Playbook 18.2 request. Its implementation exists on a branch outside the frozen base; provider copies must exclude that branch and its review context. If a provider can still access the prior implementation through its account or session, record that exposure and mark the attempt potentially contaminated.

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
