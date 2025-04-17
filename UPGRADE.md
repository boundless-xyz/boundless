# Upgrade dependencies process

## Upgrade risc0-ethereum

When a new version of `risc0-ethereum` gets released, run the following steps:

1. Under the folder ./lib/risc0-ethereum run:

   ```bash
   git fetch
   git checkout vX.Y.Z
   ```

   where `vX.Y.Z` correspond to the latest version.

2. Update all the `Cargo.toml` files for the following packages:
   - `risc0-aggregation`
   - `risc0-build-ethereum`
   - `risc0-ethereum-contracts`
   - `guest-set-builder`

3. Make sure that all `alloy` related crates match with the version used in `risc0-ethereum`.

## Upgrade zkVM version

When a new version of `risc0` gets released, run the following steps:

1. Update all the `Cargo.toml` files for the following packages:
   - `bonsai-sdk`
   - `risc0-build`
   - `risc0-circuit-recursion`
   - `risc0-zkvm`
   - `risc0-zkp`

2. Update, if changed, the `channel` of the [rust-toolchain.toml](./rust-toolchain.toml) file to match with the latest supported risc0-toolchain version.

3. Update the CI workflows to use the latest version of:
   - `risczero-version`
   - `toolchain-version`

4. If the new `zkVM` version also required a new `RiscZeroGroth16Verifier` contract deployment, make sure to update all the references of `Groth16Vx_y`:
   - search for all the occurrences of `Groth16Vx_y` and update accordingly to the latest version used in the [selector.rs](./lib/risc0-ethereum/contracts/src/selector.rs) file.

5. Make sure to update the revision of `zeth` in the [Cargo.toml](./crates/order-generator/Cargo.toml) of the `order-generator` to match with the most recent commit compatible with the `zkVM` version that is being updated. 

6. Update all the `Cargo.lock` files by running:

   ```bash
   cargo check --workspace -F zeth
   ```
