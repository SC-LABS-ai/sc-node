# SC Node public Rust API policy

> **As of:** 2026-08-05 · Public alpha. No crate has been published to crates.io yet.

## Purpose

SC Node exposes several Rust crates for embedding providers, tools, contracts, and proof generation. This document defines which alpha surfaces are intentionally consumable, how compatibility is tested, and which crates are candidates for a first publication.

## Compatibility baseline

The separate crate at `tests/public-api-consumer` depends on SC Node only through normal public path dependencies. It must compile and pass on Windows and Linux. The fixture covers these intentional alpha surfaces:

- `sc-message-types`: roles, content blocks, messages and constructors, tool definitions/calls/results, session IDs, audit/provider/model metadata, stream events, and completion requests;
- `sc-provider-core`: `Provider`, `ProviderError`, `Result`, `EventStream`, provider information, and deterministic routing types/functions;
- `sc-tool-core`: `Tool`, `ApprovalGate`, `ToolRegistry`, tool/approval decisions, tool contexts/results/errors, and permission evaluation;
- `sc-contract`: contract parsing, validation, canonicalization, policy hashing, explanation, policy enums, and preflight reports;
- `sc-proof`: redaction, audit events, hash-chain construction/verification, proof bundles, check outcomes, and builder methods.

Removing or changing a covered symbol requires an intentional baseline update, a changelog entry, and a new alpha version.

## Explicitly experimental surfaces

The following remain public for implementation reuse but are not yet compatibility commitments:

- OpenAI-compatible wire structs and request/response normalization internals;
- the incremental SSE decoder and its buffering limits;
- CLI-specific approval UI;
- direct public struct fields where constructors/builders have not yet replaced literal construction;
- `sc-tool-core` coupling to `sc-config` and `sc-sandbox`;
- all runtime wiring in `sc-agent-core`.

Users should expect these areas to change between alpha releases.

## SemVer intent

- During `0.1.0-alpha.N`, breaking changes are allowed only in a new alpha release and must be documented. Published bytes for an existing alpha tag are immutable.
- Additive changes should remain source compatible where practical.
- Once a non-prerelease `0.1.x` line is published, breaking API changes require `0.2.0`.
- A future `1.0.0` will indicate the full documented public surface is stable under normal SemVer rules.
- Serialization formats are versioned separately where a schema version exists; Rust type compatibility does not silently change contract schema semantics.

## Publication waves

### Wave 1: independent pure crates

1. `sc-message-types`
2. `sc-contract`
3. `sc-proof`

These crates have no unpublished SC Node dependencies. The public API gate performs full `cargo package` verification for them.

### Wave 2: provider abstraction

4. `sc-provider-core`, after `sc-message-types` is available at the matching version.

Its package manifest is validated now, but registry verification must wait until the dependency is published.

### Deferred

- `sc-tool-core`, until the `sc-config` and `sc-sandbox` public surfaces are reviewed and publication order is defined.
- provider adapters, runtime crates, tools, and the CLI binary.

No crate is published automatically by CI. Publication requires a dedicated release PR, clean package verification, release notes, an owner-approved crates.io action, and post-publish installation verification.

## Required gate

Run from the repository root:

```powershell
./scripts/verify-public-api.ps1
```

The gate checks the external consumer, treats Rustdoc warnings as errors, fully packages Wave 1, and validates the package manifests/files for the later crates.
