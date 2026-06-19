# `contracts/shanghai/legacy/` — frozen audited tree (Taiko)

This subtree is a frozen copy of the pre-router `BoundlessMarket` and its transitive dependencies
**as deployed on Taiko mainnet**, compiled under the Shanghai EVM (`FOUNDRY_PROFILE=shanghai`). It
exists so the new router-based shanghai market can forward its pre-router legacy ABI to the audited
bytecode at the existing implementation address via a `fallback() + delegatecall` shim, without
re-introducing the legacy bodies into the new market's bytecode.

It is the Taiko/Shanghai counterpart of `contracts/src/legacy/` (which freezes the Base/Cancun
deployment). The two are separate because they reproduce different deployed bytecode: Taiko's impl
was compiled for the Shanghai EVM (no `tstore`/`mcopy`), so it uses the `sstore`/`sload`
`FulfillmentContext` and the `compat/Bytes` shim rather than the Cancun originals.

## Provenance

The sources here mirror the pre-router shanghai market suite (`contracts/shanghai/src/`) as it was
at the Taiko deployment. The only diffs from those sources are file-basename + import renames
(`BoundlessMarket.sol` → `BoundlessMarketLegacy.sol`, `IBoundlessMarket*.sol` →
`IBoundlessMarket*Legacy.sol`) so Forge writes artifacts to distinct `out-shanghai/` directories; the
contract **symbols** stay `BoundlessMarket` / `IBoundlessMarket*` so the build (with
`bytecode_hash = none`) is byte-identical to the deployed audited code.

The on-chain identity that ultimately matters is the deployed bytecode at the Taiko mainnet
`BoundlessMarket` proxy's pre-upgrade implementation address:

- proxy: `0xb3f5c7b4379052eade8c7f3fa6da37fb871da28b`
- impl: `0x6c2d2c33e9a7cd0e1b39dc218f472e4bf534523b`

The bytecode-parity invariant under `deployed-bytecode.hex` + `deployed-bytecode.meta.toml` (in this
directory) is the load-bearing check.

## Verification

```bash
# Bytecode parity: this tree compiles to the deployed Taiko impl (modulo immutables).
FOUNDRY_PROFILE=shanghai forge build contracts/shanghai/legacy/BoundlessMarketLegacy.sol
BOUNDLESS_OUT_DIR=out-shanghai BOUNDLESS_LEGACY_SNAPSHOT_DIR=contracts/shanghai/legacy \
    uv run contracts/scripts/verify-legacy-bytecode.py

# Storage-layout interop: the new shanghai market can safely delegatecall this legacy impl.
FOUNDRY_PROFILE=shanghai forge build \
    contracts/shanghai/variants/BoundlessMarket.sol contracts/shanghai/legacy/BoundlessMarketLegacy.sol
BOUNDLESS_OUT_DIR=out-shanghai uv run contracts/scripts/verify-storage-layout.py
```

(Each command is a single line; the `\` continuations above are for documentation only — paste them
as one line.)

## Freeze policy

Do not modify any file in this tree. Any drift between the frozen source and the deployed audited
bytecode is a real issue — fix the drift, do not edit the verification script or this snapshot to
mask it. The deployed implementation is reused directly as the new market's `LEGACY_IMPL`; this tree
is the source-of-record + the interop safety check, not a redeployment target.
