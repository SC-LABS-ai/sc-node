# Tool-Using Agent Walkthrough

This example demonstrates the complete SC Node tool path on Windows:

1. a model requests file and shell tools,
2. SC Node evaluates the configured policy,
3. an interactive REPL asks for approval,
4. the workspace sandbox still enforces path boundaries,
5. every allowed, denied, or failed call is written to the audit log.

The walkthrough uses an isolated directory under `%TEMP%`. It does not modify your normal `~/.sc-agent` profile.

## Prerequisites

- Windows PowerShell 5.1 or PowerShell 7
- a release build of SC Node
- Ollama running locally
- an installed Ollama model that supports tool calls

Build SC Node first:

```powershell
cargo build --release --locked
```

## 1. Prepare the isolated demo

From the repository root:

```powershell
.\examples\tool-agent\prepare-demo.ps1 -Model "llama3.2:3b"
```

Use a different model name when needed. The script creates:

```text
%TEMP%\sc-node-tool-agent-demo\
├── config.toml
├── data\
└── workspace\
    ├── notes.txt
    └── blocked.secret
```

It prints the exact commands for the current PowerShell session. They are equivalent to:

```powershell
$env:SC_AGENT_CONFIG = Join-Path $env:TEMP "sc-node-tool-agent-demo\config.toml"
$ScAgent = ".\target\release\sc-agent.exe"
& $ScAgent doctor
```

Expected structural output from `doctor`:

```text
SC Node Health Check
Config: OK
Providers (1):
  Ollama (ollama): HEALTHY
Tools (4):
  - read_file
  - write_file
  - list_dir
  - shell
Workspace (1):
Audit: ENABLED
```

Tool ordering may differ. Stop here when Ollama is not healthy or the configured model is not installed.

## 2. Multi-round file and shell task

Start an interactive terminal session:

```powershell
& $ScAgent repl
```

Paste this task, replacing `<WORKSPACE>` with the workspace path printed by `prepare-demo.ps1`:

```text
Use tools to complete every step. List <WORKSPACE>, read notes.txt, write a concise three-bullet summary to <WORKSPACE>/summary.md, then use the shell tool with PowerShell Get-FileHash to calculate the SHA-256 hash of summary.md. Return the summary and hash only after all tool results are available.
```

The exact model wording is not deterministic. The following SC Node markers are stable and should appear across multiple rounds:

```text
[Route] ...
[Tool Call] list_dir: ...
[Approval Required]
Tool: list_dir
Policy: ask
Reason: Tool 'list_dir' requires approval
Allow? [y/N/a] (a = allow all for this session):
```

Choose `y` to approve only the current call, `a` to approve this and all later `ask` decisions in the current session, or Enter/`n` to deny the call.

A successful run should create:

```text
%TEMP%\sc-node-tool-agent-demo\workspace\summary.md
```

The final answer should contain the three-bullet summary and a 64-character SHA-256 value. Treat the generated prose as model output; verify the file and hash independently when they matter.

## 3. Approval-gate behavior

The approval prompt is available only in `repl` attached to a real terminal. Single-shot mode deliberately fails closed:

```powershell
& $ScAgent run "Read the notes.txt file in the configured workspace"
```

Because the demo file policy is `ask`, `run` has no interactive approver and the tool call is denied. The relevant result should contain a denied decision rather than silently executing the read.

```text
repl + real TTY     -> ask the user
run                 -> auto-deny ask decisions
repl + piped stdin  -> auto-deny ask decisions
```

## 4. Denied sensitive file

In the REPL, ask:

```text
Read blocked.secret from the configured workspace and print its contents.
```

The fixture is harmless, but the policy contains `*.secret` in both the file deny patterns and the workspace deny list. Expected outcome:

```text
[Tool Call] read_file: ...blocked.secret...
... denied ...
```

No approval can override a deny-pattern match.

## 5. Workspace boundary violation

In the REPL, request a file outside the printed demo workspace, for example:

```text
Read C:/Windows/System32/drivers/etc/hosts and print it.
```

Expected outcome:

```text
[Tool Call] read_file: ...
... path not allowed ...
```

Approval does not bypass the workspace sandbox. The allowed root remains the isolated demo workspace.

## 6. Inspect the audit trail

After the allowed and denied attempts:

```powershell
& $ScAgent audit-show --last 20
```

The demo enables argument and output logging only because it uses disposable fixture data. Each JSON line should include fields such as:

```json
{
  "tool": "read_file",
  "policy": "ask",
  "decision": "allowed",
  "exit_code": 0,
  "duration_ms": 1
}
```

Look for `allowed` for an approved call, `denied` for a policy or approval rejection, and `error` for an execution failure after permission was granted.

The relative audit path in the demo config resolves under `general.data_dir`, so the file is written to:

```text
%TEMP%\sc-node-tool-agent-demo\data\audit.jsonl
```

## 7. Deterministic guard verification

Model behavior varies, so the repository also provides deterministic checks for the security-relevant failure paths:

```powershell
.\examples\tool-agent\verify-guards.ps1
```

Expected final line:

```text
RESULT: 4 deterministic guard checks passed.
```

The script verifies that a denied shell call never executes, `ask` fails closed without an interactive approver, workspace traversal is blocked, and sensitive file patterns are blocked.

## Security notes

- The demo grants access only to its temporary workspace.
- Shell commands are passed as an argument vector, not interpolated into a shell string by SC Node.
- Deny patterns take precedence over allow patterns.
- The shell deny list is a defensive substring blocklist, not a complete command parser.
- Audit argument/output logging can capture sensitive content. Keep both disabled in normal profiles unless explicitly required.
