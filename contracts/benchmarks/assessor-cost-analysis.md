# Assessor cost analysis: guest-based vs on-chain assessor

Comparing the total cost (on-chain gas + off-chain proving) of fulfilling proof
requests through the **guest-based assessor** (current production) vs the
**on-chain assessor** ([PR #2005](https://github.com/boundless-xyz/boundless/pull/2005)),
on top of the router-decoupling + FulfillLib contract stack
([#1982](https://github.com/boundless-xyz/boundless/pull/1982) →
[#2020](https://github.com/boundless-xyz/boundless/pull/2020) →
[#2022](https://github.com/boundless-xyz/boundless/pull/2022)).

> **Branch:** `jonas/market-fulfill-onchain-assessor-bench` — the fulfill-lib
> branch with PR #2005's `OnChainAssessor` merged in, so every flow is measured
> against one identical base, same harness.

---

## TL;DR

1. **Key finding — for the dominant production workload the on-chain assessor is
   materially cheaper.** That workload is single, explicit-Groth16 requests
   (~82% of orders over the last 30 days). Single proof, real on-chain: the
   on-chain assessor is **~44% less gas** than the guest's current path
   (**373,891 vs ~668,784**), and it deletes the assessor STARK off-chain entirely.
2. **The cost driver is the Groth16 count + the mandatory `submitRoot`.** The
   guest assessor's STARK is *currently* only submitted as a set-builder
   inclusion, so **the guest always submits a root**; on-chain that's 1 Groth16
   (best case) or **2 Groth16 (worst case = explicit Groth16)** plus that root.
   The on-chain assessor is **1 Groth16, no root**, always.
3. **Off-chain the assessor is pure overhead** (no salable proof). Real
   `prod_base` (30d): assessor STARK p50 3.3s (Groth16 path) / 12s (Merkle),
   assessor + compression ~13.6s/order — a ~9% throughput tax at the median
   order, 45%+ on small proofs. The on-chain assessor recovers all of it.
4. **vs main, the new architecture's overhead is localized to fulfillment.** The
   directly-comparable `submitRootAndFulfill` (single, R0 assessor both sides)
   goes **120,209 → 168,784 (+48,575, +40%** settlement; **~+13% real)**.
   Request/lock/slash/deposit/withdraw are **neutral (±1k gas)**.
5. the new-architecture on-chain assessor (373,891 single, real) lands right at the old architecture's guest best case (legacy 370,209) — moving the assessor on-chain roughly buys back the router/FulfillLib fulfillment overhead.

---

## Where the on-chain cost comes from  ⭐

- **The assessor is, today, always verified as a set-builder inclusion.**
  `R0BoundlessAssessorAdapter` is pinned to `RiscZeroSetVerifier`
  (`R0BoundlessAssessorAdapter.sol:65-67`; `verifyAssessor` →
  `RISC_ZERO_VERIFIER.verify(innerSeal, ASSESSOR_IMAGE_ID, journalDigest)` at
  `:162`), and the broker always proves the assessor and chains it into the set
  builder on finalize (`aggregator/service.rs`; live telemetry shows even
  Groth16-path orders carry `set_builder_proving_secs` + `assessor_proving_secs`).
  So **the guest flow always does a `submitRoot`** for the assessor. This isn't
  fundamental — the assessor STARK *could* instead be compressed to its own direct
  Groth16 with no root — but that's not how it works today, so today the root is
  mandatory.
- **Whether the app proof can share that root depends on the request.** If the
  request accepts inclusion, the app proof rides the *same* root as the assessor
  → 1 Groth16 total. If the request wants an **explicit on-chain Groth16** (a
  specific verifier selector — the ~82% case), the app proof must be its own
  standalone Groth16, so the set builder can't fold it in with the assessor → the
  assessor needs its **own** root → **2 Groth16**.

| flow | app proof | assessor | **on-chain crypto** |
|---|---|---|---|
| **Guest — best case** (inclusion ok) | leaf in the set root | leaf in the **same** root | **1 Groth16 + 1 submitRoot** |
| **Guest — worst case** (explicit Groth16, ~82%) | its own direct Groth16 | leaf in its **own** root | **2 Groth16 + 1 submitRoot** |
| **On-chain assessor** | its own direct Groth16 | on-chain ECDSA + predicate | **1 Groth16, no root** |

So the on-chain assessor always saves **the assessor's set-builder root** (the
`submitRoot` ~33k SSTORE + the off-chain STARK) and, on the explicit-Groth16 path,
**a whole Groth16 (~250k)** on top.

### Single proof, real on-chain (settlement + Groth16)

| flow | Groth16 | root? | real on-chain | vs on-chain |
|---|:---:|:---:|---:|---:|
| Guest best case | 1 | yes | 418,784 | +12% |
| **Guest worst case (explicit Groth16, production)** | 2 | yes | **~668,784** | **+79%** |
| **On-chain assessor** | 1 | no | **373,891** | — |

(Guest settlement 168,784 measured; worst case adds the 2nd Groth16 analytically.
On-chain 123,891 measured. +250k per Groth16.) *Batch caveat:* for genuinely
*aggregatable* batches the guest amortizes to one Groth16 while the no-root
on-chain path pays N — so the guest wins there. But batches are tiny in production
(~63% single-order) and ~82% are explicit/individual anyway.

---

## Part 0 — What the *current* benchmarks measure

The bench harness (`BoundlessMarket.t.sol`) wires a `RiscZeroMockVerifier`
wrapped by a **real** `RiscZeroSetVerifier`. So `BoundlessMarketBench.json`
captures:

| Captured ✅ | Omitted ❌ |
|---|---|
| Settlement (pay/credit/refund) | Real STARK→Groth16 verification (mock ≈ 0 gas) |
| Router dispatch to adapters | All off-chain proving |
| Real set-inclusion Merkle verification | |
| Assessor reconstruction (R0) / ECDSA + predicate (on-chain) | |

**Cross-check:** broker model `fulfill_gas_estimate = 420_000` ("observed ~390k"),
`groth16_verify_gas_estimate = 250_000`. Guest best-case settlement 168,784 + 250k
= **418,784** — on the broker's anchor. Validates the analytical layer.

---

## Part 1 — The flows

The assessor is a **guest** (zkVM) program in production; on finalize the broker
proves it and aggregates it into the set-builder tree with the app proofs, then
compresses the root to a Groth16 (`aggregator/service.rs`). The **on-chain
assessor** (#2005) replaces that STARK with an EIP-712 ECDSA prover signature +
per-fill predicate eval, entirely on-chain — no STARK, no aggregation, no root for
the assessor.

The assessor choice (guest vs on-chain) and the app-proof verification (inclusion
vs explicit Groth16) are **independent axes**; the ⭐ table enumerates the
combinations that matter. Code refs: `BoundlessMarket.sol` (`submitRootAndFulfill`,
`fulfill`), `router/BoundlessRouter.sol`,
`router/adapters/{R0BoundlessAssessorAdapter,OnChainAssessor}.sol`.

---

## Part 2 — Measured settlement (mock verifier; Groth16 excluded)

Combined branch, `forge test --isolate`. Regenerate per *Reproduce*.

| batch | guest `submitRootAndFulfill`¹ (best) | on-chain `fulfill` (no root)² |
|---:|---:|---:|
| 1 | 168,784 | 123,891 |
| 2 | 225,378 | 174,543 |
| 4 | 340,838 | 276,121 |
| 8 | 562,783 | 465,973 |
| 16 | 1,010,272 | 840,263 |
| 32 | 1,943,858 | 1,605,632 |

¹ root + fulfill in one tx — the real guest flow (root is mandatory) ·
² on-chain assessor, no root, app proof via NullVerifier (mock for a direct Groth16)

- **On-chain (no root) vs guest best case:** −27% at batch 1 (123,891 vs 168,784),
  −17% at batch 32. The gap ≈ the `submitRoot` SSTORE (~33k) the guest can't avoid
  + the per-fill inclusion hashing the no-root path skips.
- Per-fill settlement: guest best ~57.2k/fill, on-chain (no root) ~47.8k/fill.

---

## Part 3 — Off-chain proving cost (pure overhead)

Only the guest pays this; the on-chain assessor pays ~0 for assessment. The right
lens is **foregone proving capacity** — the app proof is the productive, paid work;
the assessor produces nothing salable, so every second on it is a proof the prover
couldn't compute. Live `prod_base`, 30d (`telemetry.request_completions`):

**Assessor STARK:**

| proof_type | n | p50 | p90 | mean |
|---|---:|---:|---:|---:|
| Groth16 (~82% of volume) | 31,317 | **3.3 s** | 14.6 s | 8.2 s |
| Merkle | 4,747 | **12.0 s** | 16.5 s | 10.8 s |

p99 ≈ 34 s. Assessor + its Groth16 compression averages **~13.6 s/order**. App
`stark_proving_secs`: p10 29 s, p50 **150 s**, p75 553 s. As a tax on productive
GPU time:

| app-proof size (order) | assessor overhead tax |
|---|---:|
| p75 (~553 s) | ~2.5 % |
| p50 (~150 s) | ~**9 %** |
| <30 s (bottom ~10 %) | **45 %+** (nearly doubles the work) |

Path split (30d): Groth16/individual **82%**, Merkle/aggregated **12%**. Batches
tiny (~63% single-order), so the assessor is barely amortized — effectively a
per-order overhead. *(Caveat: wall-clock around the prove calls; `set_builder` is
bimodal, ~15s mode, likely including aggregation-window wait, so it overstates
exclusive GPU.)*

---

## Part 4 — Bottom line

The relevant comparison is the **worst case (explicit-Groth16 requests)** — that's
the actual production workload (~82% of orders). There the on-chain assessor wins
on every axis:
- **On-chain: −44% vs the guest's current path** (drops a Groth16 + the
  `submitRoot`); −12% even vs the inclusion-friendly best case.
- **Off-chain:** deletes the assessor STARK + set-builder + compression
  (~13.6 s/order of pure-overhead proving; ~9% throughput tax at the median, far
  more on small jobs).
- **Pipeline:** removes an entire proving stage and the root-submission tx.

---

## Part 5 — Overhead of the new implementation vs main

How much does the new stack (router + adapters + FulfillLib) cost vs the previous
monolithic market, across the whole operation surface? Baseline =
`BoundlessMarketLegacy{Bench,BasicTest}` — the frozen `src/legacy/` market
(byte-identical to the deployed main market per CI) run under the **identical**
harness, with a matching `benchSubmitRootAndFulfill` added so settlement entrypoints
line up. All same-harness, regenerated on this branch with zero drift.

**The overhead is entirely localized to the fulfillment path; everything else is
neutral (±1k gas)** — request/lock/slash/account ops don't touch the router or
FulfillLib:

| operation | main | new | Δ gas |
|---|---:|---:|---:|
| submitRequest | 45,914 | 45,638 | −276 |
| lockRequest | 147,046 | 147,687 | +641 |
| slash | 101,033 | 101,114 | +81 |
| deposit / withdraw | 50,942 / 40,358 | 50,671 / 40,226 | −271 / −132 |
| depositCollateral / withdrawCollateral | 59,403 / 69,140 | 58,908 / 68,938 | −495 / −202 |

The line that matters is **`submitRootAndFulfill`** — the same entrypoint *and* the
same R0 guest assessor as the options measured above, so it compares directly:

| | main | new | Δ gas | Δ% |
|---|---:|---:|---:|---:|
| **➤ submitRootAndFulfill (single, R0)** | **120,209** | **168,784** | **+48,575** | **+40%** |

**In real terms** (+ the common 250k Groth16): **370,209 → 418,784, +13%**
(batch-32: 1.63M → 2.19M, +35%).

> Measured with the bench (R0 assessor on both sides). The single-op *BasicTest*
> variant stubs a `NullAssessor` on the new side (reads ~154k) and **understates**
> the overhead — don't use it for this comparison.

**Where it comes from:** almost entirely the **PR #1982 router-decoupling**, not
this branch's FulfillLib extraction (gas-neutral: +~400 gas single, cheaper on
batches). Two components: (1) **router framing** — `RouterBench` measures ~22k at
N=1 (router vs a hardcoded direct call), of which **~18k is cold registry SLOADs**
to resolve the verifier + assessor adapter entries, the rest selector resolution +
the per-fill verifier STATICCALL + per-batch assessor STATICCALL; (2) the **R0
assessor adapter's on-chain reconstruction** (~7.7k/fill, `AdapterBench`) + the
slim-request EIP-712 binding — a calldata-for-compute tradeoff. (`forge test
--match-contract 'RouterBench|AdapterBench' -vv`.)

**Not a regression elsewhere:** new market impl bytecode is slightly *smaller*
(24,026 vs 24,371), and the legacy ABI via the new market's `fallback` adds ~0
(≤+2.2k single, cheaper at scale) — migrating brokers see no meaningful penalty.

> Notable: the new-architecture on-chain **assessor** (373,891 single, real) lands
> right at the *old* architecture's guest best case (legacy 370,209) — moving the
> assessor on-chain roughly buys back the router/FulfillLib fulfillment overhead.

---

## Reproduce

```bash
git switch jonas/market-fulfill-onchain-assessor-bench
# regenerate bench snapshots (FORGE_SNAPSHOT_CHECK unset to write); full contract, no --match-test
forge test --isolate --match-contract '^BoundlessMarketBench$'
forge test --isolate --match-contract '^BoundlessMarketLegacyBench$'
# off-chain proving numbers (psql-free; reads network_secrets.toml)
uv run --with 'psycopg[binary]' --with duckdb --with boto3 \
    contracts/benchmarks/run-telemetry.py prod_base
```

Benches in `BoundlessMarketBench`: `benchSubmitRootAndFulfill` (guest best),
`benchFulfillOnChainAssessor` (on-chain, no root), plus the legacy
`benchSubmitRootAndFulfill`. Keys in `BoundlessMarketBench.json` /
`BoundlessMarketLegacyBench.json`.
