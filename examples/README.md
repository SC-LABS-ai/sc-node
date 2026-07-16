# Examples

Three worked examples for `sc-agent`, each a self-contained walkthrough
rather than a `.rs` sample — the CLI is the public surface today, so a
README with exact commands and config snippets is the accurate way to
show usage. See [`config.example.toml`](config.example.toml) for the full,
annotated config schema referenced by all three.

- [`ollama/`](ollama/README.md) — run entirely locally against Ollama, no
  cloud calls. Start here.
- [`nvidia-nim/`](nvidia-nim/README.md) — opt-in cloud provider using your
  own NVIDIA API key, key supplied via environment variable only.
- [`tool-agent/`](tool-agent/README.md) — a task that exercises the
  built-in file/shell tools, and how the permission/approval gate decides
  whether a tool call runs.

## Before you start

```powershell
sc-agent init          # creates ~/.sc-agent/config.toml
sc-agent config-show   # see what was loaded
sc-agent doctor        # check provider connectivity + config
```

Every example assumes a config file already exists at
`~/.sc-agent/config.toml` (or at the path pointed to by `SC_AGENT_CONFIG`,
if you've set that). Copy [`config.example.toml`](config.example.toml)
over it, or edit the file `sc-agent init` created, and adjust the
`[providers.*]` block for the example you're following.
