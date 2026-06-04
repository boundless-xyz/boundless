# Assessor cost analysis: guest-based vs on-chain assessor

Comparing the total cost (on-chain gas + off-chain proving) of fulfilling proof
requests through the **guest-based assessor** (current production) vs the
**on-chain assessor** (PR 2005), on top of the router-decoupling + FulfillLib
contract stack (PRs 1982 → 2020 → fulfill-lib).

> **Branch:** `jonas/market-fulfill-onchain-assessor-bench` — the fulfill-lib
> branch with PR 2005's `OnChainAssessor` merged in, so every flow is measured
> against one identical base.

---

## TL;DR

1. **The current foundry benchmarks mock the verifier.** They measure market
   settlement + routing + set-inclusion only — **not** the ~250k-gas Groth16
   verification, and **nothing** off-chain. A single `fulfill` reads as 135k
   gas in the snapshot; the real on-chain cost is ~390–420k.
2. **On-chain, the two designs are ~equal *only in the guest's best case***
   (aggregated, 1 Groth16): within ~0.4% at batch 1, ~2% at batch 32 (on-chain
   assessor +1.5k gas/fill for predicate eval). But the guest's **dominant
   real-world path is *not* the best case** — see (4) — and its non-aggregated
   "2 Groth16" single costs ~635k vs the on-chain assessor's ~417k (**+52%**),
   because the on-chain assessor never emits an assessor Groth16.
3. **Off-chain (live `prod_base`, 30d): the assessor is *pure overhead* — the
   right lens is foregone proving capacity, not "% of total."** Assessor STARK
   p50 **3.3s** (Groth16, ~82% of volume) / **12s** (Merkle), p99 ~34s;
   assessor + its Groth16 compression averages **~13.6s/order**. The app proof
   is the *productive, paid* work (p50 ~150s all / ~752s Merkle); the assessor
   produces nothing salable. As a throughput tax on productive GPU time that's
   ~**9% at the median order**, ~2.5% on big proofs, and **45%+ on the ~10% of
   orders whose proof is <30s** — and batches are tiny (~63% single-order), so
   it's barely amortized. On a busy prover every one of those seconds is a proof
   not computed and not paid for.
4. **Path split (live, 30d): Groth16/individual 82%, Merkle/aggregated 12%.**
   The aggregated best case is the *minority*; most orders take the
   individual-Groth16 path where the on-chain assessor's on-chain advantage is
   largest.
5. **vs main: the new architecture is +42–55% on-chain** (router/adapter/
   FulfillLib indirection), dwarfing the guest-vs-on-chain-assessor delta. This
   is *preliminary* — measured via the frozen legacy bench as a main proxy; a
   proper baseline on this branch is still TODO (see Open items).
6. **Net:** on L2, the on-chain assessor is attractive — deletes a proving
   stage (~2% of compute) + the 2nd Groth16 in the dominant non-aggregated case
   (~250k gas) + simplifies the prover pipeline, for ~1.5k gas/fill. The
   off-chain saving is a few percent of proving, not a dominant cost.

---

## Part 0 — What the *current* benchmarks measure

The bench harness (`contracts/test/BoundlessMarket.t.sol:147`) wires a
`RiscZeroMockVerifier`, wrapped by a **real** `RiscZeroSetVerifier`
(`:154`). So the committed `BoundlessMarketBench.json` numbers capture:

| Captured ✅ | Omitted ❌ |
|---|---|
| Market settlement (pay/credit/refund) | Real STARK→Groth16 verification (mock ≈ 0 gas) |
| Router dispatch to adapters | All off-chain proving (app, assessor, set-builder, compression) |
| Real set-inclusion Merkle verification | |
| Assessor-adapter reconstruction (R0) / ECDSA + predicate (on-chain) | |
| Callback execution | |

**Cross-check that ~250k is hidden:** the broker's own cost model
(`crates/boundless-market/src/prover_utils/config.rs:54-73`) sets
`fulfill_gas_estimate() = 420_000` ("observed ~390k single fulfill") and
`groth16_verify_gas_estimate() = 250_000`. Our measured best-case settlement
(168,784) + 250k Groth16 = **418,784**, landing right on the broker's anchor.
That validates the analytical layer used throughout this doc.

