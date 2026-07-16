# Changelog

All notable changes to SC Node are documented here. The format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project aims to
follow [Semantic Versioning](https://semver.org/) once the API stabilizes (the
API is unstable during the alpha).

> The public repository starts from a fresh initial commit; prior private
> development history is archived internally and is intentionally not part of the
> public git history.

## [0.1.0-alpha.1] — 2026-07-16

First public alpha of SC Node — an experimental, provider-neutral Rust agent
harness for executing tool-using AI agents across local and cloud models.

### Added

- **Agent harness.** `sc-agent` binary with a tool-using agent loop: send a task
  to a model, receive text and/or tool-call requests, dispatch tools through a
  central permission gate, and feed results back for the next round (bounded by a
  max-rounds limit). REPL and single-shot `run` modes.
- **Providers.** Real Ollama provider (local, enabled by default) and real NVIDIA
  NIM provider (cloud, opt-in), both live-tested. A shared OpenAI-compatible
  client (`sc-provider-core`) with incremental SSE streaming, tool-envelope
  serialization, system-parameter folding, bounded retries, secret redaction, and
  HTTPS enforcement. An OpenRouter adapter built on the same client (not yet
  live-tested).
- **Deterministic routing.** Five-step provider/model resolution with a hard
  cloud gate, local-first defaults, and no silent cloud fallback.
- **Tools.** `read_file`, `write_file`, `list_dir`, and `shell`, each subject to
  workspace path boundaries and permission policies.
- **Optional control layers.** Workspace sandbox (`sc-sandbox`), permission engine
  and approval gate (`sc-tool-core`), append-only JSONL audit (`sc-audit`),
  fail-closed execution contracts (`sc-contract`, `contract validate|explain`),
  and proof bundles with a SHA-256 audit hash chain (`sc-proof`, `proof verify`).
- **Memory core.** A backend-agnostic `sc-memory` crate (traits + reference
  backend + optional `turbovec` feature), present but not yet wired into the
  runtime.
- **CLI.** `run`, `repl`, `init` (alias `config-init`), `config-show`,
  `providers-list`, `models-list`, `audit-show`, `doctor`, `contract`, `proof`,
  plus `config-set` and `workspace-add` (currently stubs).
- **Docs.** README, `docs/ARCHITECTURE.md`, `docs/STATUS.md`, `docs/ROADMAP.md`,
  `docs/PROVIDERS.md`, `docs/SECURITY_MODEL.md`, `docs/BENCHMARKING.md`,
  `docs/DEPENDENCY_INVENTORY.md`, `SECURITY.md`, `THREAT_MODEL.md`,
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, and `THIRD_PARTY_NOTICES.md`.
- **Licensing.** Dual-licensed under `MIT OR Apache-2.0` (`LICENSE-MIT` and
  `LICENSE-APACHE`).

### Fixed

- **OpenAI-compatible tool round-trip.** `tool` role messages now carry
  `tool_call_id`, and assistant turns replay their `tool_calls`, so strict
  endpoints (e.g. NVIDIA NIM) accept multi-round tool conversations instead of
  rejecting them with HTTP 400. Found by live testing against NVIDIA NIM;
  covered by new unit tests in `sc-provider-core`.

### Known limitations

Interactive approval prompts are only available in `repl` on a TTY (`run`
auto-denies tools that require approval); the audit log is append-only, not
tamper-evident; the sandbox has a check-then-open (TOCTOU) window; shell command
arguments are not workspace-bounded; there are no per-process resource limits;
`sc-memory` is not wired into the runtime; and only Windows has been verified. See
[docs/STATUS.md](docs/STATUS.md), [docs/SECURITY_MODEL.md](docs/SECURITY_MODEL.md),
and [THREAT_MODEL.md](THREAT_MODEL.md) for the full list.
