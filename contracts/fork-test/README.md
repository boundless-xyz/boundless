# Reproduce the sunset upgrades

Use both feature branches, checked out as sibling directories `market/` and `zkc/`.
The ZKC branch is `codex/povw-sunset`; the market branch is `codex/protocol-sunset`.
These instructions need no files from the original author's preparation directory.
To share the unpublished branches, create local Git bundles:

```bash
git -C market bundle create ../sunset-market.bundle codex/protocol-sunset
git -C zkc bundle create ../sunset-zkc.bundle codex/povw-sunset
```

A colleague can use `git clone sunset-market.bundle market` and
`git clone sunset-zkc.bundle zkc`, then follow the commands below. Submodules still
come from their pinned upstream commits. Neither bundle contains deployment secrets.

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
npx --yes @openzeppelin/upgrades-core@1.40.0 validate --help > /dev/null
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

From the same parent directory, compare each candidate with its frozen original.
Separate build-info directories prevent older compiler output from entering the check.
No unsafe validation flags are required.

```bash
(cd market && FOUNDRY_BUILD_INFO_PATH=out/validate-base/build-info forge build --offline --force --out out/validate-base --cache-path cache/validate-base)
(cd market && FOUNDRY_PROFILE=shanghai FOUNDRY_EVM_VERSION=shanghai FOUNDRY_BUILD_INFO_PATH=out/validate-taiko/build-info forge build --offline --force --out out/validate-taiko --cache-path cache/validate-taiko)
(cd market && FOUNDRY_PROFILE=povw-deploy FOUNDRY_BUILD_INFO_PATH=out/validate-povw/build-info forge build --offline --force --out out/validate-povw --cache-path cache/validate-povw)
(cd zkc && FOUNDRY_BUILD_INFO_PATH=out/validate-zkc/build-info forge build --offline --force --out out/validate-zkc --cache-path cache/validate-zkc)
npx --yes @openzeppelin/upgrades-core@1.40.0 validate market/out/validate-base/build-info --contract contracts/src/BoundlessMarket.sol:BoundlessMarket --reference contracts/test/reference/BoundlessMarketOriginal.sol:BoundlessMarketOriginal --requireReference
npx --yes @openzeppelin/upgrades-core@1.40.0 validate market/out/validate-taiko/build-info --contract contracts/shanghai/variants/BoundlessMarket.sol:BoundlessMarket --reference contracts/test/reference/BoundlessMarketShanghaiOriginal.sol:BoundlessMarketOriginal --requireReference
npx --yes @openzeppelin/upgrades-core@1.40.0 validate market/out/validate-povw/build-info --contract contracts/src/povw/PovwAccounting.sol:PovwAccounting --reference contracts/test/reference/PovwAccountingOriginal.sol:PovwAccountingOriginal --requireReference
npx --yes @openzeppelin/upgrades-core@1.40.0 validate market/out/validate-povw/build-info --contract contracts/src/povw/PovwMint.sol:PovwMint --reference contracts/test/reference/PovwMintOriginal.sol:PovwMintOriginal --requireReference
npx --yes @openzeppelin/upgrades-core@1.40.0 validate zkc/out/validate-zkc/build-info --contract src/ZKC.sol:ZKC --reference test/reference/ZKCOriginal.sol:ZKCOriginal --requireReference
```

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
python3 market/contracts/fork-test/rehearse.py --environment production --zkc-repo zkc --output market/out/handoff/production
```

All six rehearsals must report `passed`. Staging has 17 Foundry cases; production has
21 supplied-candidate cases plus two exact Ethereum batch cases. Ethereum upgrades
Accounting, Mint and ZKC atomically in that order, with no initializer. Base and Taiko
upgrade their markets separately. Staging Taiko is chain 167000 and shares the production
Safe and nonce. Sepolia uses its own 3-of-5 Safe; the other Safes require 3-of-6.

`baselines.json` pins ten proxy baselines, full runtime hashes, dependencies, snapshot
blocks and candidate bytecode templates. The scripts rebuild candidates; they do not
accept arbitrary replacement artifacts. Override the default read-only endpoints with
`BASE_RPC_URL`, `ETHEREUM_RPC_URL`, `TAIKO_RPC_URL`, `BASE_SEPOLIA_RPC_URL` or
`ETHEREUM_SEPOLIA_RPC_URL` if an archive endpoint is unavailable. `--chain` can select
`base`, `taiko` or `ethereum`. Rehearsals use historical snapshots even after the cutoff.

These execution rehearsals set `SKIP_SAFETY_CHECKS=true` only on localhost. The separate
OpenZeppelin checks above are required. Keep validation enabled for an actual deployment.
Proof verifiers are mocked where needed; these checks do not prove signer availability
or exhaust every possible contract state. The suites cover cutoff boundaries, preserved
passive staking rewards and withdrawals, permissions, initialization and failed upgrades.

## Bundles and unsigned transactions

Keep the generated `out/handoff/` bundle with the two source revisions. It contains
Safe JSON, full calldata, baseline and candidate hashes, tool versions, submodule pins,
Anvil execution results and test logs. It is useful evidence for a reviewer. Candidate
addresses exist only on the disposable forks; replay a saved bundle by rerunning the
rehearsal from its recorded source revisions. The saved JSON alone cannot restore fork state.

```bash
tar -czf sunset-rehearsal-evidence.tgz -C market/out handoff
shasum -a 256 sunset-rehearsal-evidence.tgz
```

Do not submit rehearsal JSON to a live Safe. After approved candidate deployment,
regenerate the unsigned draft against current chain state. `prepare.py` performs RPC
reads only, even for loopback endpoints. It checks supplied candidate runtimes against
locally rebuilt artifacts and pins every read to one block. For example, from `market/`:

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
