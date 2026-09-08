# Releasing Ultra Edit

GitHub Actions builds and publishes releases from exact `vMAJOR.MINOR.PATCH`
tags whose commits are reachable from `main`. The final job creates a draft bound
to the exact tag and commit, uploads and verifies the complete asset set, and
publishes it only after every gate passes. A rerun can resume that matching draft
or verify the exact published release; it refuses to use any other existing
release.

## Release contents

Each release contains:

- `ultra-edit-plugin-vX.Y.Z-all.zip`, the primary Claude Code marketplace
  package;
- one thin plugin ZIP for each supported target;
- `marketplace.json`, whose archive source uses the versioned all-target ZIP and
  its exact SHA-256 digest; and
- `SHA256SUMS`, covering the catalog and every ZIP.

Every plugin ZIP includes Ultra Edit's MIT and Apache-2.0 licenses, notices for
Cargo dependencies in `THIRD_PARTY_LICENSES.txt`, and the Rust standard library
notices from the pinned toolchain in `RUST_STANDARD_LIBRARY_LICENSES.html`.
`MUSL_COPYRIGHT` covers the musl 1.2.3 libc bundled with Rust 1.89's Linux musl
targets. `LLVM_LIBUNWIND_LICENSE.txt` covers the LLVM libunwind archive bundled
with those targets from Rust 1.89's pinned LLVM revision.
`RUST_COMPILER_BUILTINS_LICENSE.txt` and `RUST_LIBM_LICENSE.txt` cover the Rust
compiler-builtins runtime and its bundled libm code. `LLVM_COMPILER_RT_LICENSE.txt`
covers the compiler-rt startup objects in the Linux musl toolchains.

The compiler runtime notices come from immutable source revisions:

| Notice | Source | SHA-256 |
| --- | --- | --- |
| `RUST_COMPILER_BUILTINS_LICENSE.txt` | [Rust 1.89 source](https://raw.githubusercontent.com/rust-lang/rust/29483883eed69d5fb4db01964cdf2af4d86e9cb2/library/compiler-builtins/LICENSE.txt) | `ab6eec6caf0fa5775e411c7a8bc6a45c4ef2956b0980b157ab74fc5cd62a928b` |
| `RUST_LIBM_LICENSE.txt` | [Rust 1.89 bundled libm source](https://raw.githubusercontent.com/rust-lang/rust/29483883eed69d5fb4db01964cdf2af4d86e9cb2/library/compiler-builtins/libm/LICENSE.txt) | `3823dda7cf046602f4b4e77ec8e227863dc4736037cc85bb33d9f19febe16bb7` |
| `LLVM_COMPILER_RT_LICENSE.txt` | [Rust 1.89 LLVM revision](https://raw.githubusercontent.com/rust-lang/llvm-project/9b1bf4cf041c1c1fe62cf03891ac90431615e780/compiler-rt/LICENSE.TXT) | `1a8f1058753f1ba890de984e48f0242a3a5c29a6a8f2ed9fd813f36985387e8d` |

The supported targets are:

| Platform | Rust target | GitHub runner |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-2025` |
| Linux x64 | `x86_64-unknown-linux-musl` | `ubuntu-24.04` |
| Linux ARM64 | `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` |
| macOS Intel | `x86_64-apple-darwin` | `macos-15-intel` |
| macOS Apple Silicon | `aarch64-apple-darwin` | `macos-15` |

Windows uses the static MSVC CRT. Linux uses musl to avoid a distribution-specific
glibc floor. macOS builds reject dependencies outside `/usr/lib` and
`/System/Library`. Each native job runs the Rust suite, builds both executables,
packages a complete thin plugin, extracts it, and executes both packaged
binaries. The downstream Linux jobs assemble the all-target archive, exercise
its launchers, validate the plugin and generated marketplace with Claude Code,
check every SHA-256 entry, and create GitHub build-provenance attestations. The
downloaded Claude Code package runs in a separate read-only job; the attestation
job consumes the immutable artifact in a fresh runner.

## Prepare a release

1. Update `version` in `Cargo.toml` and
   `plugin/claude-code/.claude-plugin/plugin.json` to the same semantic version.
   Run `cargo check` once so the root package entry in `Cargo.lock` follows the
   manifest.
2. Move the release notes in `CHANGELOG.md` under that version and date. Update
   documentation for any changed installation or compatibility contract.
3. If Cargo dependencies changed, regenerate their tracked notices with the
   pinned generator:

   ```text
   cargo install cargo-about --version 0.9.2 --locked --features cli
   cargo about generate --locked --fail third-party-licenses.hbs -o THIRD_PARTY_LICENSES.txt
   ```

   `THIRD_PARTY_LICENSES.txt` covers Cargo dependencies. When `RUST_VERSION`
   changes, separately copy `share/doc/rust/COPYRIGHT-library.html` from that
   exact rustc sysroot to `RUST_STANDARD_LIBRARY_LICENSES.html` byte for byte,
   and verify the musl, LLVM libunwind, Rust compiler-builtins, bundled libm,
   and LLVM compiler-rt revisions bundled with the new toolchain before
   retaining or refreshing their notices.

4. Run the local gates, replacing `X.Y.Z` below:

   ```text
   python scripts/release.py verify-version --tag vX.Y.Z
   python -m unittest discover -s scripts/tests -v
   cargo fmt --all -- --check
   cargo clippy --locked --all-targets -- -D warnings
   cargo check --locked --all-targets
   cargo test --locked --all-targets
   cargo test --locked --doc
   claude plugin validate plugin/claude-code --strict --json
   cargo about generate --locked --fail third-party-licenses.hbs -o THIRD_PARTY_LICENSES.generated.txt
   ```

   Compare `THIRD_PARTY_LICENSES.generated.txt` byte for byte with
   `THIRD_PARTY_LICENSES.txt`, and compare `RUST_STANDARD_LIBRARY_LICENSES.html`
   with `$(rustc +1.89.0 --print sysroot)/share/doc/rust/COPYRIGHT-library.html`.
   Then remove the generated comparison file. CI repeats these checks with Rust
   1.89.0 and cargo-about 0.9.2. It pins the musl and LLVM libunwind notices by
   SHA-256, and checks the compiler runtime notices against both their recorded
   SHA-256 values and their immutable upstream files.

5. Merge the reviewed change to `main`, push it, and wait for the branch CI run
   on that exact commit to pass:

   ```text
   git push origin main
   ```

6. Create an annotated tag on that verified commit and push the tag:

   ```text
   git tag -a vX.Y.Z -m "Ultra Edit X.Y.Z"
   git push origin vX.Y.Z
   ```

Pushing the tag starts `.github/workflows/release.yml`. Do not move or reuse a
published tag. If its commit needs a code change, release a new patch version.

## Verify the published release

Wait for every `Release` job to pass, then download the assets and verify them:

```text
sha256sum --check SHA256SUMS
gh attestation verify ultra-edit-plugin-vX.Y.Z-all.zip --repo subashc2023/ultra-edit
```

Confirm that the release is neither a draft nor a prerelease and that the stable
catalog URL returns its `marketplace.json`. Test a clean user-scoped install and
both executable versions:

```text
claude plugin marketplace add https://github.com/subashc2023/ultra-edit/releases/latest/download/marketplace.json
claude plugin install ultra-edit@ultra-edit
```

Start Claude Code in a disposable workspace, inspect `/mcp`, and confirm both the
`SessionStart` context and one focused snapshot-to-edit flow. Release binaries
are currently unsigned. Add Authenticode signing and macOS Developer ID signing
and notarization before promoting the package beyond an unsigned developer
release.
