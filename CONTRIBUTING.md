# Contributing to SC Node

Thanks for your interest. SC Node is an experimental public alpha — the
workflow below is intentionally lightweight but the honesty bar is not:
please read the [documentation policy](#documentation--honesty-policy)
before opening a pull request that touches docs or README claims.

## Before You Start

- Read [docs/STATUS.md](docs/STATUS.md) to see what's actually implemented vs.
  planned. Don't assume a feature exists because an archived design doc under
  `docs/archive/` describes it.
- Read [THREAT_MODEL.md](THREAT_MODEL.md), especially the "Explicitly
  Unmitigated / Known Gaps" section, before changing anything in
  `sc-sandbox`, `sc-tool-core`, `sc-tool-file`, or `sc-tool-shell`.
- Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the crate layout and
  control flow.

## Development Setup

Requires a recent stable Rust toolchain (edition 2024). This repository is
developed and tested on Windows; Linux/macOS contributions and CI are
welcome but currently unverified.

```bash
git clone https://github.com/SC-LABS-ai/sc-node
cd sc-node
cargo build --workspace
cargo test --workspace
```

## Required Checks Before Opening a PR

Run these locally and make sure they're clean:

```bash
cargo fmt -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On Windows, you can also run the smoke check and the broader
public-beta-verification gate runner:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\smoke-check.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\verify-public-beta.ps1
```

`verify-public-beta.ps1` reports `SKIP` (not `PASS`) for anything it cannot
exercise in the current environment — for example features that are not yet
wired into the runtime (such as local memory/RAG and Windows process
hardening), or live Ollama/NIM checks when no endpoint is reachable and no
credentials are present. A `SKIP` there is expected and is not a bug in your PR.

## Documentation & Honesty Policy

This is the part that matters most for review:

1. **Never upgrade a status claim without evidence.** If you implement a
   feature, record it in [docs/STATUS.md](docs/STATUS.md) as `works`/`partial`
   with a note on the gap; do not mark something `live-tested` or verified on a
   platform it has not actually been exercised on. Promotions happen
   deliberately, with evidence, separately from a feature PR.
2. **Never claim something is "secure," "production ready," "unbreakable,"
   or "military grade."** If you close a real gap from
   `THREAT_MODEL.md`, update that file to say the gap is mitigated and
   describe how — don't just delete the row.
3. **If you find a stale/incorrect claim in existing docs, fix it in the
   same PR** (or a preceding one) rather than building on top of it. This
   repository has already had at least one such correction (a
   `web_fetch`/SSRF-deny-list tool described in an earlier `SECURITY.md`
   that was never implemented) — don't reintroduce that pattern.
4. **New tools must implement `check_permission` enforcement before doing
   any I/O**, matching the pattern in `sc-tool-file`/`sc-tool-shell`:
   resolve the decision, and refuse to run on `Deny`. Don't ship a tool
   that skips the permission check "temporarily."
5. **New dependencies:** check the license via `cargo metadata` (or just
   look at its crates.io page) before adding it, and regenerate
   `docs/DEPENDENCY_INVENTORY.md` per the command documented in that file
   if your change adds or removes a dependency.

## Commit Style

Recent history in this repository uses a lightweight
`type: short description` prefix (`feat:`, `fix:`, `test:`, `docs:`,
`build:`, `style:`) — see `git log --oneline` for examples. Keep commits
focused; prefer several small commits over one large one when the changes
are logically separable.

## Reporting Security Issues

Do not open a public issue for a security vulnerability — see
[SECURITY.md](SECURITY.md) for the reporting process.
