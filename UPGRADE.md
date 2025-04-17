# Upgrade dependencies process

## Upgrade risc0-ethereum

When a new version of `risc0-ethereum` gets released, run the following steps:

1. Under the folder ./lib/risc0-ethereum run:

   ```bash
   git fetch
   git checkout vX.Y.Z
   ```

   where `vX.Y.Z` correspond to the latest version.

2. Update all the `Cargo.toml` files in all the workspaces for the following packages (examples folder included):
   - `risc0-aggregation`
   - `risc0-build-ethereum`
   - `risc0-ethereum-contracts`
   - `guest-set-builder`

   > Here is a command to help find all the Cargo.toml files that need to be updated
   >
   > ```
   > grep -rl '^version = "' --include=Cargo.toml --exclude-dir='./lib' --exclude-dir='./target' .
   > ```

3. Make sure that all `alloy` related crates match with the version used in `risc0-ethereum`.

4. Update all the `Cargo.lock` files (examples folder included).

   > Here is a nifty oneliner to do that
   >
   > ```
   > grep -rl --include="Cargo.toml" '\[workspace\]' --exclude-dir=./lib | sort -u | xargs -n1 cargo check --manifest-path
   > ```

## Upgrade zkVM version

When a new version of `risc0` gets released, run the following steps:

1. Update all the `Cargo.toml` files for the following packages (examples folder included):
   - `bonsai-sdk`
   - `risc0-build`
   - `risc0-circuit-recursion`
   - `risc0-zkvm`
   - `risc0-zkp`

   > Here is a command to help find all the Cargo.toml files that need to be updated
   >
   > ```
   > grep -rl '^version = "' --include=Cargo.toml --exclude-dir='./lib' --exclude-dir='./target' .
   > ```

2. Update, if changed, the `channel` of the [rust-toolchain.toml](./rust-toolchain.toml) file to match with the latest supported risc0-toolchain version.

3. Update the CI workflows to use the latest version of:
   - `risczero-version`
   - `toolchain-version`

4. If the new `zkVM` version also required a new `RiscZeroGroth16Verifier` contract deployment, make sure to update all the references of `Groth16Vx_y`:
   - search for all the occurrences of `Groth16Vx_y` and update accordingly to the latest version used in the [selector.rs](./lib/risc0-ethereum/contracts/src/selector.rs) file.

5. Make sure to update the revision of `zeth` in the [Cargo.toml](./crates/order-generator/Cargo.toml) of the `order-generator` to match with the most recent commit compatible with the `zkVM` version that is being updated. Also update its `Cargo.lock` with:

   ```bash
   cargo check --workspace -F zeth
   ```

6. Update all the `Cargo.lock` files by running:

   ```bash
   grep -rl --include="Cargo.toml" '\[workspace\]' --exclude-dir=./lib | sort -u | xargs -n1 cargo check --manifest-path
   ```
