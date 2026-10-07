# Implementation tickets

The developer approved these 19 tickets and their blocking edges. GitHub issues are the implementation tracker and record the current completion state; this index provides navigation. Each issue carries acceptance criteria, spec coverage, and blocking dependencies.

Work the runnable frontier: a ticket can start only after its blockers complete. The [product specification](spec.md) remains the source of requirements.

| Ticket | GitHub issue | Blocked by |
| --- | --- | --- |
| 1 | [Prepare and inspect a workflow run](https://github.com/VictorGTheCoder/kiln/issues/1) | None |
| 2 | [Generate and independently verify tickets from specs](https://github.com/VictorGTheCoder/kiln/issues/2) | [#1](https://github.com/VictorGTheCoder/kiln/issues/1) |
| 3 | [Implement one ticket in an isolated Git worktree](https://github.com/VictorGTheCoder/kiln/issues/3) | [#2](https://github.com/VictorGTheCoder/kiln/issues/2) |
| 4 | [Run commands and agents within the authorized project environment](https://github.com/VictorGTheCoder/kiln/issues/4) | [#3](https://github.com/VictorGTheCoder/kiln/issues/3) |
| 5 | [Execute implementation sessions with Codex](https://github.com/VictorGTheCoder/kiln/issues/5) | [#4](https://github.com/VictorGTheCoder/kiln/issues/4) |
| 6 | [Review changes independently against Standards and Spec](https://github.com/VictorGTheCoder/kiln/issues/6) | [#5](https://github.com/VictorGTheCoder/kiln/issues/5) |
| 7 | [Correct findings and rerun review automatically](https://github.com/VictorGTheCoder/kiln/issues/7) | [#6](https://github.com/VictorGTheCoder/kiln/issues/6) |
| 8 | [Integrate a validated ticket and verify the combined result](https://github.com/VictorGTheCoder/kiln/issues/8) | [#7](https://github.com/VictorGTheCoder/kiln/issues/7) |
| 9 | [Schedule independent tickets concurrently](https://github.com/VictorGTheCoder/kiln/issues/9) | [#8](https://github.com/VictorGTheCoder/kiln/issues/8) |
| 10 | [Resume interrupted runs without duplicating completed work](https://github.com/VictorGTheCoder/kiln/issues/10) | [#9](https://github.com/VictorGTheCoder/kiln/issues/9) |
| 11 | [Enforce run limits and stop cleanly](https://github.com/VictorGTheCoder/kiln/issues/11) | [#7](https://github.com/VictorGTheCoder/kiln/issues/7), [#9](https://github.com/VictorGTheCoder/kiln/issues/9) |
| 12 | [Resolve ambiguities and replan autonomously](https://github.com/VictorGTheCoder/kiln/issues/12) | [#7](https://github.com/VictorGTheCoder/kiln/issues/7), [#11](https://github.com/VictorGTheCoder/kiln/issues/11) |
| 13 | [Replan explicitly when approved specs change](https://github.com/VictorGTheCoder/kiln/issues/13) | [#8](https://github.com/VictorGTheCoder/kiln/issues/8), [#12](https://github.com/VictorGTheCoder/kiln/issues/12) |
| 14 | [Validate complete workflows and report evidence per spec](https://github.com/VictorGTheCoder/kiln/issues/14) | [#8](https://github.com/VictorGTheCoder/kiln/issues/8) |
| 15 | [Import GitHub issues with dependencies and provenance](https://github.com/VictorGTheCoder/kiln/issues/15) | [#2](https://github.com/VictorGTheCoder/kiln/issues/2) |
| 16 | [Publish the verified integration branch as a pull request](https://github.com/VictorGTheCoder/kiln/issues/16) | [#14](https://github.com/VictorGTheCoder/kiln/issues/14) |
| 17 | [Synchronize verified progress back to GitHub](https://github.com/VictorGTheCoder/kiln/issues/17) | [#15](https://github.com/VictorGTheCoder/kiln/issues/15), [#16](https://github.com/VictorGTheCoder/kiln/issues/16) |
| 18 | [Monitor the workflow in the local web interface](https://github.com/VictorGTheCoder/kiln/issues/18) | [#10](https://github.com/VictorGTheCoder/kiln/issues/10), [#11](https://github.com/VictorGTheCoder/kiln/issues/11), [#14](https://github.com/VictorGTheCoder/kiln/issues/14) |
| 19 | [Demonstrate the complete workflow on a pilot repository](https://github.com/VictorGTheCoder/kiln/issues/19) | [#13](https://github.com/VictorGTheCoder/kiln/issues/13), [#17](https://github.com/VictorGTheCoder/kiln/issues/17), [#18](https://github.com/VictorGTheCoder/kiln/issues/18) |
