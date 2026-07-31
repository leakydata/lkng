# `fdev build` panics "Could not find workspace root" unless CARGO_TARGET_DIR is set

## Summary

`fdev build` (0.3.278) panics for a contract crate that is its own
workspace root:

```
thread 'main' panicked at crates/fdev/src/util.rs:103:10:
Could not find workspace root
stack backtrace:
   3: fdev::util::get_workspace_target_dir
   4: fdev::build::compile_rust_wasm_lib
   5: fdev::build::contract::compile_contract
```

## Cause

`get_workspace_target_dir` falls back to `env!("CARGO_MANIFEST_DIR")` —
which is **fdev's own compile-time manifest path**, not the user's crate —
and then walks *that* tree looking for a `Cargo.toml` containing
`[workspace]`. For an installed `fdev` binary that path doesn't exist on
the user's machine, so `find_workspace_root_from(...).expect(...)` panics.

The `env::var("CARGO_TARGET_DIR")` branch above it succeeds, which is why
setting that variable works around it.

## Workaround

```bash
CARGO_TARGET_DIR="$PWD/target" fdev build
```

## Suggested fix

Resolve the workspace root from the **current directory** (where
`freenet.toml` was found) rather than from fdev's compile-time manifest
dir, and return a `Result` instead of `expect`ing — the panic gives no hint
that the target dir is the problem.

## Environment

`fdev 0.3.278`, Linux x86_64, contract crate with `[workspace]` in its own
`Cargo.toml` (matching the mail/delta layout).
