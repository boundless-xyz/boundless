# Market and PoVW sunset

Local implementation for November 5, 2026, 00:00 UTC (`1793836800`). Both market variants reject new work and fulfillment. PoVW Accounting rejects updates/finalization; PoVW Mint rejects every claim. Withdrawals, reads and upgrade administration remain available. No storage or initializer changes.

The only production changes are cutoff checks, the shared `Sunset` library, and the generated Solidity ZKC interface's `pure` to `view` correction. Frozen baseline contracts are test fixtures. ZKC emission changes live in the separate `codex/povw-sunset` branch.

## Tests

```sh
forge test --offline --match-contract 'MarketSunsetTest|PovwSunsetTest'
forge test --offline --match-contract '.*ValidSunsetTest' --match-test '^testSunset'
FOUNDRY_PROFILE=shanghai forge test --offline --match-contract MarketSunsetTest
FOUNDRY_PROFILE=shanghai forge test --offline --match-contract CurrentMarketValidSunsetTest --match-test '^testSunset'
FOUNDRY_PROFILE=povw-deploy forge test --offline --out out-povw-sunset --cache-path cache-povw-sunset --match-contract PovwSunsetTest
contracts/fork-test/run.sh all
```

The unit/profile suites pass 41 tests. The fork runner passes 9 more tests using Base block 53113412, Taiko block 13186463 and Ethereum block 26276732. Override `BASE_RPC_URL`, `TAIKO_RPC_URL` or `ETHEREUM_RPC_URL` to use another archive provider. Results go to `out/sunset-fork-reports/`.

Fork tests require the pinned chain, block and implementation code hashes. They deploy candidates with live constructor dependencies and execute the real 3-of-6 Safes using locally impersonated owners. They check implementation-slot-only writes, cutoff boundaries, balances, withdrawals, initializer/authorization failures, atomic PoVW batch failure and rollback. No transaction is broadcast.

Unit fulfillment tests use accepting proof-verifier fixtures. Fork cutoff tests deliberately reject proof calls before verification; they do not prove a new cryptographic proof succeeds. Fork addresses are dynamically deployed test addresses. Final production deployment bytecode, addresses and transaction payloads still require verification before signing.
