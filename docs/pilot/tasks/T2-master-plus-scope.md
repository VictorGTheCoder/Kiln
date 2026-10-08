# T2 — Make the Playbook's Master+ scope explicit

## Packet status

Frozen against repository `VictorGTheCoder/tft-improvement-system` at base commit `bb4806e17b842f19a83de2113f1ec9677a1be6aa`. Two fresh attempts per system use separate clean copies of this exact commit.

## Original request source

Verbatim scope bullets from the repository owner's open Playbook 18.2 change request:

> make **Master+** the explicit target audience for Playbook 18.2 and Meta Pulse
>
> use Master+ composition statistics as the default quantitative lens, with GM+/Challenger evidence as a higher-elo cross-check when sample sizes are usable
>
> do not promote comps/routes from lower-rank aggregate performance or from guide existence alone

The existing product rule is also retained: 18.2 stays `editorialOnly`; the live default remains the Riot-backed 18.1 dataset.

## Shared provider request

Make the intended Master+ audience and evidence scope clear in the 18.2 Playbook and Meta Pulse. Treat Master+ composition statistics as the primary quantitative lens and GM+/Challenger as a cross-check only when sample sizes support it. Lower-rank aggregate performance or the existence of a guide alone must not promote a composition or route. Preserve the current 18.1 live recommendation default and keep 18.2 editorial-only.

Use only the evidence and source material already present at the frozen base commit. Do not browse, fetch new game data, or make claims about later patches. Deliver a commit and a pull request in the isolated evaluation copy. Report commands run and any unavailable evidence.

## Acceptance criteria

1. The 18.2 Playbook visibly identifies Master+ as its target audience and explains the role of GM+/Challenger cross-checks.
2. Meta Pulse visibly identifies the same rank scope; its article data does not imply unsupported rank coverage.
3. No lower-rank aggregate or guide-only signal is promoted as quantitative proof.
4. Patch 18.2 remains editorial-only and the default live recommendation path remains 18.1.
5. Existing Playbook content, data contracts, source references, and non-18.2 views remain valid.
6. The delivered commit passes the required checks below.

## Required checks

- `npm --prefix engine test`
- `npm --prefix engine run typecheck`
- `npm --prefix ui-v2 run build`
- `npm test`

## Independent evaluator procedure

On the exact delivered commit, inspect both the 18.2 Playbook and Meta Pulse in a browser or preview. Verify the audience and evidence scope are visible without relying on a tooltip or source code. Inspect the structured 18.2 data and tests for rank-scope semantics, confirm source references remain attached, and assert that the current live default is still 18.1 and 18.2 remains editorial-only. Retain screenshots or interaction notes, check output, delivered SHA, and evaluator notes.

## Approved Kiln spec

Implement only this packet in Kiln's normal approved-spec workflow. The user-facing behavior is the acceptance criteria above. Preserve current product safety boundaries and let the agent choose the implementation. Evaluate the exact delivered commit independently with the procedure above.
