# Contract Operations Guide

An operations guide for the Boundless contracts.

> [!NOTE]
> All the commands in this guide assume your current working directory is the root of the repo.

## Dependencies

Requires [Foundry](https://book.getfoundry.sh/getting-started/installation).

> [!NOTE]
> Running the `manage` commands will run in simulation mode (i.e. will not send transactions) unless the `--broadcast` flag is passed.

Commands in this guide use `yq` to parse the TOML config files.

You can install `yq` by following the [direction on GitHub][yq-install], or using `go install`.

```bash
go install github.com/mikefarah/yq/v4@latest
```

## Configuration

Configurations and deployment state information is stored in `deployment.toml`.
It contains information about each chain (e.g. name, ID, Etherscan URL), and addresses for the RISC Zero verifier and Boundless market contracts on each chain.

Accompanying the `deployment.toml` file is a `deployment_secrets.toml` file with the following schema.
It is used to store somewhat sensitive API keys for RPC services and Etherscan.
Note that it does not contain private keys or API keys for Fireblocks.
It should never be committed to `git`, and the API keys should be rotated if this occurs.

```toml
[chains.$CHAIN_KEY]
rpc-url = "..."
etherscan-api-key = "..."
```

## Environment

### Anvil

In development and to test the operations process, you can use Anvil.

Start Anvil:

```bash
anvil -a 10 --block-time 1 --host 0.0.0.0 --port 8545
```

Set your RPC URL, as well as your public and private key:

```bash
export RPC_URL="http://localhost:8545"
export DEPLOYER_PUBLIC_KEY="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
export DEPLOYER_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
```

### Public Networks (Testnet or Mainnet)

Set the chain you are operating on by the key from the `deployment.toml` file.
An example chain key is "ethereum-sepolia", and you can look at `deployment.toml` for the full list.

> TODO: Instead of reading these into environment variables, we can have the Forge script directly read them from the TOML file.

```zsh
export CHAIN_KEY="xxx-testnet"
```

Set your RPC URL, public and private key.

```bash
export RPC_URL=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].rpc-url" contracts/deployment_secrets.toml | tee /dev/stderr)
export ADMIN_ADDRESS=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].admin" contracts/deployment.toml | tee /dev/stderr)
```

> [!TIP]
> Foundry has a [config full of information about each chain][alloy-chains], mapped from chain ID.

Example RPC URLs:

- `https://ethereum-sepolia-rpc.publicnode.com`
- `https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY`
- `https://sepolia.infura.io/v3/YOUR_API_KEY`

### Fireblocks

Requires the [Fireblocks integration for Foundry](https://developers.fireblocks.com/docs/ethereum-smart-contract-development#using-foundry).

Also requires that you have a [Fireblocks API account](https://developers.fireblocks.com/docs/quickstart).

Set your public key, your Etherscan API key, and the necessary parameters for Fireblocks:

> [!NOTE]
> Fireblocks only supports RSA for API request signing.
> `FIREBLOCKS_API_PRIVATE_KEY_PATH` can be the key itself, rather than a path.

```bash
export FIREBLOCKS_API_KEY="..."
export FIREBLOCKS_API_PRIVATE_KEY_PATH="..."

# IF YOU ARE IN A SANDBOX ENVIRONMENT, be sure to also set this:
export FIREBLOCKS_API_BASE_URL="https://sandbox-api.fireblocks.io"
```

Then, in the instructions below, pass the `--fireblocks` (`-f`) flag to the `manage` script.

> [!NOTE]
> Your Fireblocks API user will need to have "Editor" permissions (i.e., ability to propose transactions for signing, but not necessarily the ability to sign transactions). You will also need a Transaction Authorization Policy (TAP) that specifies who the signers are for transactions initiated by your API user, and this policy will need to permit contract creation as well as contract calls.

> [!NOTE]
> Before you approve any contract-call transactions, be sure you understand what the call does! When in doubt, use [Etherscan](https://etherscan.io/) to lookup the function selector, together with a [calldata decoder](https://openchain.xyz/tools/abi) ([alternative](https://calldata.swiss-knife.xyz/decoder)) to decode the call's arguments.

> [!TIP]
> Foundry and the Fireblocks JSON RPC shim don't quite get along.
> In order to avoid sending the same transaction for approval twice (or more), use ctrl-c to
> kill the forge script once you see that the transaction is pending approval in the Fireblocks
> console.

## Deploy and upgrade the market contract with the **UUPS** proxy pattern

The Boundless market is deployed and upgraded using the **UUPS (Universal Upgradeable Proxy Standard)** proxy pattern.

### Deploy the HitPoints contract

1. Dry run deployment of the HitPoints contract:

   ```zsh
   BOUNDLESS_MARKET_OWNER=${ADMIN_ADDRESS:?} \
   bash contracts/scripts/manage DeployHitPoints
   ```

2. Send deployment transactions for the HitPoints contract by running the command again with `--broadcast`.

   > [!NOTE]
   > When using Fireblocks, sending a transaction to a particular address may require allow-listing it.

3. Update the `stake-token` field with the HitPoints address of the newly deployed contract to the `deployment.toml` file.

### Deploy the market contract

1. Make available for download the `assessor` elf and set its image ID and url in the `deployment.toml` file.

   To generate a deterministic image ID run (from the repo root folder):

   ```zsh
   cargo risczero build --manifest-path crates/guest/assessor/assessor-guest/Cargo.toml
   ```

   This will output the image ID and file location.

   1. Upload the ELF to some public HTTP location (such as Pinata), and get back a download URL.
   2. Record these values in `deployment.toml` as `assessor-image-id` and `assessor-guest-url`.

   <br/>

   > [!TIP]
   > The `r0vm` binary can be used to double-check that the imageID corresponds to a given elf. e.g., `r0vm --id --elf [elf_path]`
   > You can combine this with curl to check the image ID of an ELF hosted at a URL.
   >
   > ```
   > r0vm --id --elf <(curl $ELF_URL)
   > ```

2. Dry run deployment of the market implementation and proxy:

   ```zsh
   BOUNDLESS_MARKET_OWNER=${ADMIN_ADDRESS:?} \
   bash contracts/scripts/manage DeployBoundlessMarket
   ```

   > [!IMPORTANT]
   > Check the logs from this dry run to verify the market owner is the expected address.
   > It should be equal to the RISC Zero admin address on the given chain.
   > Note that it should not be the `TimelockController`.
   > Also check the chain ID to ensure you are deploying to the chain you expect.
   > And check the Assessor info to make sure they match what you expect.

3. Send deployment transactions for the market contract by running the command again with `--broadcast`.

   > [!NOTE]
   > When using Fireblocks, sending a transaction to a particular address may require allow-listing it.

4. Add the BoundlessMarket (proxy) address of the newly deployed contract to the `deployment.toml` file.

5. Test the deployment.

   ```bash
   FOUNDRY_PROFILE=deployment-test forge test --fork-url $RPC_URL
   ```

### Upgrade the market contract

1. Git clone and forge build the last deployment, then copy the `contracts/out/build-info` folder into `contracts/reference-contract/build-info-reference`

2. If changed, upload the new `assessor` elf and update its image ID and url in the `deployment.toml` file (optional)

3. Dry run the upgrade of the market implementation and proxy:

   ```zsh
   BOUNDLESS_MARKET_OWNER=${ADMIN_ADDRESS:?} \
   bash contracts/scripts/manage UpgradeBoundlessMarket
   ```

   > [!IMPORTANT]
   > Check the logs from this dry run to verify the market owner is the expected address.
   > It should be equal to the RISC Zero admin address on the given chain.
   > Note that it should not be the `TimelockController`.
   > Also check the chain ID to ensure you are deploying to the chain you expect.
   > And check the Assessor info to make sure they match what you expect.

4. Send the upgrade transactions for the market contract by running the command again with `--broadcast`.

   > [!NOTE]
   > When using Fireblocks, sending a transaction to a particular address may require allow-listing it.

5. Test the deployment.

   ```bash
   FOUNDRY_PROFILE=deployment-test forge test --fork-url $RPC_URL
   ```

   > [!IMPORTANT]
   > Make sure the Assessor info to make sure they match what you expect.

[yq-install]: https://github.com/mikefarah/yq?tab=readme-ov-file#install
[alloy-chains]: https://github.com/alloy-rs/chains/blob/main/src/named.rs