---

## Part 1 — The flows and where cost goes

The assessor is a **guest** (zkVM) program, not an on-chain contract, in the
production path. The broker's finalize path
(`crates/broker/src/aggregator/service.rs:617-686`) proves the assessor STARK,
**chains it into the set-builder tree alongside the app proofs**, aggregates,
and compresses the whole root to **one Groth16**.

### Flow A — Guest-based, best case (`submitRootAndFulfill`)

```
off-chain:  N app STARKs ─┐
                          ├─► set-builder STARK ─► 1 Groth16 (root)
            assessor STARK ┘   (~270k cyc)         (~250k gas on-chain)
            (~2M cyc, Path B)
on-chain:   submitMerkleRoot(root, groth16Seal)   ← 1 Groth16 verify
            + N Merkle inclusion proofs (cheap hashing)
            + 1 assessor Merkle inclusion
            + settlement
```
One Groth16 covers app proofs **and** the assessor. This answers the
"(assessor still via set builder?)" question: **yes — the assessor proof is a
leaf in the set-builder tree and is verified by the single root Groth16.**

### Flow B — Guest-based, "2 Groth16" (non-aggregated)

When aggregation is skipped (Path A, `proof_type ∈ {Groth16, Blake3Groth16}`),
the app proof is compressed and submitted individually as its own Groth16, and
the assessor is verified as a separate Groth16 → **2 Groth16 verifications
on-chain** (~500k gas), with no set-builder aggregation off-chain. Higher
on-chain cost, used for single/urgent requests where aggregating isn't worth
the latency.

### Flow C — On-chain assessor (PR 2005)

```
off-chain:  N app STARKs ─► (set-builder STARK ─► 1 Groth16 (app root))
            NO assessor STARK, NO assessor leaf
on-chain:   submitMerkleRoot(appRoot, groth16Seal)  ← 1 Groth16 verify
            + N Merkle inclusion proofs
            + OnChainAssessor: 1 EIP-712 ECDSA (batch) + N predicate evals
            + settlement
```
The assessor work moves on-chain: one prover ECDSA signature over
`(prover, requestDigests[], claimDigests[])` + per-fill predicate evaluation.
**No assessor STARK, no assessor Groth16, no assessor leaf.**

### Per-component attribution

| Component | Flow A (guest best) | Flow B (2×Groth16) | Flow C (on-chain) |
|---|---|---|---|
| On-chain Groth16 verifies | 1 (covers app+assessor) | 2 | 1 (app root) |
| On-chain assessor cost | Merkle inclusion (~0) | Groth16 (250k) | ECDSA + N·predicate (~1.5k/fill) |
| Off-chain assessor STARK | ~2M cyc (always) | ~2M cyc | **0** |
| Off-chain set-builder STARK | ~270k cyc | 0 | ~270k cyc (app only) |
| Off-chain Groth16 compression | 1 | per-proof | 1 |

---

## Part 2 — Measured on-chain results

Settlement gas, **mock verifier** (Groth16 excluded), combined branch,
`forge test --isolate`. Regenerate via the commands in *Reproduce* below.

| batch | guest `fulfill`¹ | guest `submitRootAndFulfill`² (best case) | `fulfill (on-chain assessor)`³ |
|---:|---:|---:|---:|
| 1 | 135,330 | 168,784 | 133,522 |
| 2 | 191,911 | 225,378 | 191,459 |
| 4 | 307,381 | 340,838 | 309,823 |
| 8 | 529,310 | 562,783 | 537,741 |
| 16 | 976,792 | 1,010,272 | 997,756 |
| 32 | 1,910,376 | 1,943,858 | 1,956,084 |

¹ root pre-submitted in a separate (unmeasured) tx · ² root + fulfill in one tx
· ³ root pre-submitted separately, native assessor

