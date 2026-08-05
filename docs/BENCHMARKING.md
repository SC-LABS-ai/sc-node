# SC Node — Benchmarking

> **As of:** 2026-08-05 · Experimental public alpha.

## State

SC Node is designed as a thin Rust execution layer, but design intent is not a
performance result. Performance claims are accepted only when the raw data,
commit, environment, model, endpoint, commands, and iteration policy are
published together.

The repository now includes a reproducible benchmark harness. A first reference
run is stored under [`benchmarks/`](benchmarks/) when available. Results from one
machine and model are evidence for that configuration only; they are not a
universal Rust-vs-Python or SC-Node-vs-framework claim.

## Run the complete benchmark

From the repository root on Windows PowerShell or PowerShell 7:

```powershell
./scripts/benchmark-overhead.ps1 `
  -Model "qwen:latest" `
  -FixtureIterations 20 `
  -WarmIterations 8 `
  -ColdIterations 3 `
  -SyntheticIterations 100 `
  -MicroIterations 100000 `
  -MicroIoIterations 1000
```

The helper is built with:

```text
cargo build --release --locked -p sc-benchmark
```

An explicit `-OutputDirectory` may point outside the repository. The script
holds an exclusive lock on that directory so duplicate measurement processes
cannot corrupt the same run.

## Measurement layers

### 1. Deterministic Ollama-compatible fixture

A loopback-only fixture server returns an immediate fixed Ollama NDJSON response.
The harness alternates call order and compares:

```text
direct HTTP request
versus
SC Node route -> session -> Ollama adapter -> stream parse -> output
```

Because no model runs, this is the primary process-level estimate of SC Node's
incremental provider path on the measured machine. It still includes process
startup and console formatting for both paths.

### 2. Real local Ollama model

The same prompt, model, endpoint, streaming flag, temperature, token limit, and
keep-alive setting are sent directly and through SC Node.

- **Warm:** the model is preloaded and direct/node order alternates each pair.
- **Cold:** the model is explicitly unloaded before every measured process.

Real-model differences are reported as *observed differences*, not pure runtime
overhead, because inference, model loading, scheduling, and the Ollama server are
variable and usually dominate the small wrapper cost.

### 3. Deterministic microbenchmarks

The helper reports nanoseconds per operation for:

- deterministic route resolution;
- permission/pattern evaluation;
- disabled audit logging;
- enabled append-and-flush audit logging;
- building a 100-event SHA-256 proof chain;
- verifying a 100-event proof bundle.

### 4. Synthetic agent and tool rounds

A no-network fake provider measures:

- an agent round without tools;
- a round with exactly one no-op tool dispatch;
- the same tool round with audit logging enabled.

The no-op tool increments an atomic counter. The command fails if observed tool
executions differ from the requested iteration count, preventing duplicate or
missing dispatches from silently entering the results.

### 5. CPU and process memory

For every process-level run, the harness records:

- wall-clock duration;
- process CPU time;
- peak working set;
- exit code;
- output sizes;
- the helper's internal elapsed marker where applicable.

Peak working set covers only the measured benchmark/SC Node process. The
external Ollama server and model memory are deliberately excluded and this scope
is repeated in every generated report.

## Raw output

Each run contains:

```text
environment.json
micro.jsonl
raw-process-measurements.csv
summary.json
REPORT.md
fixture-server.stdout.txt
fixture-server.stderr.txt
<one stdout/stderr pair per measured process>
```

`environment.json` records the SC Node commit, dirty-worktree state, Rust/Cargo
versions, OS, architecture, logical processor count, machine name, CPU, RAM,
model, prompt, endpoint, and all iteration settings.

## Interpretation rules

1. Never compare a cold result with a warm result.
2. Use the deterministic fixture for the process-level runtime contribution.
3. Treat real-model deltas inside normal model variance as inconclusive.
4. Publish medians, p95, sample counts, and every raw sample—not only an average.
5. A negative observed real-model difference does not mean SC Node accelerates
   inference; it means model/server variance exceeded the measured wrapper cost.
6. Do not generalize one CPU, OS, model, or endpoint to other systems.
7. Do not convert these measurements into a generic "Rust is X times faster than
   Python" claim; no Python framework is part of this benchmark.

## Reproducing or extending

The non-published helper lives in
[`tools/sc-benchmark/`](../tools/sc-benchmark/). New benchmark dimensions should
be added there and to the harness while preserving:

- identical request semantics between baseline and SC Node;
- loopback-only deterministic fixtures;
- `--locked` builds;
- alternating order;
- explicit cold/warm separation;
- raw, machine-readable evidence;
- no secrets in output.
