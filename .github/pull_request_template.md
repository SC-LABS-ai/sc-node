## Summary

<!-- What does this change, and why? -->

## Linked issue

<!-- Closes #... , or "None" -->

## What changed

<!-- Bullet points of the actual changes. -->

## How tested

<!-- e.g. `cargo test --workspace`, plus any relevant script under scripts/
     (smoke-check.ps1, verify-local.ps1, verify-public-beta.ps1). -->

## Checklist

- [ ] `cargo fmt -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] No secrets, API keys, or private paths in code, comments, or commit history
- [ ] Docs updated if behavior, config schema, or CLI syntax changed
- [ ] Claims in this PR description and any docs touched are accurate as of this diff (no unverified "works", "secure", or maturity upgrades)