**Marginal cost per fill** (slope, 1→32): guest `fulfill` ≈ **57.3k/fill**;
on-chain assessor ≈ **58.8k/fill** → the on-chain assessor adds **~1.5k
gas/fill** (per-fill predicate eval + claim-digest reconstruction).

**Root-submission overhead** (`submitRootAndFulfill − fulfill`, guest): a
near-constant **~33.5k gas** regardless of batch (one set-verifier root write
per batch; Groth16 still mocked).

---

## Part 3 — Realistic on-chain cost (analytical Groth16 layer)

Add the verification gas the mock omits (`groth16_verify_gas_estimate = 250k`
per Groth16). All flows verify the app-proof root with **one** Groth16; the
guest path's assessor is inside that same root, the on-chain path adds none.

| Scenario (single, batch=1) | settlement | + Groth16 | **real on-chain** |
|---|---:|---:|---:|
| Guest best case (`submitRootAndFulfill`) | 168,784 | +250k ×1 | **418,784** |
| Guest "2 Groth16" (non-aggregated) | 135,330 | +250k ×2 | **635,330** |
| On-chain assessor (root+fulfill, +33.5k) | 166,976 | +250k ×1 | **416,976** |

| Scenario (batch=32, root+fulfill basis) | real on-chain | per fill |
|---|---:|---:|
| Guest best case | 1,943,858 + 250k = **2,193,858** | 68,558 |
| On-chain assessor | 1,989,566 + 250k = **2,239,566** | 69,986 |

**On-chain takeaways**
- Best-case guest vs on-chain assessor differ by **−0.4%** at batch 1 (on-chain
  cheaper) and **+2.1%** at batch 32 (guest cheaper). Effectively a wash.
- The non-aggregated "2 Groth16" path is **~52% more expensive** on-chain than
  the aggregated best case — this is *why* the set builder exists.

---

## Part 4 — Off-chain proving cost

Only the **guest** path pays this; the on-chain assessor pays ~0 for assessment.
It is real, but — see the live numbers below — **small relative to the app
proof** it rides alongside.

**Documented cycle costs** (`prover_utils/config.rs:75-78`,
`additional_proof_cycles()`): **assessor ≈ 2,000,000 cycles**, **set builder ≈
270,000 cycles** (per batch; assessor also grows with the number of ECDSA
signatures — the assessor lib notes each verify is ~1M cycles,
`crates/assessor/src/lib.rs:100`).

Proving time = `cycles / (peak_prove_khz × 1000)` s. Sweep over prover speed
(`peak_prove_khz` config examples: 100–999):

| peak_prove_khz | assessor STARK (2M cyc) | set-builder (270k cyc) | assessor+sb |
|---:|---:|---:|---:|
| 100 | ~20.0 s | ~2.7 s | ~22.7 s |
| 250 | ~8.0 s | ~1.1 s | ~9.1 s |
| 500 | ~4.0 s | ~0.5 s | ~4.5 s |
| 1000 | ~2.0 s | ~0.3 s | ~2.3 s |

The model lands in the right ballpark, but **measure, don't model** — live
numbers below.

### Live telemetry — `prod_base` (Redshift, last 30 days)

Run via `contracts/benchmarks/run-telemetry.py` (or `telemetry-queries.sql`).
Source: `telemetry.request_completions`, `outcome='Fulfilled'`.

**Path split** — the aggregated "best case" is the minority:

| proof_type | completions | share |
|---|---:|---:|
| Groth16 (individual) | 31,317 | ~82% |
| Merkle (aggregated / set-builder) | 4,747 | ~12% |
| Unknown | 2,167 | ~6% |

> Note: `assessor_proving_secs` / `set_builder_proving_secs` are populated on
> **both** paths in current prod — the schema's "Path B only" comment is stale.

**Assessor STARK proving** (the cost the on-chain assessor removes):

| proof_type | n | p50 | p90 | mean |
|---|---:|---:|---:|---:|
| Groth16 (~82% of volume) | 31,317 | **3.3 s** | 14.6 s | 8.2 s |
| Merkle | 4,747 | **12.0 s** | 16.5 s | 10.8 s |

