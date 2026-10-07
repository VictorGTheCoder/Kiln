# Real Codex contract smoke, 2026-10-06

Executed Kiln CLI in a disposable Git repository at `/tmp/kiln-codex-smoke` with the installed standalone Codex 0.160.1 and authenticated ChatGPT subscription. Explicit filesystem isolation and `network=allow-all` applied. No credentials are recorded in this evidence.

The one approved tiny ticket asked for `greeting.txt` containing exactly `hello kiln` plus newline. Initial attempt reached the provider but the installation required its companion code-mode host. Its successful exit produced no diff and Kiln recorded failure rather than completion. The adapter now mounts only that sibling executable when available and recognizes capability error events.

Retry created all 11 expected bytes. Codex reported a failing pre-change shell assertion and a passing post-change assertion. The engine independently ran `git diff --check` and `sh -c 'test "$(cat greeting.txt)" = "hello kiln"'` in the sandbox; both exited 0. Kiln recorded `status=implemented`, `verification_passed=true`, and commit `dd6440522f48f94a2143ca089c47289ac1fe8d6e`. The original checkout contained no greeting file: the change remained in its isolated assigned worktree.

The deterministic suite covers observable provider usage/thread identity, credential redaction, failure events despite exit 0, malformed JSONL, absent completed turns, timeout, explicit stop with sleeping descendants, and the CLI rejecting a completed turn without a usable Git change. It runs without a subscription. This targeted smoke establishes one real implementation invocation, not the later multi-spec product pilot.
