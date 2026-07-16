# SC Node — Security Model

> **As of:** 2026-07-16 · Experimental public alpha.
>
> This document describes the security controls that exist today and, just as
> importantly, the ones that do **not**. It is not a claim that SC Node is
> secure, sandboxed, or production-ready. For the full asset/trust-boundary
> analysis see [../THREAT_MODEL.md](../THREAT_MODEL.md); for how to report a
> vulnerability see [../SECURITY.md](../SECURITY.md).

The core trust assumption: **model output is untrusted.** Every tool-call request
returned by any model (local or cloud) is treated as adversarial input and must
pass the permission layer before it can do anything.

## Control layers

### Workspace path boundaries (`sc-sandbox`)

File tools resolve their path argument relative to the working directory,
syntactically normalize it (resolving `.`/`..` without touching disk), fold
Windows prefix forms (drive letter, UNC, extended-length `\\?\`), and check it
against `workspace.allow` / `workspace.deny` before any filesystem operation. An
empty allowlist denies all filesystem access. As defense in depth against
reparse-point escapes, the nearest existing ancestor is canonicalized (following
symlinks/junctions) and re-checked. Device paths (`\\.\…`), NTFS alternate data
streams (`name:stream`), reserved device names (`CON`, `NUL`, `COM1`, …), and
trailing-dot/space name tricks are rejected.

### Permissions and the approval gate (`sc-tool-core`, `sc-agent-core`)

`check_permission` is a pure, fail-closed function returning `Allow`,
`Ask(reason)`, or `Deny(reason)`. A `deny` policy short-circuits before any
pattern is evaluated; deny patterns always win over allow patterns; a non-empty
allow list with no match denies; a target that cannot be derived from the
arguments is denied. The decision is enforced centrally in `dispatch_tool_call`
**before** any tool I/O.

Approval handling:

- In `repl` on a real interactive terminal, an `Ask` decision triggers an
  interactive prompt (`[Approval Required] … y/N/a`, where `a` allows all for the
  session).
- In `sc-agent run`, and in `repl` fed from a piped/non-TTY stdin, an `Ask`
  decision is **auto-denied (fail-closed)** — no human is present to approve, so
  the call is refused rather than silently allowed.

### Shell isolation (`sc-sandbox`)

The shell tool spawns child processes with arguments passed as an argument vector
(never interpolated into a shell string), an enforced working directory that must
be inside the workspace, `stdin` set to null, and a per-call timeout. A default
deny-pattern list blocks obviously destructive commands (`rm -rf`, `sudo`,
`chmod 777`, pipe-to-shell, `dd if=`, `mkfs`, `format`, `shutdown`, `reboot`, …).

### Audit (`sc-audit`)

Every tool execution is written to a local, append-only JSONL log with policy,
decision, timing, and optional (default-off) redaction of args/output. Provider
failures are audited without capturing the task prompt or the API key.

### Contracts (`sc-contract`)

An execution contract is a strict, fail-closed TOML policy document: unknown
fields are rejected, and every absent security-critical field defaults to the
most restrictive value (network `deny`, provider `local_only`, empty model
allowlist, commit/push `never`, approvals `all`, data boundary `local_only`). It
exposes a deterministic `policy_hash`. Available via
`sc-agent contract validate|explain`.

### Proof (`sc-proof`)

A proof bundle records a task run and builds a SHA-256 hash chain over its audit
events, with best-effort secret redaction (sensitive key names and secret-shaped
token values are replaced with `[REDACTED]`). `sc-agent proof verify`
independently re-derives the chain and checks the recorded event count.

### Credential handling

API keys are read from environment variables only
(`SC_AGENT_NVIDIA_API_KEY`, `SC_AGENT_OPENROUTER_API_KEY`). The config key field
is `#[serde(skip)]`, so a key placed in the TOML is ignored on load and never
written back by `config-show`. Keys are never included in audit entries and are
redacted from provider error messages.

### Network / data boundary

SC Node makes zero outbound calls unless a provider is enabled. Cloud providers
are off by default; there is no silent cloud fallback (local-first routing, cloud
only via an explicit rule/fallback/override behind the cloud opt-in).

### HTTPS enforcement

The shared OpenAI-compatible client refuses to attach a credential to a
non-`https` base URL unless it points at a local/loopback host
(`localhost`/`127.0.0.1`/`::1`). Cloud HTTP uses `rustls` with certificate
verification.

### SSRF / metadata-endpoint guard (provisioned, not active)

The default config ships deny patterns for a `web` tool that block
`localhost`, `127.0.0.1`, and the cloud metadata address `169.254.169.254`.
**However, no network-capable tool (`web_fetch` or similar) is registered in the
binary today**, so this is a pre-provisioned guard for a tool that does not yet
ship, not an active mitigation. Any future network tool must be designed against
this SSRF/metadata threat before shipping.

## Limitations (blunt)

- **Interactive approval is REPL-only.** `sc-agent run` auto-denies any tool call
  that requires approval; only the REPL on a TTY prompts a human. Do not treat a
  `run` invocation as offering per-call human review.
- **Check-then-open (TOCTOU) window.** The sandbox canonicalizes the nearest
  existing ancestor at check time; a reparse point swapped in between the check
  and the actual open is not re-detected. Closing this requires OS-level
  no-follow opens, which are not implemented.
- **Audit log is not tamper-evident.** It is append-only by file-open mode only —
  no hash chain, no signature. A local actor with filesystem access can truncate
  or edit it without automatic detection. (The `sc-proof` hash chain is a separate
  facility and is not automatically applied to the live audit log.)
- **Shell deny-list is a substring blocklist, not a command parser.** Flag
  reordering and alternate spellings (`rm -fr`, `rm -r -f`, `find -delete`) can
  evade it by design of the current approach.
- **Shell command arguments are not workspace-bounded.** Only the shell tool's
  working directory is checked, so an allow-listed reader (`cat`, `grep`) can read
  files outside the workspace via its own argument paths.
- **No resource containment.** No CPU/memory/pid limits; no Windows Job Object or
  Linux cgroup around spawned processes.
- **No guarantee of complete isolation.** SC Node does not sandbox itself at the
  OS level and does not defend against a malicious operator on their own machine.
  For stronger isolation, run it under a low-privilege account and/or inside a
  container or VM.
- **Windows-only verification.** Path/permission logic is platform-independent in
  its unit tests, but only Windows execution has been exercised.
- **No `cargo audit` / `cargo deny` automation** in CI (tracked gap).

## Operator recommendations

1. Keep the workspace allowlist as narrow as possible; do not rely on it alone
   for secrets, given the shell-argument gap.
2. Prefer `deny` policies for anything you are not willing to see executed
   automatically; do not rely on `ask` under `run` (it fails closed, but the REPL
   is where a human actually reviews).
3. Do not point SC Node at real credentials or an unreviewed workspace until you
   have read this document and [../THREAT_MODEL.md](../THREAT_MODEL.md).
4. Run under a low-privilege account, ideally inside a container or VM.
