# SC Node benchmark helper

`sc-benchmark` is a non-published workspace tool used by
[`scripts/benchmark-overhead.ps1`](../../scripts/benchmark-overhead.ps1). It is
not part of the SC Node runtime or release package.

## Commands

```text
direct-ollama      Call an Ollama-compatible endpoint directly.
node-ollama        Run the same request through SC Node routing and agent setup.
unload-ollama      Unload a real Ollama model before a cold run.
micro              Measure routing, permission, audit, and proof primitives.
synthetic-agent    Measure no-model agent loops and no-op tool rounds.
serve-mock-ollama  Serve a deterministic local compatibility fixture.
```

The direct and SC Node provider paths use the same model, prompt, endpoint,
streaming request, temperature, output limit, and keep-alive value. The mock
endpoint returns an immediate fixed response so provider/model latency does not
hide the runtime path under test.

## Safety and scope

- The mock endpoint binds to loopback by default.
- No API key is read or printed.
- The synthetic tool is a no-op and its execution count is asserted.
- Benchmark output belongs under `artifacts/` or an explicitly selected output
  directory.
- The helper is `publish = false` and should not be shipped as the end-user CLI.
