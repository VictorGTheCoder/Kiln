# T3 — Refine the Nidalee route and evaluate a follow-up revision

## Packet status

Frozen against repository `VictorGTheCoder/tft-improvement-system` at base commit `bb4806e17b842f19a83de2113f1ec9677a1be6aa`. Two fresh attempts per system use separate clean copies of this exact commit. Each attempt remains in one session and updates the same pull request for the follow-up.

## Original request source

Verbatim route requirement from the repository owner's open Playbook 18.2 change request:

> correct Nidalee framing: **AD/Hunter is the primary Master+ route; AP Marksman remains a real but situational conversion**

## Shared initial request

Correct the 18.2 Nidalee route so AD/Hunter is the primary Master+ recommendation. Keep AP Marksman as a valid alternate rather than deleting it. Use the frozen repository's existing 18.2 source references and preserve the rule that 18.2 remains editorial-only. Do not browse or fetch newer game data.

## Frozen follow-up request

After the initial handoff, send this same follow-up in each provider session:

> Please keep the AP Marksman route available only as a situational conversion when the board and AP/attack-speed item package support it. Make sure it does not rank like the primary AD/Hunter route when those conditions are absent. Update the guidance and regression coverage, preserve the initial requirements, and update this same pull request.

This follow-up is an evaluator-authored operationalization of the source request's “situational conversion” requirement, not a verbatim historical comment. All providers receive identical initial and follow-up text.

## Acceptance criteria

1. At initial handoff, AD/Hunter is represented as the primary route and AP Marksman remains present as an alternate.
2. After the follow-up, AP Marksman is explicitly conditional on supporting AP/attack-speed context and does not rank as primary without those conditions.
3. Previously satisfied criteria remain true after the revision; source references remain attached to the changed guidance.
4. The 18.1 live default and 18.2 editorial-only boundary remain unchanged.
5. The same pull request contains the revision, and both initial and revised commit identities are recorded.
6. The final delivered commit passes the required checks below.

## Required checks

- `npm --prefix engine test`
- `npm --prefix engine run typecheck`
- `npm --prefix ui-v2 run build`
- `npm test`

## Independent evaluator procedure

Evaluate the initial commit before sending the follow-up, recording each acceptance criterion. Then send the frozen follow-up in the same session, verify the same pull request receives the revision, and evaluate the final commit for all initial and revised criteria. Inspect the route data and user-visible guidance; do not accept an agent's report as evidence. Retain both SHAs, session and PR links, check output, evaluator notes, and browser evidence where the changed view is visible.

## Approved Kiln spec

Implement the initial request through Kiln's normal approved-spec workflow. Keep the frozen follow-up as the only post-handoff change request. Do not merge or deploy. The evaluator independently checks both handoffs and the final commit against all criteria above.

## Evaluator-only provenance and contamination note

This packet is derived from an open repository PR whose implementation is not in the frozen base commit. Provider copies must contain only the frozen base commit and must not include the PR branch, its commits, or its review context. Record the PR's existence as historical contamination risk; do not include this provenance section in provider-facing instructions. If a provider can access the prior implementation through its account or session, record that exposure and treat the attempt as potentially contaminated.
