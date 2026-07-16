# Third-Party Notices

SC Node is written in Rust and depends on third-party crates distributed
via [crates.io](https://crates.io). None of their source code is vendored
or modified in this repository — they are pulled in at build time via
Cargo, unmodified, under their own licenses.

This file is a human-readable notice pointer, not a substitute for the
machine-generated inventory. For the full list of resolved packages
(name, version, license) see
[docs/DEPENDENCY_INVENTORY.md](docs/DEPENDENCY_INVENTORY.md), which is
derived from `Cargo.lock` via `cargo metadata` (225 external packages as
of that document's snapshot).

## License Summary

The overwhelming majority of SC Node's dependency tree is permissively
licensed: `MIT`, `Apache-2.0`, `MIT OR Apache-2.0`, `Unicode-3.0`, `ISC`,
`BSD-3-Clause`, or similar. A small number of packages carry other
permissive-with-notice terms worth calling out explicitly:

| Package | License | Note |
|---------|---------|------|
| `option-ext` | MPL-2.0 | Weak, file-level copyleft. Used unmodified via crates.io (pulled in transitively through `dirs`/`dirs-sys`); no MPL-licensed file has been modified or vendored into this repository. |
| `webpki-roots` | CDLA-Permissive-2.0 | Permissive data license covering a bundled root-certificate list; not a code-copyleft concern. |
| `r-efi` | MIT OR Apache-2.0 OR LGPL-2.1-or-later | Triple-licensed; SC Node's use relies on the MIT/Apache-2.0 options. |
| `subtle` | BSD-3-Clause | Permissive; requires only attribution. |

No dependency in the resolved graph is licensed under a strong,
whole-program copyleft license (GPL/AGPL) as reported by `cargo metadata`.

## SC Node's Own License

SC Node itself is distributed under `MIT OR Apache-2.0` (see
`Cargo.toml`). The full text of both options is provided in this
repository: see [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE). You may choose either license.

## Obtaining Full License Texts

Full license texts for each dependency are available from the
corresponding crate's page on crates.io (e.g.
`https://crates.io/crates/<name>`) or from its source repository. This
repository does not bundle third-party license texts inline; if a
downstream redistribution requirement calls for that, generate a
consolidated notices file with a tool such as
[`cargo-about`](https://github.com/EmbarkStudios/cargo-about) or
[`cargo-license`](https://github.com/onur/cargo-license) (neither is
installed or run as part of this repository's build):

```bash
cargo install cargo-about --locked
cargo about generate about.hbs > full-license-texts.html
```

## Reporting an Issue With This Notice

If you believe a dependency's license has been misrepresented here or in
`docs/DEPENDENCY_INVENTORY.md`, please open an issue — this file is
derived from tooling output and reviewed manually, but tooling and manual
review can both be wrong.
