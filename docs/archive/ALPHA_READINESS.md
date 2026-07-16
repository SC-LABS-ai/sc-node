> **ARCHIVED / HISTORICAL — may be outdated.** Kept for context only; see
> [../STATUS.md](../STATUS.md) for the current status. Original follows.

---

# SC Node — Alpha Readiness Assessment

> **Date:** 2026-07-09 · **Commit:** (current)

## Capability Score: 6.0 / 10

This is a **private alpha with two real providers** — Ollama (local) and NVIDIA NIM (cloud). OpenRouter remains stubbed. It can run real local and cloud LLM tasks but does not yet chain tool results into follow-up reasoning. Not beta, not production.

## Scoring Breakdown

| Category | Score | Notes |
|----------|-------|-------|
| **Build & Compilation** | 9/10 | Zero warnings, all crates compile, 22 tests pass, clippy clean |
| **CLI Usability** | 7/10 | --help, --version, init, config show, providers list, models list, run, repl, doctor, audit show all work |
| **Provider System** | 7/10 | Ollama fully real. NVIDIA NIM fully real. OpenRouter stubbed. Trait design validated by two real implementations. |
| **Tool Execution** | 4/10 | File tools work (sandboxed). Shell tool works (approval prompt). Not audited. |
| **Agent Loop** | 2/10 | Single-shot works. No tool-result feedback. No multi-turn. |
| **Security** | 5/10 | No telemetry. API keys from env only. Sandboxing exists. Audit not wired. Windows hardening absent. |
| **Testing** | 5/10 | 22 tests (3 config, 1 audit, 11 ollama, 7 NIM). No sandbox, tool execution, or integration tests. |
| **Documentation** | 6/10 | STATUS, ALPHA_READINESS, SECURITY_NOTES, PROVIDER_SYSTEM, OLLAMA_BACKEND_PLAN present. Missing provider setup guides. |
| **Cross-Platform** | 3/10 | Compiles on Windows (MSVC). Linux not tested. macOS unknown. |

**Weighted average:** ~6.0/10 — solid alpha with two working providers, ready for agent loop and audit wiring.

## Score Changes Since Last Assessment (5.0 → 6.0)

| Change | Effect |
|--------|--------|
| NVIDIA NIM fully implemented (health, models, streaming) | Provider System 5→7, CLI Usability 5→7, Testing 4→5 |

## What Would Raise the Score Further

| Action | Score Gain | Effort |
|--------|------------|--------|
| Wire audit logging into tool execution | +0.5 | Small |
| Agent tool-result feedback loop | +1.0 | Medium |
| Incremental streaming (Ollama + NIM) | +0.5 | Small |
| Sandbox path tests | +0.5 | Small |
| Audit log tests | +0.5 | Small |
| OpenRouter real implementation | +1.0 | Medium |
| Config `set` subcommand | +0.5 | Small |
| Linux CI in GitHub Actions | +0.5 | Small |

**Target for Beta:** 7/10 (Ollama + NIM working, audit wired, feedback loop, tested).

## Alpha Criteria Met?

| Criterion | Status |
|-----------|--------|
| Compiles without errors | Yes |
| Tests pass | Yes |
| CLI --help works | Yes |
| Config loading works | Yes |
| Default config generation works | Yes |
| Provider abstraction in place | Yes |
| Tool abstraction in place | Yes |
| Sandboxing in place | Yes |
| Audit logger in place | Yes |
| Approval gate in place | Yes |
| No telemetry | Yes |
| No hardcoded secrets | Yes |
| Docs describe what works/stubbed | Yes |

**Alpha criteria:** Met — this is a valid alpha-ready repository.

## Beta Criteria (Not Yet Met at time of writing)

- A local LLM can complete a real task end-to-end (Ollama)
- A cloud LLM can complete a real task end-to-end (NIM)
- Tools execute and feed back into agent reasoning loop
- Audit log collects real entries from tool execution
- Provider error handling works (Ollama + NIM)
- At least 20 unit + integration tests
- CI runs on Windows and Linux
- Configuration can be modified via CLI
- Smoke test script passes

## Release Checklist (For Future Reference)

- `cargo check --workspace` — zero warnings
- `cargo test --workspace` — all pass
- `cargo clippy --workspace --all-targets` — no errors
- `cargo audit` — no critical vulnerabilities
- `cargo deny check` — licenses OK, no banned crates
- Architecture docs match code
- Status docs updated
- Changelog written
- All providers documented with setup guides
- Smoke test script passes
- README honest about limitations

## Verdict

**SC Node is at a valid private-alpha checkpoint with two working providers.** The NVIDIA NIM implementation validates the provider abstraction. The next sprint should focus on closing the agent loop (tool-result feedback) and wiring audit — those two changes make it a real agent rather than a chat wrapper.
