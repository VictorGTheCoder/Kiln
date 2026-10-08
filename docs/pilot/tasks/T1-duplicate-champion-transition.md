# T1 — Preserve duplicate champion instances in transitions

## Packet status

Frozen against repository `VictorGTheCoder/tft-improvement-system` at base commit `bb4806e17b842f19a83de2113f1ec9677a1be6aa`. Two fresh attempts per system use separate clean copies of this exact commit.

## Original request source

Verbatim excerpt from the repository owner's open issue #28:

> La couche de présentation utilise un `Map<championId, UnitState>` pour retrouver les unités. Plusieurs copies du même champion peuvent donc s'écraser alors que le moteur préserve explicitement leur multiplicité. Cela peut entraîner étoiles/items incorrects ou réutilisation de la mauvaise instance lors de la présentation des transitions.

## Shared provider request

Fix transition presentation so duplicate copies of the same champion retain their individual identity and state. The current board and bench may each contain multiple instances with the same `championId`; their unit IDs, star levels, equipped items, and board/bench locations must not be conflated. Preserve the engine's transition semantics and the existing behavior for unique champions. Do not change scoring or recommendation ranking to work around a presentation error.

Use the repository's existing local toolchain. Do not fetch external game data, access production data, or alter the evaluation copy's base state. Deliver a commit and a pull request in the isolated evaluation copy. Report commands run and any remaining uncertainty.

## Acceptance criteria

1. A board with two instances of the same champion and different star/item state renders both instances in transition output with the correct state for each.
2. A duplicate champion split between board and bench retains the correct instance when the transition keeps, removes, or promotes it.
3. Existing unique-champion transitions keep their prior output and ordering.
4. The regression is covered through the public spot-analysis/presentation seam; no test depends on private helper names or implementation choices.
5. The delivered commit passes the required checks below.

## Required checks

- `npm --prefix engine test`
- `npm --prefix engine run typecheck`
- `npm --prefix ui-v2 run test:engine-integration`
- `npm --prefix ui-v2 run build`

## Independent evaluator procedure

From the exact delivered commit, construct a spot with two same-ID units that have distinct unit IDs, star levels, and items. Exercise transitions that keep one copy, remove one copy, and promote a bench copy. Compare each output instance against the source instance selected by the engine transition, not merely against the champion ID. Repeat with a unique-champion spot and confirm the existing result is unchanged. Retain the spot fixture, command output, delivered SHA, and evaluator notes.

## Approved Kiln spec

Implement only this packet in Kiln's normal approved-spec workflow. The user-facing behavior is the acceptance criteria above. Let the agent choose the internal representation; independently evaluate the delivered commit with the procedure above. No additional product decisions or external services are in scope.
