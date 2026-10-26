# Market and PoVW sunset

Local implementation for November 5, 2026, 00:00 UTC (`1793836800`). Both market variants reject new work and fulfillment. PoVW Accounting rejects updates/finalization; PoVW Mint rejects every claim. Withdrawals, reads and upgrade administration remain available. No storage or initializer changes.

The only production changes are cutoff checks, the shared `Sunset` library, and the generated Solidity ZKC interface's `pure` to `view` correction. Frozen baseline contracts are test fixtures. ZKC emission changes live in the separate `codex/povw-sunset` branch.

## Tests

```sh
forge test --offline --match-contract 'MarketSunsetTest|PovwSunsetTest'
forge test --offline --match-contract 'BoundlessMarketBasicTest|BoundlessMarketLegacyViaFallbackBasicTest' --match-test '^testSunset'
FOUNDRY_PROFILE=shanghai forge test --offline --match-contract MarketSunsetTest
FOUNDRY_PROFILE=shanghai forge test --offline --match-contract BoundlessMarketBasicTest --match-test '^testSunset'
FOUNDRY_PROFILE=povw-deploy forge test --offline --out out-povw-sunset --cache-path cache-povw-sunset --match-contract PovwSunsetTest
contracts/fork-test/run.sh all
```

The unit/profile suites pass 41 tests. The fork runner passes 15 more tests using Base block 53113412, Taiko block 13186463 and Ethereum block 26276732. Override `BASE_RPC_URL`, `TAIKO_RPC_URL` or `ETHEREUM_RPC_URL` to use another archive provider. Results go to `out/sunset-fork-reports/`.

Fork tests require the pinned chain, block and implementation code hashes. They deploy candidates with live constructor dependencies and execute the real 3-of-6 Safes using locally impersonated owners. They check implementation-slot-only writes, cutoff boundaries, balances, withdrawals, initializer/authorization failures, atomic PoVW batch failure and rollback. No transaction is broadcast.

Unit fulfillment tests use accepting proof-verifier fixtures. Fork cutoff tests deliberately reject proof calls before verification; they do not prove a new cryptographic proof succeeds. Fork addresses are dynamically deployed test addresses. Final production deployment bytecode, addresses and transaction payloads still require verification before signing.

## Prepare the exact Safe transaction

Use the wrappers below; they set the required artifact path and build profile. The printer requires an implementation already deployed on the selected network. It checks its complete runtime against this build, including constructor dependencies, allowing only self-address immutable differences. Tests and printer share the same calldata builder. The scripts never broadcast; reference implementations exist only in the local script VM.

```sh
SUNSET_IMPL=<market-implementation> contracts/scripts/sunset/prepare.sh base
SUNSET_IMPL=<market-implementation> contracts/scripts/sunset/prepare.sh taiko
ACCOUNTING_SUNSET_IMPL=<accounting-implementation> MINT_SUNSET_IMPL=<mint-implementation> ZKC_SUNSET_IMPL=<zkc-implementation> contracts/scripts/sunset/prepare-ethereum.sh
```

Rerun the fork suite against those same deployed addresses with the same variables and `SUNSET_BLOCK_NUMBER=<block-after-deployment>`, for example `contracts/fork-test/run.sh base`. Default blocks remain the historical baselines for tests that deploy their own candidates.

Each command writes `transaction.json` with all Safe fields, current nonce and transaction hash, plus `simulation.json`. The preflight uses read-only `eth_call` at the recorded block and an explicit threshold override from 3 to 1. The fork tests separately execute the real 3-of-6 threshold. The RPC must support historical state and state overrides.

The helper generates a Tenderly draft link locally. It does not submit a Tenderly simulation. If the link omits long calldata, paste `execTransactionData` from `simulation.json` into Raw input data. Verify the network and recorded block before simulating. No private key is required.

Preflight guard tests use a mocked transport: `python3 -m unittest discover -s contracts/scripts/sunset -p 'test_*.py'`. The local Anvil workflow separately checks real calldata encoding and Safe execution.

Run `all` only with default local candidates. For supplied addresses or a block override, run each chain separately.

The Ethereum command runs both repositories' candidate checks at one block, then creates **one atomic Foundation Safe transaction** for Accounting, Mint and ZKC. `ZKC_REPO` defaults to the sibling `../zkc` checkout. Sign only the final `transaction.json`; the `povw/` and `zkc/` files are inputs, not separate transactions. The wrapper also runs the combined-batch fork tests, including failure of the third upgrade rolling back all three.
