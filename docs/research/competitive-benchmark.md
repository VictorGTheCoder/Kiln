# Kiln competitive benchmark

Research date: 2026-10-07. Status: primary-source comparison complete; live competitor trials have not run. No performance winner has been established.

## Decision to make

Can Kiln deliver bounded changes in existing TypeScript applications with substantially less developer attention than Devin or GitHub Copilot cloud agent? Separately, does adopting OpenHands or factory reduce the cost of delivering that experience compared with extending the existing Rust engine?

The conversational product direction is a single prompt in a GitHub-connected web application, automatic implementation and independent review, and a verified pull request. This differs from the existing [product specification](../spec.md), which explicitly starts from approved Markdown specs and excludes initial product grilling and approval automation. This report records the proposed direction; it does not supersede that specification.

## What already exists

| System | Primary-source evidence | Implication |
| --- | --- | --- |
| Devin | Official documentation demonstrates implementing a feature in an existing repository, tests, browser verification before opening a PR, subsequent revisions, and Devin Review. [Feature example](https://docs.devin.ai/use-cases/gallery/implement-feature-from-spec). | Implementation, browser verification, revisions, and review are already competitor capabilities. The example uses a detailed spec; it does not prove reliable interpretation of a vague prompt. |
| Copilot cloud agent | GitHub documents repository research, planning, background changes, tests and linters, branch management, PR creation, and steering during a session. [GitHub surface](https://docs.github.com/en/copilot/concepts/copilot-surfaces/copilot-on-github). | Kiln competes with an integrated GitHub workflow, rather than merely an editor assistant. |
| OpenHands | Agent Canvas is a self-hosted UI and automation control center with multiple agents and execution backends. [Repository](https://github.com/OpenHands/OpenHands). | A possible interface and execution foundation; workflow equivalence still requires examination. See [foundation research](agent-foundations.md). |
| watt-mind/factory | The README describes isolated ticket worktrees, independently rerun verification, and CI/reviewer gates around existing coding agents. [Repository](https://github.com/watt-mind/factory). | Direct overlap with Kiln's orchestration thesis. See the source inspection in [foundation research](agent-foundations.md). |

These are documented capabilities, not measured reliability. Vendor examples and customer testimonials are not controlled benchmark results.

## Kiln today

Inspected checkout: `54ce0f0b95b3826365b95face519120a59de5c89`.

- [README](../../README.md), [agent adapter](../../src/agent.rs), and [CLI](../../src/main.rs) show an existing Rust workflow around planning, implementation, fresh review contexts, corrections, integration, validation, and publication. The CLI currently exposes both Codex and Claude adapters; the README's first-provider positioning should not be mistaken for the complete current provider set.
- [Specification](../spec.md) and [CLI](../../src/main.rs) require approved specs. Prompt interpretation and acceptance-criteria generation are proposed work.
- [Web implementation](../../src/web.rs) is a local monitoring interface. A hosted, authenticated write interface is proposed work.
- [Limits implementation](../../src/limits.rs) supports duration, token, and measured-cost thresholds. Monetary estimates are not enforced. Missing costs remain unavailable; partial cost data only enforces the measured subset. An advertised universal hard dollar cap would exceed the inspected implementation.
- The repository is Rust. Results on its issues are supplementary engineering evidence, not evidence for the proposed TypeScript customer segment.
- [Issue #19](https://github.com/VictorGTheCoder/kiln/issues/19) remains open and requests a real-provider pilot. The present research does not fulfill or close that issue.

## Available access

GitHub CLI is authenticated as the repository owner. A read-only assignees query for Kiln returned only the owner; this does not establish either account-wide absence or availability of Copilot. The browser had only a blank page. No authenticated Devin connection was established. Live trials require a selected target repository and confirmed provider access; a clarification is pending.

GitHub documents that cloud-agent access requires a paid Copilot plan and repository enablement. Assigning an issue starts work and creates a PR. [Start a task](https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/kick-off-a-task). Devin API automation requires Devin credentials and organization permissions. [Authentication](https://docs.devin.ai/api-reference/authentication).

## Proposed experiment

The approved pilot specification (#35) narrows the study to three real tasks and eighteen initial attempts: a reproducible bug (T1), an incremental feature (T2), and a task with a follow-up revision (T3). Run two fresh attempts per task for each of Kiln, Devin, and Copilot. First run one matched bug trial across all three systems as an explicit setup rehearsal; retain its outcomes and label it as the first scored attempt only if the frozen procedure remains valid. The six-task research draft and its 36-row scorecard are superseded for this pilot.

Freeze repository identity and authorization, exact base commits, actual task prompts, acceptance criteria, independent checks, permissions, provider order, timeout, and available provider usage controls before scored attempts. The proposed wall-clock limit is 45 minutes per attempt. Spending ceilings must be selected from actual account controls and an authorized allowance; none is assumed. The final decision threshold is also pending confirmation before launch. See the [readiness record](../pilot-readiness.md).

### Task selection

| ID | Required category | Actual task |
| --- | --- | --- |
| T1 | Reproducible bug | Candidate from TFT Improvement System issue #28: preserve the distinct instances of duplicate champions when presenting a board transition. |
| T2 | Incremental feature | Candidate from issue #28: connect the existing NextBoard analysis to real evidence so the user-facing feature can return useful results. The issue also permits hiding/removing the path if that feature is not intended to be active, so the desired outcome must be frozen. |
| T3 | Follow-up revision | A separate initial task and its exact follow-up request are still needed. |

T1 and T2 are sourced from the owner's open issue [#28 in TFT Improvement System](https://github.com/VictorGTheCoder/tft-improvement-system/issues/28); they remain candidates until the expected product behavior and evaluator cases are frozen. Do not invent needs, seed artificial defects, or treat historical issues as current developer requests. Each task needs the exact copyable original prompt, shared provider instructions, acceptance checks, approved Kiln spec, and evaluator procedure.

Useful supplementary Rust tasks already exist as historical Kiln issues: [#33](https://github.com/VictorGTheCoder/kiln/issues/33) (HTML-escaped secret redaction), [#27](https://github.com/VictorGTheCoder/kiln/issues/27) (publication concurrent state preservation), and [#30](https://github.com/VictorGTheCoder/kiln/issues/30) (credential-field selection). They are closed. Replay requires identifying pre-fix commits, excluding future solution history, and independently confirming the starting failure. They are not launch-ready benchmark targets.

### Execution and fairness

1. Use separate evaluation repository copies starting at the identical frozen commit. Exclude completed solutions and previous agent output from accessible history/context. Historical issue tasks may be contaminated by model training; record that limitation.
2. Supply identical task text and repository instructions. Record model, reasoning setting when exposed, system version, run date, environment, and allowed tools. Equalize time and available permissions; report differing native billing units.
3. Record environment onboarding separately. Include prompt writing, clarifications, supervision, review, and repairs in per-task human attention. Report one-time onboarding effort alongside the task metrics.
4. Alternate provider order between tasks. Start each attempt in a fresh session; disable or reset cross-run memory where possible and record any inability to do so. Run the revision task as a deliberate continuation only.
5. Let each provider's normal implementation and review behavior run. Apply the same evaluator checks to the exact delivered commit; author assertions alone do not satisfy acceptance criteria. The evaluator may know requirement details, but those requirements must be present in the shared prompt.
6. Stop at the frozen budget. Record failures, missing evidence, blocked access, and exceeded limits. A clarification is an intervention, not automatic failure. A task requiring manual code repair fails the no-repair metric.
7. Evaluate outputs without provider labels where feasible. Do not merge or deploy evaluation changes. Keep evaluation notes, logs, PR links, commit hashes, and billing evidence.

### Scoring

- **Primary:** total active developer minutes per accepted change, including failed attempts and manual repairs. Compute total developer minutes across a provider's runs divided by accepted outputs; report total minutes and zero-success cases explicitly. Also report per-task paired comparisons.
- **No-repair acceptance rate:** accepted outputs requiring zero human code edits / all attempts.
- **Completion rate:** outputs satisfying every acceptance criterion within limits / all attempts.
- **Cost per accepted change:** total measured variable charges, including failures, / accepted outputs. Report subscription fees, included credits, usage units, and estimates separately. Missing cost is unavailable, never zero.
- **Secondary:** wall-clock duration, clarifications, interventions, independent check failures, unsupported claims, and revision success.

Use the [CSV scorecard](benchmark-scorecard.csv), reconciled to exactly eighteen planned rows. Blank measurements mean not collected. An unrun attempt is `not_run`, not failure and not success. The [readiness record](../pilot-readiness.md) defines the human-attention fields, outcome categories, missing-cost treatment, and evidence retention requirements.

### Decision rule

Proposed pilot threshold: Kiln reduces active developer time by at least 30% against the better competitor, with no lower no-repair acceptance rate and no material increase in cost per accepted change. This is a product decision threshold, not a scientifically established constant. Agree it before running the evaluation. Report small sample counts and raw paired results; the pilot screens for a promising advantage and does not establish universal superiority.

If the advantage appears only in one task category, narrow the product to that category. If no advantage appears, prefer adapting an existing foundation or focusing Kiln on a demonstrated workflow gap before building the hosted product.

## Next executable step

Confirm the target TypeScript repository, actual developer needs, copy authorization, provider access and usage controls, and the decision threshold. Then freeze three task packets and compatible environments before any scored run. Until then, the scorecard remains unrun and no scored attempt is authorized.
