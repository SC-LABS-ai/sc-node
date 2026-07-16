# Tool-Agent Example

A task where the model calls `sc-agent`'s built-in file and shell tools,
and how the permission/approval gate decides whether a given tool call is
allowed to run.

## Registered tools

From `crates/sc-tool-file` and `crates/sc-tool-shell`, registered in that
order in `src/main.rs`:

- `read_file`, `write_file`, `list_dir` — path-sandboxed via `sc-sandbox`
- `shell` — runs `cmd`/args as a vector (never interpolated into a shell
  string), also sandboxed via `sc-sandbox`, with a per-call timeout

## How dispatch works

1. The model requests a tool call; it is printed as
   `[Tool Call] <name>: {args}`.
2. The central gate in `sc-agent-core` resolves a decision from
   `[permissions]` in config: `allow`, `ask`, or `deny` (pattern match
   first, falling back to `default_policy`). `deny` always wins over a
   matching `allow` pattern.
3. `allow` executes immediately. `deny` blocks unconditionally.
   `ask` is resolved against how you invoked `sc-agent`:
   - `sc-agent repl`, run from a real interactive terminal, shows an
     `[Approval Required]` prompt (`y`/`N`/`a` — `a` allows every
     remaining `ask` for the session) before the tool executes.
   - `sc-agent run "<task>"` (single-shot, non-interactive), and `repl`
     fed via piped/redirected stdin, auto-deny every `ask` decision — no
     human is present to approve, so SC Node fails closed rather than
     silently allowing.
4. Even an approved call still goes through the workspace/path sandbox in
   `sc-sandbox` (see below) — approval does not bypass it.
5. Every outcome (allowed, denied, or errored) is written to the audit
   log.

## Prerequisites

- `sc-agent init` already run
- At least one provider enabled (see the `ollama` or `nvidia-nim`
  examples)

## Workspace boundary

Tools can only touch paths under `[workspace] allow` in config (deny
patterns always win). Point it at a scratch folder you're comfortable
with the agent read/writing:

```toml
[workspace]
allow = ["C:/Users/you/sc-agent-scratch"]
deny = ["**/.git/**", "**/.env*"]
```

## Run it

```powershell
sc-agent run "List the files in C:/Users/you/sc-agent-scratch and read one of them"
```

## Audit trail

```powershell
sc-agent audit-show --last 5
```

Each line is one JSON object: `timestamp`, `session_id`, `tool`, `args`
(present only if `audit.log_args = true`), `policy`, `decision`
(`allowed`/`denied`/`error`), `exit_code`, `duration_ms`, `output`
(present only if `audit.log_output = true`).

## Security notes

- Shell args are passed as a vector; nothing is ever interpolated into a
  shell string.
- File-path resolution rejects Windows reserved device names (`CON`,
  `NUL`, ...) and alternate-data-stream syntax (`file:$DATA`) even when
  the workspace allow-list would otherwise match.
- `deny_patterns` for both the `file` and `shell` tool policies always
  take precedence over `allow_patterns` on overlap.
