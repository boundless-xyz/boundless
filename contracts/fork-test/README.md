# Reproduce the sunset upgrades

Use both feature branches, checked out as sibling directories `market/` and `zkc/`.
The ZKC branch is `codex/povw-sunset`; the market branch is `codex/protocol-sunset`.
These instructions need no files from the original author's preparation directory.

## Setup and unit tests

Use Foundry 1.8.6, solc 0.8.26, Python 3.10 or newer, Node 22 and npm.
The fork runners need archive RPC access. No credentials or signing keys are needed.
From the directory containing both checkouts:

```bash
set -euo pipefail
export FORGE_SNAPSHOT_EMIT=false
export FOUNDRY_SOLC=0.8.26
export FOUNDRY_EVM_VERSION=cancun
export PYTHONDONTWRITEBYTECODE=1
export MAINNET_RPC_URL=
git -C market submodule update --init --recursive
git -C zkc submodule update --init --recursive
(cd market && forge build)
(cd zkc && forge build)
(cd market && forge test --offline)
(cd market && FOUNDRY_PROFILE=shanghai FOUNDRY_EVM_VERSION=shanghai forge test --offline)
(cd zkc && forge test --offline)
(cd market && python3 -m unittest discover -s contracts/scripts -p 'test_write_safe_batch.py')
(cd market && python3 -m unittest discover -s contracts/fork-test -p 'test_*.py')
```

