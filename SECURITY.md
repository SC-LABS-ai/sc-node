# Security Policy

> SC Node is an **experimental public alpha**. This file is not a claim that it
> is production-ready, completely secure, or fully sandboxed — it describes what
> exists today and how to report a problem. For the full control-layer and
> limitations analysis see [docs/SECURITY_MODEL.md](docs/SECURITY_MODEL.md); for
> the asset/trust-boundary/threat analysis see [THREAT_MODEL.md](THREAT_MODEL.md);
> for per-feature maturity see [docs/STATUS.md](docs/STATUS.md).

## Maturity

SC Node is in early development (experimental public alpha). The API may change.
Only Windows has been exercised so far. Do not point it at real credentials or an
unreviewed workspace before reading [docs/SECURITY_MODEL.md](docs/SECURITY_MODEL.md)
and [THREAT_MODEL.md](THREAT_MODEL.md).

## Reporting a vulnerability (responsible disclosure)

Please report security vulnerabilities **privately** to **contact@sclabs.uk**
rather than opening a public issue, pull request, or discussion. Include enough
detail to reproduce the issue. We will acknowledge your report and work with you
on a fix and coordinated disclosure. Security patches are applied to the latest
development version; there is no long-term-support release yet.

## What exists today

- **No telemetry.** SC Node collects and transmits nothing. There are no outbound
  network calls unless you explicitly enable a provider.
- **Workspace path boundaries.** File tools canonicalize their path argument and
  check it against a configurable allow/deny list before any filesystem access;
  an empty allowlist denies everything.
- **Shell isolation.** The shell tool passes arguments as a vector (no shell-string
  interpolation), enforces the working directory, applies a per-call timeout, and
  applies a default deny-pattern list for obviously destructive commands.
- **Permission engine + approval gate.** Per-tool `allow`/`ask`/`deny` policies
  with fail-closed evaluation, enforced centrally before any tool I/O.
- **Audit logging.** Every tool execution is recorded in a local, append-only
  JSONL log.
- **Contracts and proof.** Optional fail-closed execution contracts and proof
  bundles (audit hash chain + secret redaction).

## Credential handling

API keys are read from environment variables only (`SC_AGENT_NVIDIA_API_KEY`,
`SC_AGENT_OPENROUTER_API_KEY`). The config key field is `#[serde(skip)]`, so a key
placed in the TOML is ignored on load and is never written back by `config-show`.
Keys are never included in audit entries and are redacted from provider error
messages. The shared cloud client refuses to attach a key to a non-`https`,
non-local base URL.

## No silent cloud fallback

Cloud providers are disabled by default. Routing is local-first, and a cloud
provider is only ever selected via an explicit rule, fallback, or override once a
cloud provider has been enabled (the cloud opt-in). SC Node never silently sends a
task to a cloud endpoint.

## Known limitations (read before relying on this)

- **Interactive approval is REPL-only.** In `repl` on a real terminal, an `ask`
  decision prompts for approval; `sc-agent run` (and a non-TTY `repl`) auto-deny
  any tool call that requires approval (fail-closed). Do not treat `run` as
  offering per-call human review.
- **Sandbox check-then-open (TOCTOU) window.** A reparse point swapped in between
  the path check and the actual open is not re-detected.
- **Shell deny-list is a substring blocklist, not a command parser** — flag
  reordering can evade it.
- **Shell command arguments are not workspace-bounded** — only the working
  directory is checked.
- **Audit log is not cryptographically tamper-evident** — append-only by
  file-open mode; no hash chain or signature over the live log.
- **No per-process resource limits**; no Windows Job Object / Linux cgroup
  containment.
- **No guarantee of complete isolation.** SC Node does not sandbox itself at the
  OS level and does not defend against a malicious operator on their own machine.
  For stronger isolation, run under a low-privilege account and/or in a container
  or VM.
- **No `web_fetch`/network tool ships** (an earlier version of this file described
  one that never existed; that claim was removed). The default `web` deny patterns
  (including the `169.254.169.254` metadata address) are provisioned for a future
  tool, not an active mitigation.

## Supply chain

Dependency versions are pinned via `Cargo.lock`; see
[docs/DEPENDENCY_INVENTORY.md](docs/DEPENDENCY_INVENTORY.md) for the resolved graph
with per-package license. `cargo audit` and `cargo deny` are **not** installed or
run in CI in this repository — a tracked gap, with the exact commands documented
in [docs/DEPENDENCY_INVENTORY.md §6](docs/DEPENDENCY_INVENTORY.md#6-vulnerability-scanning-not-run).

## Operator recommendations

1. Keep the workspace allowlist as narrow as possible.
2. Prefer `deny` policies for anything you are not willing to see run
   automatically.
3. Run SC Node under a dedicated low-privilege account, ideally inside a container
   or VM.
4. Treat the audit log as a best-effort activity trail, not tamper-proof evidence.
