# Kiln

Kiln is a Rust orchestrator that takes approved specs to an integrated, verified application by coordinating agents, reviews, corrections, and dependencies.

## Project status

The product specification and its decomposition into 19 tickets are approved. The Rust foundation prepares and inspects durable workflow runs through the CLI and local web view. Agent execution is added in subsequent implementation slices.

See [configuration and CLI usage](docs/configuration.md). Run `cargo test` to verify preparation behavior in temporary Git repositories and the shared local web state.

The [product specification](docs/spec.md) defines the first-version scope: a Rust engine, CLI, local web interface, Codex as the first agent engine, concurrent execution, and recovery after interruptions. The [GitHub issues](https://github.com/VictorGTheCoder/kiln/issues) are the implementation tracker.

See the [approved ticket index](docs/implementation-tickets.md) for the 19 implementation slices and their blocking dependencies.

## Target workflow

Approved specs → verified tickets → implementation → review → corrections → integration → global validation → pull request.

The Rust engine owns execution state. Matt Pocock's skills provide instructions to agents.

## Language

Repository documentation, specs, issues, and project text are written in English.
