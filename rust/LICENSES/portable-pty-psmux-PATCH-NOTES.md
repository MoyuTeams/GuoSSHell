# rsHell patch provenance

This directory contains only Task 4's manifest-selected files from the crates.io
`portable-pty-psmux` package version `0.9.6`. Cargo selected that exact package
with crates.io checksum
`793e46fb3212b514f6eb694e26a64aeaca64b47a2d66b810351b44628e307a0e`.
The package records upstream VCS revision
`05cc5d4cded047bd3f3d1955299fd0bd259f2d81` at
`crates/portable-pty-psmux`; no repository history is vendored here.

rsHell changes the package manifest plus these four package source files:

- `Cargo.toml`: declare the opt-in containment boundary test-support feature.
- `src/lib.rs`: add the Windows-only borrowed Job-handle spawn interface.
- `src/win/procthreadattr.rs`: own stable heap-backed Job-list handle storage.
- `src/win/psuedocon.rs`: add the Job list to the ConPTY creation attributes.
- `src/win/conpty.rs`: share contained/uncontained spawning and preserve the Job on retry.

`src/cmdbuilder.rs` also skips rustfmt traversal into test modules omitted from
the published package; `src/win/mod.rs` and other touched Windows source lines
have formatting-only changes. All other package files are copies of the
selected crates.io source. The package remains outside rsHell workspace lint
membership. `rshell-session` depends on this vendored fork by relative path in
both normal and test builds, so downstream path/git consumers do not need a
root `[patch.crates-io]` override.
