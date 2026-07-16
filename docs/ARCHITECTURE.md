# SC Node — Architecture

This describes the crate layout and control flow of SC Node as it exists today.
For per-feature maturity see [STATUS.md](STATUS.md); for the security posture see
[SECURITY_MODEL.md](SECURITY_MODEL.md) and
[../THREAT_MODEL.md](../THREAT_MODEL.md).

## 1. Workspace shape

SC Node is a Cargo workspace (edition 2024) with one binary crate (`sc-agent`,
`src/main.rs`) and 15 library crates under `crates/`:

```
sc-agent (binary, src/main.rs)
├── sc-agent-core       — CLI parsing, agent/REPL loop, session, dispatch + permission gate
├── sc-config           — TOML config types, loading, env-var overrides
├── sc-message-types    — shared Message/StreamEvent/ToolCall/AuditEntry types
├── sc-provider-core    — Provider trait, OpenAI-compatible client + SSE decoder, routing module
├── sc-provider-ollama  — real Ollama HTTP client (batch-collect streaming)
├── sc-provider-nvidia  — real NVIDIA NIM client (via the shared OpenAI-compatible client)
├── sc-provider-openrouter — OpenRouter adapter (shared client; not yet live-tested)
├── sc-tool-core        — Tool trait, registry, check_permission, approval gate
├── sc-tool-file        — read_file / write_file / list_dir
├── sc-tool-shell       — shell command execution
├── sc-sandbox          — path canonicalization/allowlisting, sandboxed process spawn
├── sc-audit            — append-only JSONL audit logging
├── sc-contract         — execution contracts (TOML, fail-closed) + preflight
├── sc-proof            — proof bundles (SHA-256 hash chain over audit events) + secret redaction
└── sc-memory           — memory/RAG core (present, NOT wired into the binary)
```

Dependencies flow roughly top-to-bottom. No crate depends "upward" on
`sc-agent-core` or the binary. `sc-memory` is a member of the workspace and
builds and tests on its own, but the binary does not currently construct or use
it.

## 2. Control flow (`sc-agent run "<task>"` / `sc-agent repl`)

1. `src/main.rs` parses CLI args (`sc_agent_core::Cli`), loads `Config`
   (`sc-config`), constructs an `AuditLogger` (`sc-audit`), instantiates whichever
   providers are `enabled = true`, registers the four tools (`ReadFileTool`,
   `WriteFileTool`, `ListDirTool`, `ShellTool`) into a `ToolRegistry`, and builds a
   `Session`. The approval mode is chosen here: `repl` on a real TTY gets an
   interactive gate; every other invocation fails closed (`AutoDeny`).
2. `sc_agent_core::execute_task` resolves one provider/model for the task via the
   deterministic router (see §3), then loops: send the message history to the
   selected provider, receive `StreamEvent`s (text and/or tool-call requests).
3. Each requested tool call flows through the central gate `dispatch_tool_call`,
   which resolves the permission decision (`sc_tool_core::check_permission`)
   **before** any execution. `Deny` is refused; `Ask` is routed to the approval
   gate (prompt in the REPL, denied under `AutoDeny`); `Allow` proceeds.
4. File tools resolve their path argument through
   `sc_sandbox::resolve_and_check_path` (canonicalize, then check against
   `workspace.allow`/`workspace.deny`). The shell tool runs through
   `sc_sandbox::SandboxedCommand` (argument vector, enforced working directory,
   per-call timeout).
5. Every outcome (allowed, denied, error, unknown tool) is written through a
   single audit-emission point, honoring `audit.log_args`/`audit.log_output`
   redaction.
6. The tool result is appended back into the conversation and the loop repeats,
   bounded by a max-tool-rounds limit.

## 3. Routing

`sc-provider-core::routing` is a pure, I/O-free module resolving a
`ResolvedRoute` deterministically:

1. an explicit provider override (with optional model);
2. the first configured rule whose keywords match the task text;
3. the configured fallback route, if enabled;
4. the first enabled, credentialed, **local** provider;
5. otherwise a typed `NoRouteAvailable` error.

A cloud (non-local) provider is only ever selected when `allow_cloud` is true,
which `sc-agent-core` sets only when at least one cloud provider is
administratively enabled. Rules/fallbacks pointing at an unavailable provider are
dropped so routing falls through to local-first rather than hard-failing. There
is **no silent first-provider fallback**.

## 4. Provider abstraction

`sc-provider-core::Provider` is the async trait every provider implements:
`key`, `name`, `list_models`, `complete` (returns a stream of `StreamEvent`), and
`health_check`. Ollama talks to `http://localhost:11434` directly and currently
collects the full response body before emitting events. NVIDIA NIM and OpenRouter
share `openai_compat::OpenAiCompatClient`, which performs true incremental SSE
streaming, normalizes tool serialization and the `system` parameter, applies a
bounded retry policy, and redacts secrets from errors.

## 5. Permission / sandbox design

`check_permission` is a pure function returning `Allow`, `Ask(reason)`, or
`Deny(reason)` with no side effects. Evaluation order: a policy of `deny`
short-circuits first; then, if patterns are configured, the derived target (a
command or path) is checked against deny patterns (deny always wins) and then
allow patterns (at least one must match when any are configured); a target that
cannot be derived is denied (fail-closed). The decision is enforced centrally in
`dispatch_tool_call`, so audit logging and redaction cannot be skipped on any
branch.

## 6. Audit, contracts, and proof

- `sc-audit::AuditLogger` appends one JSON line per tool execution. It is
  **append-only, not tamper-evident** (no hash chain, no signature).
- `sc-contract::ExecutionContract` parses a strict, fail-closed TOML policy
  document (unknown fields rejected; absent security-critical fields default to
  the most restrictive value) and exposes a deterministic `policy_hash`. Reached
  via `sc-agent contract validate|explain`.
- `sc-proof::ProofBundle` records a task run and builds a SHA-256 hash chain over
  its audit events, with best-effort secret redaction. `sc-agent proof verify`
  re-derives the chain and checks the event count. The chain is tamper-evident
  for edits/reordering but not tamper-proof (trailing truncation needs the
  separately-recorded event count; a full re-hash is forgeable without an
  external signature).

## 7. Not yet wired

- `sc-memory` (memory/RAG) is present but not constructed by the binary.
- No network-capable tool ships (no `web_fetch`).
- No Windows-specific process containment (Job Objects) or resource limits.

See [STATUS.md](STATUS.md) for the full capability matrix.