p99 ≈ 34 s. The "~10–30 s" intuition is the **tail / aggregated** case; the
common (Groth16, `LockAndFulfill`) case is ~**3.3 s median**.

**Other per-batch off-chain components** (Merkle path): `set_builder` mean
~10.4 s (bimodal, mode ~15 s — wall-clock likely includes the aggregation
window, so it overstates pure GPU); `assessor_compression` mean ~6.7 s.

**The assessor is pure overhead — measure it as opportunity cost, not as a
fraction of total.** The app proof is the productive, paid work; every second
on the assessor is GPU capacity that produced no salable proof. App
`stark_proving_secs` (all fulfilled): p10 **29 s**, p25 68 s, p50 **150 s**,
p75 553 s (Merkle skews larger, p50 ~752 s). Avg assessor + its Groth16
compression = **13.6 s/order**. As a tax on *productive* GPU time:

| app-proof size (order) | assessor overhead tax |
|---|---:|
| p75 (~553 s) | ~2.5 % |
| p50 (~150 s) | ~**9 %** |
| <30 s (bottom ~10 %: 3,719 / 36,421 orders) | **45 %+** (nearly doubles the work) |

On a capacity-constrained prover this is foregone throughput — proofs it
couldn't compute and couldn't get paid for — worst exactly on the small/cheap
jobs where margins are thinnest.

**Batches are tiny** (orders sharing one aggregation, Merkle, 30d): ~63 %
single-order; sizes 1–3 dominate. So the assessor STARK is barely amortized —
effectively a **per-order** overhead, not spread over a big N.

---

## Part 5 — Total cost model & crossover

Per-fill total cost, comparing the two designs (app proving + app-root Groth16
are common to both and cancel):

```
guest_extra_per_fill   = assessor_offchain_$ / N           (off-chain, shrinks with N)
onchain_extra_per_fill = ~1,500 gas × gas_price × eth_$     (on-chain, constant per fill)

where assessor_offchain_$ = (assessor_secs + setbuilder_secs) × prover_$_per_sec
```

- **Guest assessor is cheaper per fill when** `assessor_offchain_$ / N` <
  `1,500 × gas_price × eth_$` — i.e. **large batches and/or expensive gas**.
- **On-chain assessor is cheaper when** the amortized off-chain assessor cost
  exceeds ~1,500 gas-equivalent per fill — i.e. **small batches and/or cheap
  L2 gas**, where eliminating a multi-second STARK beats 1.5k gas.

Worked example with live numbers (prover_$_per_sec ≈ $10/hr ÷ 3600 ≈
**$0.0028/s**, ETH = $3,000):

- **Off-chain saving (on-chain assessor avoids it):** assessor ~3.3 s (common
  Groth16 case) to ~19 s (Merkle assessor+compression) ⇒ **$0.009–0.053 per
  order** (barely amortized — batches are ~size 1–3).
- **On-chain cost of the on-chain assessor, best-case aggregated path:**
  +1,500 gas/fill ⇒ ~$0.0000002/fill (L2 @ 0.05 gwei) or ~$0.135/fill
  (L1 @ 30 gwei).
- **On-chain *saving* of the on-chain assessor, dominant non-aggregated path:**
  it drops the 2nd Groth16 (~250k gas) ⇒ ~$0.0000375/order (L2) or **~$22.5/
  order (L1)**.

So:
- **L2 (where the marketplace runs):** on-chain assessor wins decisively — it
  saves the off-chain assessor stage ($0.009–0.05/order) and, on the ~82%
  non-aggregated path, a whole Groth16, for ~1.5k gas/fill (≈ nothing).
- **L1:** in the rare best-case aggregated batch, the guest's 1,500-gas/fill
  edge can favor it; but on the dominant non-aggregated path the on-chain
  assessor still saves a full ~250k-gas Groth16 (~$22/order), so it wins there
  too.

