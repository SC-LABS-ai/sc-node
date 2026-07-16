# SC Node — Dependency Inventory & SBOM Generation

> **Update note (2026-07-16):** this inventory is a point-in-time snapshot that
> predates the `sc-contract`, `sc-proof`, and `sc-memory` crates. The workspace
> now has **one binary crate (`sc-agent`) plus 15 library crates**, and `sha2`,
> `hex`, and `chrono` are now actually consumed (by `sc-proof`/`sc-contract`).
> The package counts and the "declared but not resolved" table below reflect the
> earlier snapshot; regenerate per §4 before any release that needs exact counts.

This document is derived mechanically from `Cargo.lock` via `cargo metadata`
— it is not hand-typed and not guessed. It is a point-in-time snapshot; see
[§4](#4-regenerating-this-inventory) for the exact commands to regenerate
it. No SBOM tool (`cargo-cyclonedx`, `cargo-auditable`) is installed in the
environment this snapshot was generated in, so **no CycloneDX/SPDX SBOM
file has been fabricated** — instead this document documents the exact
command to produce one, and provides this Cargo.lock-derived inventory as
the honest fallback in the meantime.

## 1. Summary

| Metric | Value |
|--------|-------|
| Workspace member crates (internal, this repo) | 13 (`sc-agent` binary + 12 library crates under `crates/`) |
| Resolved external (third-party) packages | 225 |
| Total packages in the resolved dependency graph | 238 |
| Packages with an undeterminable license | 0 — `cargo metadata` reports a `license` field for every resolved package |
| Cargo edition | 2024 |
| Generated with | `cargo metadata --format-version 1` against the `Cargo.lock` committed alongside this document's commit |

**License mix across all 238 packages (self-reported `license` field,
counted by exact string, not normalized):**

| License string | Count |
|---|---|
| MIT OR Apache-2.0 | 145 |
| MIT | 34 |
| Unicode-3.0 | 18 |
| Apache-2.0 OR MIT | 12 |
| MIT/Apache-2.0 | 7 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 3 |
| Apache-2.0 OR ISC OR MIT | 3 |
| Unlicense OR MIT | 2 |
| MIT OR Apache-2.0 OR Zlib | 2 |
| ISC | 2 |
| Zlib OR Apache-2.0 OR MIT | 1 |
| MPL-2.0 | 1 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 1 |
| CDLA-Permissive-2.0 | 1 |
| BSD-3-Clause | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
| Apache-2.0 AND ISC | 1 |
| Apache-2.0 / MIT | 1 |
| Apache-2.0 | 1 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 |

**License notes (flagged for human review, not blocking):**

- **`option-ext` 0.2.0 — `MPL-2.0`.** Weak (file-level) copyleft. Pulled in
  transitively via the `dirs`/`dirs-sys` crates used for config-directory
  discovery. MPL-2.0 only requires source disclosure for modifications to
  the MPL-licensed files themselves, not for the rest of the codebase, and
  SC Node does not vendor or modify `option-ext`'s source — it is used
  unmodified via crates.io. Documented here rather than silently accepted.
- **`webpki-roots` 1.0.8 — `CDLA-Permissive-2.0`.** A permissive data
  license covering the bundled Mozilla root certificate list; not a code
  copyleft concern.
- **`r-efi` 6.0.0 — `MIT OR Apache-2.0 OR LGPL-2.1-or-later`.** Triple-
  licensed; SC Node relies on the `MIT`/`Apache-2.0` options, so the
  LGPL option is never the operative license for our use.
- **`subtle` 2.6.1 — `BSD-3-Clause`.** Permissive; no action needed.

No package in the resolved graph is licensed under a strong (whole-program)
copyleft license (e.g. GPL, AGPL) as reported by `cargo metadata`.

## 2. Direct Dependencies (declared in the workspace `Cargo.toml`)

Versions below are the versions actually resolved into `Cargo.lock` for
the workspace's `[workspace.dependencies]` pins (not just the pin string).
Purpose is derived from how each crate is actually used in this codebase.

| Crate | Resolved version | License | Purpose in SC Node |
|-------|------------------:|---------|--------------------|
| `tokio` | 1.52.3 | MIT | Async runtime for the CLI, providers, and tool execution |
| `tokio-util` | 0.7.18 | MIT | Tokio I/O/codec utilities (used by `sc-provider-ollama` and pulled in via `reqwest`) |
| `reqwest` | 0.12.28 | MIT OR Apache-2.0 | HTTP client for Ollama/OpenRouter/NVIDIA NIM providers (rustls-tls, no OpenSSL) |
| `serde` | 1.0.228 | MIT OR Apache-2.0 | Serialization framework used throughout (config, messages, audit entries) |
| `serde_json` | 1.0.150 | MIT OR Apache-2.0 | JSON encode/decode for provider APIs and tool arguments |
| `anyhow` | 1.0.103 | MIT OR Apache-2.0 | Application-level error handling in the binary and higher-level crates |
| `thiserror` | 1.0.69 | MIT OR Apache-2.0 | Typed error enums (e.g. `sc-config::ConfigError`) |
| `clap` | 4.6.1 | MIT OR Apache-2.0 | CLI argument parsing (`derive` + `env` features) |
| `toml` | 0.8.23 | MIT OR Apache-2.0 | Reading/writing `config.toml` |
| `dirs` | 5.0.1 | MIT OR Apache-2.0 | Cross-platform config/data directory discovery |
| `async-trait` | 0.1.89 | MIT OR Apache-2.0 | Async methods in the `Provider` trait |
| `futures` | 0.3.32 | MIT OR Apache-2.0 | Stream/future combinators |
| `uuid` | 1.23.5 | Apache-2.0 OR MIT | Session ID generation |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 | Timestamps in audit log entries and messages |
| `tracing` | 0.1.44 | MIT | Structured logging instrumentation |
| `tracing-subscriber` | 0.3.23 | MIT | Logging subscriber/formatter wired up in `main.rs` |
| `shell-escape` | 0.1.5 | MIT/Apache-2.0 | Shell-argument quoting helper for the shell tool |
| `shellexpand` | 2.1.2 | MIT/Apache-2.0 | `~` home-directory expansion in workspace/config paths |
| `tempfile` (dev-dependency) | 3.27.0 | MIT OR Apache-2.0 | Temporary files/directories in `sc-audit` tests |

**Declared but not currently resolved into the dependency graph** (present
in `[workspace.dependencies]`, but no crate in the workspace depends on
them yet — `cargo metadata` does not resolve them because nothing pulls
them in):

| Crate | Declared pin | Status |
|-------|--------------|--------|
| `tokio-stream` | `0.1` | Not consumed by any crate yet |
| `sha2` | `0.10` | Not consumed by any crate yet (the audit log has no hash-chaining — see THREAT_MODEL.md T10 — so this dependency is reserved for that future work, not in use today) |
| `hex` | `0.4` | Not consumed by any crate yet |
| `regex` | `1` | Not consumed by any crate yet (pattern matching in `sc-sandbox`/`sc-tool-core` is implemented with hand-rolled glob/substring logic, not the `regex` crate) |

Carrying unused dependency declarations is harmless for licensing/SBOM
purposes (they contribute nothing to the compiled binary or the resolved
graph) but is worth cleaning up eventually to keep the workspace manifest
honest about what is actually linked.

## 3. Full Resolved Dependency Graph (generated, external packages only)

The 13 internal workspace crates (`sc-agent`, `sc-agent-core`, `sc-audit`,
`sc-config`, `sc-message-types`, `sc-provider-core`, `sc-provider-nvidia`,
`sc-provider-ollama`, `sc-provider-openrouter`, `sc-sandbox`,
`sc-tool-core`, `sc-tool-file`, `sc-tool-shell`) are excluded below since
they are this repository, not a third-party dependency. All 225 remaining
entries are transitive or direct external crates, sorted by name.

<details>
<summary>Click to expand — 225 external packages (name, version, license)</summary>

| Package | Version | License |
|---|---|---|
| aho-corasick | 1.1.4 | Unlicense OR MIT |
| android_system_properties | 0.1.5 | MIT/Apache-2.0 |
| anstream | 1.0.0 | MIT OR Apache-2.0 |
| anstyle | 1.0.14 | MIT OR Apache-2.0 |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 |
| anstyle-wincon | 3.0.11 | MIT OR Apache-2.0 |
| anyhow | 1.0.103 | MIT OR Apache-2.0 |
| async-trait | 0.1.89 | MIT OR Apache-2.0 |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| base64 | 0.22.1 | MIT OR Apache-2.0 |
| bitflags | 2.13.0 | MIT OR Apache-2.0 |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 |
| bytes | 1.12.1 | MIT |
| cc | 1.2.67 | MIT OR Apache-2.0 |
| cfg-if | 1.0.4 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.1 | MIT |
| chacha20 | 0.10.1 | MIT OR Apache-2.0 |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| clap | 4.6.1 | MIT OR Apache-2.0 |
| clap_builder | 4.6.0 | MIT OR Apache-2.0 |
| clap_derive | 4.6.1 | MIT OR Apache-2.0 |
| clap_lex | 1.1.0 | MIT OR Apache-2.0 |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 |
| core-foundation | 0.10.1 | MIT OR Apache-2.0 |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 |
| cpufeatures | 0.3.0 | MIT OR Apache-2.0 |
| dirs | 4.0.0 | MIT OR Apache-2.0 |
| dirs | 5.0.1 | MIT OR Apache-2.0 |
| dirs-sys | 0.3.7 | MIT OR Apache-2.0 |
| dirs-sys | 0.4.1 | MIT OR Apache-2.0 |
| displaydoc | 0.2.6 | MIT OR Apache-2.0 |
| equivalent | 1.0.2 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| fastrand | 2.4.1 | Apache-2.0 OR MIT |
| find-msvc-tools | 0.1.9 | MIT OR Apache-2.0 |
| fnv | 1.0.7 | Apache-2.0 / MIT |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 |
| futures | 0.3.32 | MIT OR Apache-2.0 |
| futures-channel | 0.3.32 | MIT OR Apache-2.0 |
| futures-core | 0.3.32 | MIT OR Apache-2.0 |
| futures-executor | 0.3.32 | MIT OR Apache-2.0 |
| futures-io | 0.3.32 | MIT OR Apache-2.0 |
| futures-macro | 0.3.32 | MIT OR Apache-2.0 |
| futures-sink | 0.3.32 | MIT OR Apache-2.0 |
| futures-task | 0.3.32 | MIT OR Apache-2.0 |
| futures-util | 0.3.32 | MIT OR Apache-2.0 |
| getrandom | 0.2.17 | MIT OR Apache-2.0 |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| h2 | 0.4.15 | MIT |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| http | 1.4.2 | MIT OR Apache-2.0 |
| http-body | 1.0.1 | MIT |
| http-body-util | 0.1.3 | MIT |
| httparse | 1.10.1 | MIT OR Apache-2.0 |
| hyper | 1.10.1 | MIT |
| hyper-rustls | 0.27.9 | Apache-2.0 OR ISC OR MIT |
| hyper-util | 0.1.20 | MIT |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 |
| iana-time-zone-haiku | 0.1.2 | MIT OR Apache-2.0 |
| icu_collections | 2.2.0 | Unicode-3.0 |
| icu_locale_core | 2.2.0 | Unicode-3.0 |
| icu_normalizer | 2.2.0 | Unicode-3.0 |
| icu_normalizer_data | 2.2.0 | Unicode-3.0 |
| icu_properties | 2.2.0 | Unicode-3.0 |
| icu_properties_data | 2.2.0 | Unicode-3.0 |
| icu_provider | 2.2.0 | Unicode-3.0 |
| idna | 1.1.0 | MIT OR Apache-2.0 |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT |
| indexmap | 2.14.0 | Apache-2.0 OR MIT |
| ipnet | 2.12.0 | MIT OR Apache-2.0 |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| js-sys | 0.3.103 | MIT OR Apache-2.0 |
| lazy_static | 1.5.0 | MIT OR Apache-2.0 |
| libc | 0.2.186 | MIT OR Apache-2.0 |
| libredox | 0.1.18 | MIT |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| litemap | 0.8.2 | Unicode-3.0 |
| log | 0.4.33 | MIT OR Apache-2.0 |
| lru-slab | 0.1.2 | MIT OR Apache-2.0 OR Zlib |
| matchers | 0.2.0 | MIT |
| memchr | 2.8.3 | Unlicense OR MIT |
| mio | 1.2.1 | MIT |
| nu-ansi-term | 0.50.3 | MIT |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| once_cell_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| openssl-probe | 0.2.1 | MIT OR Apache-2.0 |
| option-ext | 0.2.0 | MPL-2.0 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| potential_utf | 0.1.5 | Unicode-3.0 |
| proc-macro2 | 1.0.106 | MIT OR Apache-2.0 |
| quinn | 0.11.11 | MIT OR Apache-2.0 |
| quinn-proto | 0.11.16 | MIT OR Apache-2.0 |
| quinn-udp | 0.5.15 | MIT OR Apache-2.0 |
| quote | 1.0.46 | MIT OR Apache-2.0 |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| rand | 0.10.2 | MIT OR Apache-2.0 |
| rand_core | 0.10.1 | MIT OR Apache-2.0 |
| rand_pcg | 0.10.2 | MIT OR Apache-2.0 |
| redox_users | 0.4.6 | MIT |
| regex-automata | 0.4.15 | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 |
| reqwest | 0.12.28 | MIT OR Apache-2.0 |
| ring | 0.17.14 | Apache-2.0 AND ISC |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT |
| rustix | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustls | 0.23.41 | Apache-2.0 OR ISC OR MIT |
| rustls-native-certs | 0.8.4 | Apache-2.0 OR ISC OR MIT |
| rustls-pki-types | 1.15.0 | MIT OR Apache-2.0 |
| rustls-webpki | 0.103.13 | ISC |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| schannel | 0.1.29 | MIT |
| security-framework | 3.7.0 | MIT OR Apache-2.0 |
| security-framework-sys | 2.17.0 | MIT OR Apache-2.0 |
| serde | 1.0.228 | MIT OR Apache-2.0 |
| serde_core | 1.0.228 | MIT OR Apache-2.0 |
| serde_derive | 1.0.228 | MIT OR Apache-2.0 |
| serde_json | 1.0.150 | MIT OR Apache-2.0 |
| serde_spanned | 0.6.9 | MIT OR Apache-2.0 |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 |
| sharded-slab | 0.1.7 | MIT |
| shell-escape | 0.1.5 | MIT/Apache-2.0 |
| shellexpand | 2.1.2 | MIT/Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| slab | 0.4.12 | MIT |
| smallvec | 1.15.2 | MIT OR Apache-2.0 |
| socket2 | 0.6.4 | MIT OR Apache-2.0 |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 |
| strsim | 0.11.1 | MIT |
| subtle | 2.6.1 | BSD-3-Clause |
| syn | 2.0.118 | MIT OR Apache-2.0 |
| sync_wrapper | 1.0.2 | Apache-2.0 |
| synstructure | 0.13.2 | MIT |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| thiserror | 1.0.69 | MIT OR Apache-2.0 |
| thiserror | 2.0.18 | MIT OR Apache-2.0 |
| thiserror-impl | 1.0.69 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.18 | MIT OR Apache-2.0 |
| thread_local | 1.1.10 | MIT OR Apache-2.0 |
| tinystr | 0.8.3 | Unicode-3.0 |
| tinyvec | 1.12.0 | Zlib OR Apache-2.0 OR MIT |
| tinyvec_macros | 0.1.1 | MIT OR Apache-2.0 OR Zlib |
| tokio | 1.52.3 | MIT |
| tokio-macros | 2.7.0 | MIT |
| tokio-rustls | 0.26.4 | MIT OR Apache-2.0 |
| tokio-util | 0.7.18 | MIT |
| toml | 0.8.23 | MIT OR Apache-2.0 |
| toml_datetime | 0.6.11 | MIT OR Apache-2.0 |
| toml_edit | 0.22.27 | MIT OR Apache-2.0 |
| toml_write | 0.1.2 | MIT OR Apache-2.0 |
| tower | 0.5.3 | MIT |
| tower-http | 0.6.11 | MIT |
| tower-layer | 0.3.3 | MIT |
| tower-service | 0.3.3 | MIT |
| tracing | 0.1.44 | MIT |
| tracing-attributes | 0.1.31 | MIT |
| tracing-core | 0.1.36 | MIT |
| tracing-log | 0.2.0 | MIT |
| tracing-subscriber | 0.3.23 | MIT |
| try-lock | 0.2.5 | MIT |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| untrusted | 0.9.0 | ISC |
| url | 2.5.8 | MIT OR Apache-2.0 |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT |
| uuid | 1.23.5 | Apache-2.0 OR MIT |
| valuable | 0.1.1 | MIT |
| want | 0.3.1 | MIT |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasm-bindgen | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-futures | 0.4.76 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.126 | MIT OR Apache-2.0 |
| wasm-streams | 0.4.2 | MIT OR Apache-2.0 |
| web-sys | 0.3.103 | MIT OR Apache-2.0 |
| web-time | 1.1.0 | MIT OR Apache-2.0 |
| webpki-roots | 1.0.8 | CDLA-Permissive-2.0 |
| winapi | 0.3.9 | MIT/Apache-2.0 |
| winapi-i686-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| winapi-x86_64-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-sys | 0.48.0 | MIT OR Apache-2.0 |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-targets | 0.48.5 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.48.5 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.48.5 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.48.5 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.48.5 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.48.5 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.48.5 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.48.5 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| winnow | 0.7.15 | MIT |
| writeable | 0.6.3 | Unicode-3.0 |
| yoke | 0.8.3 | Unicode-3.0 |
| yoke-derive | 0.8.2 | Unicode-3.0 |
| zerofrom | 0.1.8 | Unicode-3.0 |
| zerofrom-derive | 0.1.7 | Unicode-3.0 |
| zeroize | 1.9.0 | Apache-2.0 OR MIT |
| zerotrie | 0.2.4 | Unicode-3.0 |
| zerovec | 0.11.6 | Unicode-3.0 |
| zerovec-derive | 0.11.3 | Unicode-3.0 |
| zmij | 1.0.23 | MIT |

</details>

Duplicate rows (e.g. `dirs` 4.0.0 and 5.0.1, `thiserror` 1.0.69 and 2.0.18)
are not a mistake — they reflect two different major versions of the same
crate coexisting in the resolved graph (one pulled in directly by SC Node,
the other transitively by a different dependency). This is normal for
Cargo and does not indicate duplication of code shipped twice in a way
that matters for licensing (each version is its own licensed artifact).

## 4. Regenerating This Inventory

This document was produced with:

```bash
# From the repository root:
cargo metadata --format-version 1 > metadata.json

# Then the summary/tables above were derived with jq, e.g.:
jq -r '.packages[] | "\(.name)\t\(.version)\t\(.license // "UNKNOWN")"' metadata.json
```

`cargo metadata` reads `Cargo.lock` and each dependency's own `Cargo.toml`
`license` field — it requires network access only if the local registry
cache/vendor directory is missing the relevant crate metadata (it was not
needed here; the workspace already had a populated `Cargo.lock` and cargo
registry cache).

## 5. SBOM Generation Approach

**No SBOM tool is installed in this environment as of this document.**
Rather than fabricate a CycloneDX/SPDX file by hand, here is the exact,
documented path to generate a real one:

### Option A — CycloneDX (recommended, industry-standard format)

```bash
cargo install cargo-cyclonedx --locked
cargo cyclonedx --format json --output-cdx cyclonedx-sbom.json
```

This produces a CycloneDX JSON SBOM covering every resolved package in
`Cargo.lock`, including PURLs and license identifiers. Not run as part of
this change (tool not installed); run it before any release that needs a
formal SBOM artifact, and commit or attach the resulting file rather than
this Markdown table.

### Option B — `cargo-auditable` (embeds dependency metadata in the binary)

```bash
cargo install cargo-auditable --locked
cargo auditable build --release
# Later, extract from the compiled binary:
cargo install auditable-info --locked
auditable-info target/release/sc-agent
```

This embeds a compressed dependency manifest directly into the compiled
binary, which is useful for auditing a shipped artifact after the fact
(e.g. "what did I actually ship in this release") rather than the source
tree. Also not run as part of this change (tool not installed).

### Fallback (used for this document, no extra tooling required)

```bash
cargo metadata --format-version 1
```

This is what generated the tables above. It is offline, requires no
additional tool installation, and is a legitimate (if less standardized
than CycloneDX/SPDX) dependency inventory. It is the basis for this
document and is the recommended fallback whenever `cargo-cyclonedx` /
`cargo-auditable` are unavailable.

## 6. Vulnerability Scanning (Not Run)

`cargo audit` (RustSec advisory database) and `cargo deny` (license/ban
policy enforcement) are **not installed** in this environment and were
**not run** as part of this change. No claim is made here about the
presence or absence of known vulnerabilities in the dependency tree above.
To check:

```bash
cargo install cargo-audit --locked
cargo audit

cargo install cargo-deny --locked
cargo deny check
```

See [STATUS.md](../STATUS.md) for this as a tracked gap.
