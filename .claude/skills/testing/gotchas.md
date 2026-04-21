# Gotchas

Common pitfalls that cause test failures unrelated to your code changes.

## RISC0_SKIP_BUILD causes bytecode mismatch

**Never** use `RISC0_SKIP_BUILD=1` when running broker tests that deploy contracts to Anvil (e2e tests, order_committer tests, order_pricer tests, etc). Skipping the guest build means `bytecode.rs` doesn't get regenerated from the compiled Solidity contracts, causing `InvalidAssessorImage` reverts during contract deployment.

Use `RISC0_DEV_MODE=1` (without `RISC0_SKIP_BUILD`) for these tests. `RISC0_SKIP_BUILD` is only safe for tests that don't deploy contracts (e.g. config tests, pure unit tests).

## RISC0_DEV_MODE must be set

Most tests need `RISC0_DEV_MODE=1` to use fake proofs instead of real proving. Without it, tests will try to call Bonsai or generate real STARK proofs and fail.

## forge build before Rust tests

If you changed any Solidity contracts, run `forge build` before running Rust tests. The broker's `build.rs` reads from `contracts/out/` to generate Rust bindings. Stale contract artifacts cause deployment failures.

## Database tests need PostgreSQL

Tests in `order-stream`, `boundless-indexer`, `boundless-cli`, and `boundless-bench` require a running PostgreSQL instance. Use `just test-db setup` before running them, or use `just test-cargo-db` which handles setup/teardown.

## Clippy flags

Always run clippy the way the justfile does:

```
RUSTFLAGS=-Dwarnings RISC0_SKIP_BUILD=1 RISC0_SKIP_BUILD_KERNELS=1 cargo clippy --workspace --all-targets
```

`RISC0_SKIP_BUILD` is fine for clippy since it only checks types, not runtime behavior. The `-Dwarnings` flag is what CI uses — don't omit it.

## Markdown formatting with dprint

If you create or edit any `.md` files, run `dprint check` (or `dprint fmt` to auto-fix). CI runs `dprint check` as part of `just check-format`. dprint enforces consistent markdown formatting (blank lines around code fences, table alignment, etc). Check the justfile for the exact commands — `just check-format` runs it, `just format` fixes it.