The ordinary ZKC run skips the existing recovery fork suite without `MAINNET_RPC_URL`.
Run its three cases at the historical block documented in [the ZKC runbook](../../../zkc/script/README.md#sunset-verification).
Disable gas snapshot emission to preserve tracked snapshots. Do not use deployment secrets.

## Upgrade validation

Build references from the deployed source revisions with their pinned submodules.
From the parent directory, create disposable detached checkouts. The market revision
has the same contract sources as the deployed builds; the comparisons recorded with
the rehearsal confirm the executable bytecode after resolving constructor immutables.
ZKC uses the deployment commits from `deployment.toml`.

```bash
git clone --no-checkout market market-reference
git -C market-reference checkout --detach 93e971a653258bb2cf1de920b103fcfdfd9ef994
git -C market-reference submodule update --init --recursive
for spec in "base default cancun contracts/src/BoundlessMarket.sol" \
            "taiko shanghai shanghai contracts/shanghai/variants/BoundlessMarket.sol" \
            "povw-staging default cancun contracts/src/povw/PovwAccounting.sol contracts/src/povw/PovwMint.sol" \
            "povw-production povw-deploy cancun contracts/src/povw/PovwAccounting.sol contracts/src/povw/PovwMint.sol"; do
    read -r -a args <<< "$spec"
    label=${args[0]}; profile=${args[1]}; evm=${args[2]}
    (cd market-reference && FOUNDRY_PROFILE="$profile" FOUNDRY_EVM_VERSION="$evm" \
        FOUNDRY_AST=true FOUNDRY_BUILD_INFO_PATH="out/reference-$label/build-info" \
        forge build "${args[@]:3}" --out "out/reference-$label" --cache-path "cache/reference-$label")
done
mkdir -p market/contracts/build-info-reference
for label in base taiko povw-staging; do
    cp market-reference/out/reference-$label/build-info/*.json market/contracts/build-info-reference/$label.json
done
for commit in 5b8310b 99f8f78; do
    git clone --no-checkout zkc "zkc-reference-$commit"
    git -C "zkc-reference-$commit" checkout --detach "$commit"
    git -C "zkc-reference-$commit" submodule update --init --recursive
    (cd "zkc-reference-$commit" && FOUNDRY_PROFILE=default FOUNDRY_AST=true \
        FOUNDRY_BUILD_INFO_PATH=out/reference/build-info \
        forge build src/ZKC.sol --out out/reference --cache-path cache/reference)
    mkdir -p "zkc/build-info-reference-$commit"
    cp "zkc-reference-$commit"/out/reference/build-info/*.json "zkc/build-info-reference-$commit/"
done
```

Run this block in Bash, with the setup exports above. Use empty reference output folders;
old incremental build-info files can make contract names ambiguous. Staging PoVW used
100 optimizer runs; production used one million. Base and Taiko require separate Cancun
and Shanghai references. OpenZeppelin validates storage and upgrade safety inside each
upgrade script, with no unsafe flags. The rehearsals also check full candidate runtime bytes.

## Fork rehearsals

The existing single-repo runners deploy fresh candidates inside Foundry forks:

```bash
(cd market && bash contracts/fork-test/run.sh all)
(cd zkc && bash fork-test/run.sh)
```

The shared rehearsal below starts and closes its own Anvil forks. It deploys candidates
through `UpgradeBoundlessMarket`, `UpgradePoVWAccounting`, `UpgradePoVWMint` and
`UpgradeZKC`, then uses the existing `write_safe_batch.py`. It checks all runtime bytes,
constructor dependencies, baseline hashes, Safe permissions, thresholds and nonce.
It runs the supplied-candidate suites, including the atomic Ethereum batch and rollback,
then executes the same exported bytes on the parent Anvil with locally impersonated owners.

```bash
python3 market/contracts/fork-test/rehearse.py --environment staging --zkc-repo zkc --output market/out/handoff/staging
rm market/contracts/build-info-reference/povw-staging.json
cp market-reference/out/reference-povw-production/build-info/*.json market/contracts/build-info-reference/povw-production.json
python3 market/contracts/fork-test/rehearse.py --environment production --zkc-repo zkc --output market/out/handoff/production
```

All six rehearsals must report `passed`, including exact exported rollback execution
before and after the cutoff. Ethereum upgrades
Accounting, Mint and ZKC atomically in that order, with no initializer. Base and Taiko
upgrade their markets separately. Staging Taiko is chain 167000 and shares the production
Safe and nonce. Sepolia uses its own 3-of-5 Safe; the other Safes require 3-of-6.

`baselines.json` pins ten proxy baselines, full runtime hashes, dependencies, snapshot
blocks and candidate bytecode templates. The scripts rebuild candidates; they do not
accept arbitrary replacement artifacts. Override the default read-only endpoints with
`BASE_RPC_URL`, `ETHEREUM_RPC_URL`, `TAIKO_RPC_URL`, `BASE_SEPOLIA_RPC_URL` or
`ETHEREUM_SEPOLIA_RPC_URL` if an archive endpoint is unavailable. `--chain` can select
`base`, `taiko` or `ethereum`. Rehearsals use historical snapshots even after the cutoff.

The execution rehearsals set `SKIP_SAFETY_CHECKS=false` and enable FFI for OpenZeppelin
validation. Missing or incompatible reference builds stop candidate deployment.
Proof verifiers are mocked where needed; these checks do not prove signer availability
or exhaust every possible contract state. The suites cover cutoff boundaries, preserved
passive staking rewards and withdrawals, permissions, initialization and failed upgrades.

## Safe drafts and verification results

Keep the generated `out/handoff/` results with the two source revisions. The directory contains
Safe JSON, full calldata, baseline and candidate hashes, tool versions, submodule pins,
Anvil execution results and test logs. It is useful evidence for a reviewer. Candidate
addresses exist only on the disposable forks. Rerun the rehearsal from its recorded
source revisions to reproduce the checks. Saved JSON cannot restore fork state.

Do not submit rehearsal JSON to a live Safe. After approved candidate deployment,
regenerate the unsigned draft against current chain state. `prepare.py` performs RPC
reads only, even for loopback endpoints. It checks supplied candidate runtimes against
locally rebuilt artifacts and pins every read to one block. Draft preparation requires
a snapshot before the cutoff for upgrades. For example, from `market/`:

```bash
python3 contracts/fork-test/prepare.py --environment staging --chain ethereum --zkc-repo ../zkc --candidate accounting=0xACCOUNTING --candidate mint=0xMINT --candidate zkc=0xZKC --output out/staging-draft/transaction.json
```

Replace the three placeholders with deployed addresses. Use `--environment production`
for production, or `--chain base --candidate base=ADDRESS` / `--chain taiko --candidate taiko=ADDRESS`.
The artifacts under `out/script-rehearsal-*` come from the rehearsal above.
The command exports the exact Safe transaction and a Transaction Builder file, but does
not collect signatures or submit anything. Refresh after any intervening Safe transaction,
including a Taiko staging upgrade. A changed baseline, dependency or bytecode template
requires review and updated pins; do not bypass a mismatch.

Ethereum upgrade and rollback packets use MultiSendCallOnly v1.4.1 at
`0x9641d764fc13c8B624c04430C7356C1C7C8102e2`, with runtime hash
`0xecd5bd14a08c5d2122379900b2f272bdf107a7e92423c10dd5fe3254386c9939`.
These match [Safe's published deployment](https://github.com/safe-global/safe-deployments/blob/main/src/assets/v1.4.1/multi_send_call_only.json).
Before signing, compare the actual Safe proposal's target, value, data, operation,
gas/refund fields, nonce and Safe transaction hash with `transaction.json`.
Importing `safe-builder.json` supplies the inner calls; the UI chooses the outer helper.
If any field or hash differs, stop. Verify the final helper's published runtime and
rebuild and rehearse the packet for that exact envelope before collecting signatures.
Apply the same check to rollback. Base and Taiko exports are single CALLs.

## Rollback

The preparer also writes `rollback/transaction.json` and `rollback/safe-builder.json`
beside each upgrade draft. They call `upgradeToAndCall(previousImplementation, "")`
with no migration, using pinned baseline addresses and full runtime hashes. Ethereum
restores Accounting, Mint and ZKC atomically; Base and Taiko restore their markets separately.

For upgrade nonce N, the rollback hash uses N+1. Have three owners review and sign it
off-chain before upgrading if fast recovery is required. Check installed implementations,
baseline hashes and the Safe nonce before executing. Any intervening Safe transaction
requires a fresh hash and signatures; Taiko staging and production share one nonce sequence.
After installation, rerun the preparation command with the same candidate addresses,
`--rollback` and a separate `--output` directory. It requires those candidates to be
installed and works after the cutoff. All RPC calls remain read-only.

A failed atomic upgrade needs no rollback. After a successful rollback, verify the recorded
implementation slots, withdrawals and passive staking claims. Rollback restores the original
PoVW minting and emission schedule; it does not undo completed transactions or verifier cleanup.
Generated packets stay under ignored output directories. No deployment or signing keys are needed.
