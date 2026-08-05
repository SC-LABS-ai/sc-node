# SC Node overhead reference run — Windows / qwen:latest

> Measured 2026-08-05 from clean commit
> [`5fa4dd4bd5c3266019cbf830b8f9cf8b1b9eb28c`](https://github.com/SC-LABS-ai/sc-node/commit/5fa4dd4bd5c3266019cbf830b8f9cf8b1b9eb28c).

## Environment

- Windows x64, build `10.0.26200`
- AMD Ryzen Threadripper 1950X, 16 cores / 32 logical processors
- 64 GiB physical RAM
- Rust `1.96.0`, MSVC target
- Ollama loopback endpoint with local `qwen:latest` (~2.33 GB)
- exact prompt: `Reply exactly with OK.`

The measured process peak RAM excludes the external Ollama server and model
memory. Every benchmark helper was built from `Cargo.lock` with
`cargo build --release --locked -p sc-benchmark`.

## Validation

- **65 process measurements**
- **65 successful exit codes**
- **0 stderr-producing measurements**
- 20 deterministic fixture pairs
- 8 warm real-model pairs
- 3 cold direct runs and 3 cold SC Node runs
- 500 iterations per synthetic agent scenario
- 100,000 routing/permission iterations and 1,000 I/O/proof iterations

## Main result

### Deterministic provider path

The local fixture implements the same Ollama HTTP shape but returns an immediate,
fixed response, so no model inference occurs.

- paired median of the helpers' internal timers: **+0.2135 ms** for SC Node
- observed range: **-0.6417 to +1.1298 ms**
- paired process-wall median: **-1.0581 ms**, with much wider scheduler noise

The correct interpretation is that the incremental SC Node path was **around a
fraction of a millisecond on this machine** and too close to process/scheduler
noise to justify a speedup claim.

### Real warm local model

Independent wall-clock medians:

| Path | N | Median | p95 | Median process peak RAM |
|---|---:|---:|---:|---:|
| Direct Ollama | 8 | 533.4315 ms | 558.2198 ms | 6.77 MiB |
| Through SC Node | 8 | 534.5057 ms | 555.7163 ms | 6.79 MiB |

The difference between those medians is **+1.0742 ms (+0.2014%)**. The paired
internal-timer median was **+4.5202 ms**, while individual pair differences ranged
from **-67.3746 to +54.8552 ms**. Model and server variability is therefore much
larger than the observed wrapper difference.

Cold runs were dominated by model loading (roughly 8–9 seconds) and are retained
in the raw data but are not used to estimate runtime overhead.

## Deterministic microbenchmarks

| Operation | Iterations | Time per operation |
|---|---:|---:|
| Route resolution | 100,000 | 431.904 ns |
| Permission check | 100,000 | 1.509958 µs |
| Disabled audit call | 1,000 | 314.6 ns |
| Audit append + flush | 1,000 | 7.4483 µs |
| Build SHA-256 chain for 100 events | 1,000 | 429.7855 µs |
| Verify SHA-256 chain for 100 events | 1,000 | 312.8804 µs |

## Synthetic full-agent path

Internal time per iteration over 500 iterations:

| Scenario | Time per iteration |
|---|---:|
| Agent round, no tool | 28.388 µs |
| Agent round, one no-op tool, no audit | 57.6902 µs |
| Agent round, one no-op tool, append-and-flush audit | 961.8084 µs |

The no-op tool contains an atomic execution counter. The benchmark aborts unless
exactly one tool execution occurs per requested iteration.

## Conclusion

On this Windows machine and local model, SC Node's measured runtime contribution
was small relative to model latency. The data supports the narrower statement
that **SC Node adds little observable latency in this configuration**.

It does **not** establish that SC Node is faster than another agent framework,
that Rust is a specific multiple faster than Python, or that the same numbers
apply to other machines, providers, prompts, or models.

## Evidence

- [`environment.json`](environment.json) — sanitized environment and exact settings
- [`summary.json`](summary.json) — aggregate values and validation
- [`raw-process-measurements.csv`](raw-process-measurements.csv) — all 65 process rows
- [`micro.jsonl`](micro.jsonl) — all deterministic microbenchmark results
- [`SHA256SUMS.txt`](SHA256SUMS.txt) — hashes of the published evidence

Reproduction instructions and interpretation rules are in
[`../../BENCHMARKING.md`](../../BENCHMARKING.md).
