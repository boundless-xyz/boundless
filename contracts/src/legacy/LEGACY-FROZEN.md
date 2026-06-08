# `contracts/src/legacy/` — frozen audited tree

This subtree is a frozen copy of `BoundlessMarket` and its transitive
dependencies as deployed on Base mainnet. It exists so the new market can
forward its pre-router legacy ABI to the audited bytecode at the existing
implementation address via a `fallback() + delegatecall` shim, without
re-introducing the legacy bodies into the new market's bytecode.

## Provenance

The sources here mirror **`main` at commit
[`507f7469`](https://github.com/boundless-xyz/boundless/commit/507f7469)**
(`BM-2598: add depositCollateralTo and depositCollateralWithPermitTo`,
2026-03-13) — the last commit on `main` that touched any of the files in
this tree. The only diffs from that commit are the file-basename + import
renames documented in the table below; the contract bodies are identical.

To verify provenance for any file in this tree:

```bash
diff <(git show 507f7469:contracts/src/<original-path>) \
     contracts/src/legacy/<renamed-path>
```

(For types/libraries that don't reference the renamed interfaces, the
diff should be empty.)

The on-chain identity that ultimately matters is the deployed bytecode at
the BoundlessMarket proxy's pre-upgrade implementation address. On Base
mainnet that is `0x22bb6bbe5d221ef3e738029dab4d1d27ec725cd3`. The
bytecode-parity invariant under `contracts/test/legacy/deployed-bytecode.hex` +
`deployed-bytecode.meta.toml` is the load-bearing check, regardless of
which git commit the source provenance points at.

## Architecture

```
        ┌──────────────────────────────────────┐
        │  Proxy (BoundlessMarket, address P)  │
        │  delegate-calls active impl          │
        └──────────────┬───────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────┐
│  NEW market impl (src/BoundlessMarket.sol)   │
│                                              │
│  • declared selectors run here:              │
│      lockRequest, slash, withdraw,           │
│      submitRequest, deposit*, every view     │
│      getter shared with legacy, and the      │
│      new-shape fulfill(FulfillmentBatch[])   │
│                                              │
│  • everything else falls through:            │
│      fallback() → delegatecall(LEGACY_IMPL)  │
└──────────────────────┬───────────────────────┘
                       │ msg.sender, msg.value,
                       │ proxy storage all preserved
                       ▼
┌──────────────────────────────────────────────┐
│  LEGACY impl (src/legacy/                    │
│              BoundlessMarketLegacy.sol)      │
│                                              │
│  Audited deployed bytecode at the pre-       │
│  upgrade implementation address (Base        │
│  mainnet: 0x22bb...cd3).                     │
│                                              │
│  Reads + writes the same storage slots the   │
│  new market does (requestLocks at slot 0,    │
│  accounts at slot 1, imageUrl at slot 2).    │
└──────────────────────────────────────────────┘
```

## What's in here

| Path                                                | Role                                                                                                                                                                                 |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `BoundlessMarketLegacy.sol`                         | Frozen copy of `main`'s `BoundlessMarket`. Renamed file basename only; the contract symbol stays `BoundlessMarket` so deployedBytecode matches the audited deployment byte-for-byte. |
| `IBoundlessMarketLegacy.sol`                        | Frozen `IBoundlessMarket` interface (defines the legacy `Fulfillment[] + AssessorReceipt` shape, `imageInfo`, `verifyDelivery`, etc.).                                               |
| `IBoundlessMarketCallbackLegacy.sol`                | Frozen callback interface.                                                                                                                                                           |
| `libraries/{BoundlessMarketLib,MerkleProofish}.sol` | Frozen library deps.                                                                                                                                                                 |
| `types/*.sol`                                       | Frozen type tree (`Account`, `RequestLock`, `Fulfillment` with `id`+`requestDigest`, `AssessorReceipt`, etc.) the legacy contract was deployed against.                              |

File basenames are suffixed with `Legacy` so that forge writes artifacts to
distinct `out/` directories from the equivalents in `src/`. **Contract and
interface symbols are deliberately unchanged**: that preserves the
`bytecode_hash = none` build's byte-identical match against the deployed
audited code.

## Freeze policy

**Do not modify any file in this tree.**

The CI job `legacy-bytecode-parity` (in `.github/workflows/contracts.yml`)
runs `contracts/scripts/verify-legacy-bytecode.py`, which fails any PR whose
`legacy/` source no longer compiles to a byte-identical match of
`contracts/test/legacy/deployed-bytecode.hex` (the snapshot of the deployed
OLD impl) after masking the constructor-immutable byte positions. The
expected immutable values themselves are also re-checked against
`contracts/test/legacy/deployed-bytecode.meta.toml`.

## Storage layout interop

The `legacy-bytecode-parity` job also runs
`contracts/scripts/verify-storage-layout.py`, which asserts that every
storage slot reachable from both `src/BoundlessMarket` and
`src/legacy/BoundlessMarketLegacy` has the same layout (label, slot,
offset, normalized type, plus identical struct member layouts for `Account`
and `RequestLock`). If you're adding or modifying a struct in
`src/types/`, that script will catch any divergence from the legacy view
before it can corrupt delegate-call interop.
