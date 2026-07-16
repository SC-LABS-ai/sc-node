# SC Node — Benchmarking

> **As of:** 2026-07-16 · Experimental public alpha.

## State

SC Node is **designed for low overhead**: a thin Rust execution layer, no
background threads beyond the async runtime, no telemetry, and streaming that
avoids buffering whole cloud responses. That is a design intent, **not a measured
result.**

- **Competitive performance is NOT yet proven.**
- **No benchmark numbers are published.** Any figure you see elsewhere that is
  not reproduced by the methodology below should be treated as unverified.

This document exists so that when numbers are published, they are produced by a
stated, reproducible method rather than asserted.

## Planned methodology

Each measurement should compare a **baseline** (calling the provider directly)
against **through-SC-Node**, so the reported overhead is SC Node's contribution
and not the model's latency:

1. **Direct-provider-call vs through-SC-Node.** Same prompt, same model, same
   endpoint; measure wall-clock and CPU for a direct HTTP call vs the same task
   routed through SC Node's loop.
2. **Cold vs warm start.** Process startup + config load + provider construction
   (cold) vs steady-state per-task cost (warm), reported separately.
3. **Routing overhead.** Time spent in deterministic route resolution, isolated
   from any network call (the routing module is pure and can be measured on its
   own).
4. **Tool-dispatch overhead.** Cost of the permission gate + audit emission per
   tool call, measured with a no-op tool so model/tool latency is excluded.
5. **Memory usage.** Resident set for an idle session and during a representative
   multi-round task.
6. **Audit / proof overhead.** Incremental cost of audit logging (on vs off) and
   of building/verifying a proof-bundle hash chain.

## Reproducibility requirements

Any published number must ship with:

- exact SC Node commit hash and `cargo build --release` profile;
- OS, CPU, and RAM of the measurement machine;
- provider, endpoint, and model (and whether it was local or cloud);
- the exact command(s) run and the raw measurements (not just an average);
- number of iterations and how outliers/warmup were handled;
- a note on any variability (e.g. cloud rate limits, thermal throttling).

Numbers that cannot be reproduced from this information will not be presented as
benchmarks. Progress is tracked in [ROADMAP.md](ROADMAP.md).
