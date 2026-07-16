# Ollama Example

Run `sc-agent` entirely on your own machine against a local
[Ollama](https://ollama.com) server — no cloud provider involved.

## Prerequisites

- Ollama installed and running (default: `http://localhost:11434`)
- At least one model already pulled (`ollama pull <model>`)

## 1. Verify Ollama is up and see what you have

```powershell
ollama list
```

Note one of the model names printed here — SC Node does not assume a
default model exists on your machine, so you will set it explicitly
below.

## 2. Configure the provider

If you haven't already, create a config: `sc-agent init`. Then edit
`~/.sc-agent/config.toml` and set `default_model` to a model name from
step 1 (this snippet matches the schema in
[`../config.example.toml`](../config.example.toml)):

```toml
[providers.ollama]
enabled = true
base_url = "http://localhost:11434"
default_model = "llama3.2:3b"   # replace with a model you actually pulled
keep_alive = "5m"
timeout_secs = 120
max_retries = 2
```

## 3. Confirm SC Node sees your models

```powershell
sc-agent models-list
```

This queries the Ollama provider directly; the model you set above should
appear in its list.

## 4. Run a task

```powershell
sc-agent run "List the Rust files in the current directory"
```

Or interactively:

```powershell
sc-agent repl
```

## Expected output shape

```
[Route] provider=ollama model=llama3.2:3b (<routing reason>)
<streamed model text appears here as it arrives>
[Tool Call] list_dir: {"path": "."}     <- only if the model calls a tool
[Done: stop]
```

Check what actually ran:

```powershell
sc-agent audit-show --last 5
```

## Security notes

- Ollama's default endpoint (`http://localhost:11434`) is loopback-only —
  nothing leaves your machine for this provider.
- No API key is read or required for Ollama.
- `general.no_telemetry` must stay `true`; SC Node makes zero outbound
  network calls unless you explicitly enable a cloud provider elsewhere
  in config.