> Bottom line: across the real path mix the on-chain assessor is net cheaper on
> three fronts — (a) it **drops the 2nd Groth16 on the dominant non-aggregated
> path** (~250k gas), (b) it **recovers ~13.6 s/order of pure-overhead proving
> capacity** (a ~9% throughput tax at the median order, 45%+ on small proofs),
> and (c) it **deletes an entire proving stage** (assessor STARK + its
> compression), simplifying the pipeline — for ~1.5k gas/fill on L2. Plug in the
> target chain's gas price and the prover's $/proving-second to put dollars on it.

---

## Part 6 — vs main (⚠️ preliminary)

The biggest on-chain delta isn't the assessor choice — it's the new
architecture itself. Measured via the committed **legacy bench** (the frozen
`contracts/src/legacy/` market, byte-identical to the deployed main market per
the parity CI, run under the *same* harness):

| batch | main/legacy `fulfill` | new guest `fulfill` | delta |
|---:|---:|---:|---:|
| 1 | 87,281 | 135,330 | **+55%** |
| 8 | 370,240 | 529,310 | +43% |
| 32 | 1,343,691 | 1,910,376 | **+42%** |

Per-fill **40,427 → 57,153 (+16,727)**, fixed **+28,201** — the router dispatch
+ per-fill adapter try/catch + FulfillLib delegatecall + slim-request
reconstruction. Real on-chain (+250k Groth16 both sides): batch-1 ~337k → ~417k
(**+24%**), batch-32 ~1.59M → ~2.19M (**+38%**). The legacy ABI via the new
market's `fallback` adds ~0 (≤+2.2k single, *cheaper* at scale).

> **This is a proxy, not a real baseline.** TODO: measure main properly using
> this branch's bench harness (the legacy bench is the frozen audited impl, not
> a literal `main` checkout with its own tests).

---

## Reproduce

```bash
# combined branch (already created): fulfill-lib + PR 2005 OnChainAssessor
git switch jonas/market-fulfill-onchain-assessor-bench

# regenerate the bench snapshots (FORGE_SNAPSHOT_CHECK must be unset to write)
forge test --isolate --match-contract '^BoundlessMarketBench$'
forge test --isolate --match-contract '^BoundlessMarketBasicTest$'

# verify benches are deterministic against committed snapshots
FORGE_SNAPSHOT_CHECK=true forge test --isolate --match-contract '^BoundlessMarketBench$'

# off-chain proving numbers (psql not required; reads creds from network_secrets.toml)
uv run --with 'psycopg[binary]' --with duckdb --with boto3 \
    contracts/benchmarks/run-telemetry.py prod_base
# (live numbers in Part 4 are from this, prod_base, last 30 days)
```

New benches: `benchSubmitRootAndFulfill` (guest best case, R0) and
`benchFulfillOnChainAssessor` (PR 2005), batch sizes 1–32, keys
`submitRootAndFulfill: batch of NNN:v2` and
`fulfill (on-chain assessor): batch of NNN:v2` in `BoundlessMarketBench.json`.

## Open items

- **Proper main baseline (Part 6) is TODO.** The +42–55% figure is via the
  frozen legacy bench as a proxy; measure `main` itself with this branch's
  harness to confirm.
- **`prover_$_per_sec`** and **target-chain gas price** are still model
  parameters — plug in real values for the deployment chain to turn the
  throughput tax and the dropped-Groth16 saving into dollars.
- Telemetry is `prod_base`, last 30 days. Re-run `run-telemetry.py` for other
  chains (`prod_taiko`, …) or a longer window (pre-2026-04-25 uses the S3
  archive path) if you want a wider view.
- The on-chain "2 Groth16" path (Flow B) is analytical only — the mock verifier
  makes a committed on-chain snapshot for it misleading (it would look like a
  plain batch-of-1 fulfill).
- Off-chain seconds are **wall-clock** around the prove calls; `set_builder` is
  bimodal (mode ~15 s) and may include aggregation-window wait, so it overstates
  exclusive GPU time. The assessor STARK itself is real GPU compute.
