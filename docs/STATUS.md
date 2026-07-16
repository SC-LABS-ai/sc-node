# SC Node — Status

> **As of:** 2026-07-16 · **Version:** 0.1.0 · **Maturity:** experimental public alpha

This is the authoritative status page for SC Node. It is written from direct
code inspection, not from prior doc claims; where an older document disagrees
with this page or with the code, the code wins.

SC Node is an experimental public alpha. Nothing here is production-ready, and no
claim on this page should be read as "secure" in an absolute sense — see
[SECURITY_MODEL.md](SECURITY_MODEL.md) and [../THREAT_MODEL.md](../THREAT_MODEL.md)
for the honest gaps.

## Platform & providers

- **Windows tested.** Linux and macOS are unverified.
- **Ollama live-tested** (local, enabled by default).
- **NVIDIA NIM live-tested** (cloud, opt-in, key via `SC_AGENT_NVIDIA_API_KEY`).
- OpenRouter adapter is implemented on the shared OpenAI-compatible client but is
  **not live-tested**.

## Capability matrix

Legend: **works** = implemented and covered by the automated test suite and/or
live-tested · **partial** = implemented with a documented gap · **stub** =
present but returns a placeholder · **not wired** = crate exists but the binary
does not use it.

| Capability | State | Notes |
|------------|-------|-------|
| CLI bootstrap (`--help`, `--version`, `init`, `config-show`) | works | Offline; no config needed for `--help`/`--version`/`init` |
| Agent tool-result feedback loop | works | Bounded max-rounds; deterministic unit tests |
| Deterministic provider/model routing | works | 5-step resolution, cloud gated, no silent fallback; prints a `[Route]` line |
| Ollama provider (list/complete/health) | works | Real HTTP; streaming is batch-collect (see below) |
| NVIDIA NIM provider (list/complete/health) | works | Shared OpenAI-compatible client; incremental SSE; live-tested |
| OpenRouter provider | partial | Real adapter on the shared client; **not live-tested** |
| Incremental streaming (cloud) | works | SSE decoded as bytes arrive for OpenAI-compatible providers |
| Incremental streaming (Ollama) | partial | Collects the full body, then emits events |
| Workspace path sandboxing | works | Canonicalize + allow/deny; symlink/junction, UNC, device-name, ADS handling |
| Tool permission engine (`check_permission`) | works | Fail-closed; deny wins; family fallback |
| Central permission gate in the run loop | works | Decision resolved before any tool I/O |
| Interactive approval gate | partial | Prompts in `repl` on a TTY; `run` and non-TTY `repl` auto-deny (fail-closed) |
| Audit logging | partial | Append-only JSONL wired into every tool call; **not tamper-evident** |
| Execution contracts (`contract validate`/`explain`) | works | Strict, fail-closed TOML; deterministic `policy_hash` |
| Proof bundles (`proof verify`) | works | SHA-256 hash chain over audit events; secret redaction; not tamper-proof |
| `config-set` subcommand | stub | Prints "not yet implemented" |
| `workspace-add` subcommand | stub | Prints the manual config edit to make |
| Memory / RAG (`sc-memory`) | not wired | Crate builds/tests; the binary does not construct it |
| Windows process hardening (Job Objects, resource limits) | not implemented | No Windows-specific process code |
| Network-capable tool (e.g. `web_fetch`) | not implemented | Only `read_file`/`write_file`/`list_dir`/`shell` are registered |
| `cargo audit` / `cargo deny` in CI | not implemented | Tracked gap; commands documented in DEPENDENCY_INVENTORY.md §6 |

## CLI surface

| Command | Status |
|---------|--------|
| `sc-agent run "<task>"` | Works end-to-end against a reachable/enabled provider |
| `sc-agent repl` | Works; interactive approval prompts on a TTY |
| `sc-agent init` (alias `config-init`) | Works |
| `sc-agent config-show` | Works |
| `sc-agent config-set <k> <v>` | Stub |
| `sc-agent providers-list` | Works |
| `sc-agent models-list` | Works for enabled/reachable providers |
| `sc-agent audit-show [--last N]` | Works (default 50) |
| `sc-agent workspace-add <path>` | Stub |
| `sc-agent doctor` | Works (provider health, config, tools, workspace) |
| `sc-agent contract validate\|explain <path>` | Works |
| `sc-agent proof verify <path>` | Works |

## Known gaps (summary)

1. Interactive approval prompts are only available in `repl` on an interactive
   terminal; `run` auto-denies tools that require approval (fail-closed).
2. Shell deny-list is a substring blocklist, evadable by flag reordering.
3. Shell tool argument paths are not workspace-bounded.
4. No per-process resource limits; no Windows Job Objects / Linux cgroups.
5. Audit log is append-only, not cryptographically tamper-evident.
6. Sandbox has a check-then-open (TOCTOU) window.
7. `sc-memory` is present but not wired into the runtime.
8. Ollama streaming is batch-collect (cloud streaming is incremental).
9. Only verified on Windows; Linux/macOS and the OpenRouter adapter unverified.
10. `cargo audit` / `cargo deny` not wired into CI.

See [../THREAT_MODEL.md](../THREAT_MODEL.md) for detail on the security-relevant
items and [ROADMAP.md](ROADMAP.md) for what comes next.

## Testing

`cargo test --workspace` runs an offline, deterministic inline unit-test suite
across the crates (routing, permissions, sandbox, providers, contracts, proof,
audit, config). Live provider calls only run when a real API key is present.
Windows helpers: `scripts/smoke-check.ps1`, `scripts/verify-local.ps1`,
`scripts/verify-public-beta.ps1`.

## Dependencies

See [DEPENDENCY_INVENTORY.md](DEPENDENCY_INVENTORY.md) for the resolved
dependency graph with per-package license, and
[../THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) for license notices.
