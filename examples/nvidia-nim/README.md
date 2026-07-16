# NVIDIA NIM Example

Run `sc-agent` against NVIDIA's hosted, OpenAI-compatible NIM API using
your own API key. This is an opt-in cloud provider — disabled by default.

## Prerequisites

- An NVIDIA API key from [build.nvidia.com](https://build.nvidia.com)
- A config already created (`sc-agent init`)

## 1. Set the API key for this session only

PowerShell:

```powershell
$env:SC_AGENT_NVIDIA_API_KEY = "<your key>"
```

SC Node reads this key **only** from the `SC_AGENT_NVIDIA_API_KEY`
environment variable. Never place it in `config.toml` — it is not read
from there, and it is redacted before it can appear in any error message
or log line, even if a server response happens to echo it back.

## 2. Enable the provider in config

Edit `~/.sc-agent/config.toml` (schema matches
[`../config.example.toml`](../config.example.toml)):

```toml
[providers.nvidia]
enabled = true
base_url = "https://integrate.api.nvidia.com/v1"
default_model = "meta/llama-3.3-70b-instruct"   # pick a model you have access to
timeout_secs = 60
max_retries = 3
```

## 3. List models and run a task

In the same shell session (so the env var is still set):

```powershell
sc-agent models-list
sc-agent run "Summarize what this repository does in three sentences"
```

## Expected output shape

```
[Route] provider=nvidia model=meta/llama-3.3-70b-instruct (<routing reason>)
<streamed model text appears here as it arrives>
[Done: stop]
```

## 4. Remove the key when done

```powershell
Remove-Item Env:SC_AGENT_NVIDIA_API_KEY
```

## Security notes

- HTTPS is enforced for this provider: SC Node refuses to attach the API
  key to a non-`https://` base URL unless that URL points at a
  local/loopback host. There is no silent fallback to plaintext HTTP.
- The key never appears in `sc-agent config-show` output, in the audit
  log, or in printed error text — it is redacted at the source.
- `providers.nvidia.enabled` defaults to `false`; you must opt in
  explicitly in config in addition to setting the environment variable.
