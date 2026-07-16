# SC Node — Threat Model

> **Status of this document:** describes the current codebase. SC Node is an
> experimental public alpha under active development. This is **not** a claim of
> completeness, and it is **not** a claim that SC Node is production ready,
> completely secure, or unbreakable — see
> [Explicitly Unmitigated / Known Gaps](#explicitly-unmitigated--known-gaps)
> below for what is honestly still missing. For the current per-feature
> maturity, see [docs/STATUS.md](docs/STATUS.md).

## 1. What SC Node Is

SC Node (`sc-agent`) is a local-first CLI agent runtime. It sends a task to
an LLM provider (local Ollama, or a cloud provider if explicitly enabled),
receives text and/or tool-call requests back, and executes a small set of
tools (read/write/list files, run shell commands) against the local
machine, subject to a configurable permission/workspace policy.

## 2. Assets

What this threat model is trying to protect:

| Asset | Description |
|-------|--------------|
| **User's filesystem** | Files inside (and, if misconfigured, outside) the configured workspace allowlist. |
| **Local shell environment** | The ability to execute arbitrary commands on the operator's machine. |
| **Secrets (API keys)** | `SC_AGENT_OPENROUTER_API_KEY`, `SC_AGENT_NVIDIA_API_KEY` — read from environment variables only. |
| **Audit log** | Local, append-only JSONL record of tool executions — an integrity/forensics asset, not a secrecy asset. |
| **Provider account/quota** | The operator's cloud provider account (OpenRouter, NVIDIA NIM), which can be drained by runaway or malicious use. |
| **Machine availability** | CPU/memory/disk of the host machine, which an unbounded shell command could exhaust. |

## 3. Actors

| Actor | Trust level |
|-------|-------------|
| **Operator** | The person running `sc-agent` on their own machine. **Trusted** — they configure the workspace allowlist, permission policy, and which providers are enabled. SC Node does not defend against a malicious operator on their own machine. |
| **LLM provider / model output** | **Untrusted**. See Trust Boundary 1 below — every tool-call request coming back from a model is treated as adversarial input, not as an authoritative instruction. |
| **Remote provider endpoint** | **Semi-trusted transport, untrusted content.** Connections use TLS via `reqwest` + `rustls`, but the response body (including any embedded tool-call requests) is untrusted model output. |
| **Anything reachable over the network from the shell tool or a future web tool** | **Untrusted.** No web-fetch tool ships in this branch (see §6); if/when one exists it must treat fetched content the same way. |

## 4. Trust Boundaries

1. **Model output is untrusted.** Text and tool-call requests returned by
   any provider (local or cloud) are attacker-controllable input to the
   rest of the system (via prompt injection, hallucination, or a
   compromised/malicious provider). They are never treated as
   pre-authorized instructions — every tool call must still pass the
   permission layer described in §5.
2. **Workspace boundary.** File tools (`read_file`, `write_file`,
   `list_dir`) resolve every path relative to the working directory,
   canonicalize it (resolving `..` and symlinks), and check the result
   against `workspace.allow` / `workspace.deny`. An empty allowlist denies
   all filesystem access. This boundary applies to file-tool path
   arguments; it does **not** currently apply to the shell tool's own
   argument paths (see §6).
3. **Network / data boundary.** SC Node makes zero outbound network calls
   unless a provider is explicitly enabled in config. Ollama defaults to
   `http://localhost:11434` (local-only). OpenRouter and NVIDIA NIM are
   `enabled = false` by default and require both an explicit config change
   and an environment-variable API key.
4. **Secrets boundary.** API keys are read from environment variables
   only (`SC_AGENT_OPENROUTER_API_KEY`, `SC_AGENT_NVIDIA_API_KEY`). They
   are never written into the config file and never included in audit log
   entries (`audit.log_args = false`, `audit.log_output = false` by
   default).
5. **Process boundary.** The shell tool spawns child processes with
   arguments passed as an argument vector (never interpolated into a
   shell string), so the process boundary is not a string-concatenation
   injection point. It is a resource boundary the code does **not**
   currently enforce (no CPU/memory/pid limits — see §6).

## 5. Threats and Current Mitigations

| # | Threat | Current mitigation | Residual risk |
|---|--------|---------------------|----------------|
| T1 | Malicious or hallucinated model output requests a destructive shell command (`rm -rf`, `shutdown`, pipe-to-shell, disk-format patterns, etc.). | Default shell deny-pattern list (case-insensitive substring match) blocks these; deny patterns always win over allow patterns and over policy. Covered by unit tests in `sc-tool-shell`. | Substring blocklist, not a command parser — see T5. |
| T2 | Model output requests writing/reading a file outside the workspace, or via `../` traversal / a symlink. | `sc-sandbox::resolve_and_check_path` canonicalizes the path (resolving `..` and symlinks) **before** checking it against the allowlist, so a symlink or traversal sequence cannot be used to land outside the allowlist. Covered by unit tests in `sc-sandbox`. | Correct as implemented for the file tools; does not cover the shell tool's own argument paths (see T6). |
| T3 | Model output requests reading/writing an obviously sensitive file (`*.key`, `*.pem`, `id_rsa*`, `*.secret`, `credentials*`) even inside an otherwise-allowed directory. | Default file-tool deny patterns match these regardless of allow patterns; deny always wins. | Deny patterns are name/extension based; a sensitive file under a different, non-matching name is not caught by this list. |
| T4 | A tool call is malformed, or its target path/command cannot be determined from its arguments (e.g. non-string array elements, missing `cmd`, non-object `args`). | `check_permission` fails closed to `Deny` whenever the target cannot be derived — it never falls back to `Allow`. Covered by unit tests (`sc-tool-core`, `sc-tool-shell`, `sc-tool-file`). | None known for the cases covered by the existing tests; new tool types must implement an extractor or they are denied by default (also fail-closed). |
| T5 | An allow-listed command is used with reordered/renamed flags to evade the deny-pattern blocklist (e.g. `rm -fr`, `rm -r -f`, `find -delete`). | None. Explicitly and deliberately documented and tested as a **known limitation** (`test_shell_known_limitation_flag_reordering_evades_denylist`), not a false sense of coverage. | **Unmitigated.** The deny list is a substring blocklist, not a real shell/command parser. |
| T6 | An allow-listed reader (`cat`, `grep`, `git`) is used with an argument path outside the workspace (e.g. `cat /etc/passwd` when `cat ` is allow-listed). | None. The sandbox validates the shell tool's *working directory*, not the *argument paths* of the command it runs. | **Unmitigated.** Workspace-boundary enforcement does not currently extend into shell command arguments. |
| T7 | An `ask`-policy tool call (the default for shell and file tools) is approved without any human ever seeing it. | The central gate resolves the decision before any tool I/O. In `repl` on a real TTY, an `Ask` decision triggers an interactive prompt (`[Approval Required] … y/N/a`). In `sc-agent run`, and in `repl` on a non-TTY stdin, an `Ask` decision is **auto-denied (fail-closed)** — no human is present, so the call is refused rather than allowed. `Deny` is always enforced. | **Partially mitigated.** Interactive review exists only in the REPL on a TTY; `run` never prompts (it fails closed). An `AutoAllow` mode exists in the type system but is not selected by any runtime path today. |
| T8 | Unbounded resource consumption by a spawned shell command (CPU, memory, disk, wall-clock). | A per-call `timeout_secs` argument (default 300s) bounds wall-clock time. | **Partially unmitigated.** No CPU, memory, or process-count limits; no Windows Job Object or Linux cgroup/ulimit containment. |
| T9 | Secret (API key) leakage via logs or audit trail. | Keys are read from env vars only, never written to the config file. Audit log excludes tool args/output by default. HTTP calls to cloud providers use TLS (`rustls`) with certificate verification. | If an operator sets `audit.log_args = true` for compliance, any secret an operator pastes into a task/tool argument could end up in the audit log — this is a documented trade-off, not a bug, but worth restating here. |
| T10 | Audit log tampering (an operator or malware truncates/edits the log to hide what happened). | Log is append-only at the file-open level (`OpenOptions::append`). | **Unmitigated.** There is no cryptographic hash chain or signature; the log is append-only by convention/API use, not tamper-evident. (An earlier internal doc comment in the audit crate calls the log "tamper-evident" — that description is aspirational and is corrected here: as implemented, it is append-only, not tamper-evident.) |
| T11 | SSRF or metadata-endpoint access via a network-capable tool. | **Not applicable in this branch.** No `web_fetch` (or any other network-capable) tool is registered or implemented here — the tool registry in `src/main.rs` only registers `read_file`, `write_file`, `list_dir`, and `shell`. A prior version of this repository's SECURITY.md described a `web_fetch` tool with an SSRF deny-list; that tool does not exist in this codebase today, and the claim has been removed. If/when a network tool is added, it must be designed against this same deny-list threat before shipping. |
| T12 | Supply-chain compromise of a dependency. | Dependency versions are pinned via `Cargo.lock`; the dependency graph is enumerable (see `docs/DEPENDENCY_INVENTORY.md`). | **Unmitigated by automation.** `cargo audit` / `cargo deny` are not installed in this environment and are not wired into CI in this repository (see docs/STATUS.md). No advisory-database check has been run as part of this work. |
| T13 | Cross-platform gaps (this branch's testing is Windows-only). | Workspace/permission logic is unit-tested and platform-independent in its logic (path canonicalization, pattern matching). | **Unmitigated.** No Linux/macOS execution has been verified as part of this work; see the capability matrix in docs/STATUS.md. |

## 6. Explicitly Unmitigated / Known Gaps

This section exists so nobody has to infer the gaps from the mitigation
table — they are restated plainly:

- **Interactive approval is REPL-only.** In `repl` on a real terminal an
  `ask`-policy decision prompts a human; in `sc-agent run` (and a non-TTY
  `repl`) an `ask` decision is auto-denied (fail-closed). Do not treat a `run`
  invocation as offering per-call human review.
- **The shell deny-list is a substring blocklist, not a command parser.**
  Flag reordering and alternate spellings can evade it by design of the
  current approach; this is called out in the test suite itself
  (`test_shell_known_limitation_flag_reordering_evades_denylist`).
- **Shell command arguments are not workspace-bounded.** Only the shell
  tool's working directory is sandboxed.
- **No resource limits** on spawned processes (CPU/memory/pid count) and
  no Windows Job Object / Linux cgroup containment.
- **No audit tamper-evidence** (no hash chain, no signing) — append-only
  only.
- **No network-capable tool exists in this branch** (no `web_fetch`), so
  there is nothing yet to threat-model there beyond "don't build one
  without SSRF protection."
- **Routing is deterministic and active** (this is no longer a gap): a single
  `Session::resolve_route` call selects the provider/model per task, cloud is
  gated behind an explicit opt-in, and there is no silent cloud fallback. Listed
  here only to correct an earlier version of this document that said routing was
  parsed but never applied.
- **No `cargo audit` / `cargo deny` automation** run as part of this
  repository's checks.
- **Not verified on Linux or macOS**; Windows is the only platform exercised so
  far.

## 7. Recommended Operator Practices (Today, Given the Above)

1. Keep the workspace allowlist as narrow as possible — assume the shell
   tool can read outside it via an allow-listed command's own arguments
   (T6), so do not rely on the allowlist alone for secrets you keep
   elsewhere on the machine.
2. Do not enable cloud providers, and do not set real API keys, unless you
   are prepared to review every tool call yourself — interactive approval is
   only available in the `repl` on a TTY, and `run` fails closed rather than
   prompting (T7).
3. Prefer `deny`-policy for any tool/pattern you are not willing to see
   executed automatically; do not rely on `ask` to mean "will pause for
   me."
4. Run SC Node under a limited, non-privileged local account, since there
   is no per-process resource containment yet (T8).
5. Treat the audit log as a best-effort activity trail, not as forensic
   proof against a determined local attacker (T10).

## 8. Reporting

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.
