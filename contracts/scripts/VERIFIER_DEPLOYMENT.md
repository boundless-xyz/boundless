# Contract Operations Guide

An operations guide for the Boundless verifier contracts.

> [!NOTE]
> All the commands in this guide assume your current working directory is the root of the repo.

## Dependencies

Requires [Foundry](https://book.getfoundry.sh/getting-started/installation).

> [!NOTE]
> Running the `manage-verifier` commands will run in simulation mode (i.e. will not send transactions) unless the `--broadcast` flag is passed.
> When setting `GNOSIS_EXECUTE=true` all the transactions calldata will be printed so that they can be copied over to the Safe web app.

Commands in this guide use `yq` to parse the TOML config files.

You can install `yq` by following the [directions on GitHub][yq-install], or using `go install`.

```sh
go install github.com/mikefarah/yq/v4@latest
```

## Configuration

Configurations and deployment state information is stored in `deployment_verifier.toml`.
It contains information about each chain (e.g. name, ID, Etherscan URL), and addresses for the timelock, router, and verifier contracts on each chain.

Accompanying the `deployment_verifier.toml` file is a `deployment_secrets.toml` file with the following schema.
It is used to store somewhat sensitive API keys for RPC services and Etherscan.
Note that it does not contain private keys or API keys for Fireblocks.
It should never be committed to `git`, and the API keys should be rotated if this occurs.

```toml
[chains.$CHAIN_KEY]
rpc-url = "..."
etherscan-api-key = "..."
```

## Environment

### Public Networks (Testnet or Mainnet)

Set the chain you are operating on by the key from the `deployment_verifier.toml` file.
An example chain key is "ethereum-sepolia", and you can look at `deployment_verifier.toml` for the full list.

```sh
export CHAIN_KEY="xxx-testnet"
```

**Based on the chain key, the `manage-verifier` script will automatically load environment variables from deployment_verifier.toml and deployment_secrets.toml**

If the chain you are deploying to is not in `deployment_secrets.toml`, set your RPC URL, public and private key, and Etherscan API key:

```sh
export RPC_URL=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].rpc-url" contracts/deployment_secrets.toml | tee /dev/stderr)
export ETHERSCAN_URL=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].etherscan-url" contracts/deployment_verifier.toml | tee /dev/stderr)
export ETHERSCAN_API_KEY=$(yq eval -e ".chains[\"${CHAIN_KEY:?}\"].etherscan-api-key" contracts/deployment_secrets.toml | tee /dev/stderr)
```

> [!TIP]
> Foundry has a [config full of information about each chain][alloy-chains], mapped from chain ID.
> It includes the Etherscan compatible API URL, which is how only specifying the API key works.
> You can find this list in the following source file:

Example RPC URLs:

- `https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY`
- `https://sepolia.infura.io/v3/YOUR_API_KEY`

## Deploy the timelocked router

1. Dry run the contract deployment:

   > [!IMPORTANT]
   > Adjust the `MIN_DELAY` (or `timelock-delay` in the toml) to a value appropriate for the environment (e.g. 0 second for testnet and 259200 seconds (3 days) for mainnet).

   ```sh
   contracts/scripts/manage-verifier DeployTimelockRouter

   ...

   == Logs ==
     minDelay: 1
     proposers: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
     executors: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
     admin: 0x0000000000000000000000000000000000000000
     Deployed TimelockController to 0x5FbDB2315678afecb367f032d93F642f64180aa3
     Deployed VerifierLayeredRouter to 0x918063A3fa14C59b390B18db8b1A565780E8b933
   ```

2. Run the command again with `--broadcast`.

   This will result in two transactions sent from the deployer address.

3. Test the deployment.

   ```console
   FOUNDRY_PROFILE=deployment-test forge test --match-contract=VerifierDeploymentTest -vv --fork-url=${RPC_URL:?}
   ```

## Deploy a Blae3Groth16 verifier with emergency stop mechanism

This is a two-step process, guarded by the `TimelockController`.

### Deploy the verifier

1. Dry run deployment of BlakeGroth16 verifier and estop:

   ```sh
   contracts/scripts/manage-verifier DeployEstopBlake3Groth16Verifier
   ```

   > [!IMPORTANT]
   > Check the logs from this dry run to verify the estop owner is the expected address.
   > It should be equal to the admin address on the given chain.
   > Note that it should not be the `TimelockController`.
   > Also check the chain ID to ensure you are deploying to the chain you expect.
   > And check the selector to make sure it matches what you expect.

2. Send deployment transactions for verifier and estop by running the command again with `--broadcast`.

   This will result in two transactions sent from the deployer address.

3. Add the addresses for the newly deployed contract to the `deployment_verifier.toml` file.

4. Test the deployment.

   ```sh
   FOUNDRY_PROFILE=deployment-test forge test --match-contract=VerifierDeploymentTest -vv --fork-url=${RPC_URL:?}
   ```

5. Print the operation to schedule the operation to add the verifier to the router.

   ```sh
   GNOSIS_EXECUTE=true VERIFIER_SELECTOR="0x..." bash contracts/scripts/manage-verifier ScheduleAddVerifier
   ```

6. Send the transaction for the scheduled update via Safe.

### Finish the update

After the delay on the timelock controller has passed, the operation to add the new verifier to the router can be executed.

1. Print the transaction calldata to execute the add verifier operation:

   ```sh
   GNOSIS_EXECUTE=true VERIFIER_SELECTOR="0x..." bash contracts/scripts/manage-verifier FinishAddVerifier
   ```

2. Send the transaction for execution via Safe

3. Remove the `unroutable` field from the selected verifier.

4. Test the deployment.

   ```console
   FOUNDRY_PROFILE=deployment-test forge test --match-contract=VerifierDeploymentTest -vv --fork-url=${RPC_URL:?}
   ```

## Remove a verifier

This is a two-step process, guarded by the `TimelockController`.

### Schedule the update

1. Print the transaction to schedule the remove verifier operation:

   ```sh
   GNOSIS_EXECUTE=true VERIFIER_SELECTOR="0x..." contracts/scripts/manage-verifier ScheduleRemoveVerifier
   ```

2. Send the transaction for execution via Safe

### Finish the update

1. Print the transaction to execute the remove verifier operation:

   ```sh
   GNOSIS_EXECUTE=true VERIFIER_SELECTOR="0x..." contracts/scripts/manage-verifier FinishRemoveVerifier
   ```

2. Send the transaction for execution via Safe

3. Update `deployment_verifier.toml` and set `unroutable = true` on the removed verifier.

4. Test the deployment.

   ```console
   FOUNDRY_PROFILE=deployment-test forge test --match-contract=VerifierDeploymentTest -vv --fork-url=${RPC_URL:?}
   ```

## Update the TimelockController minimum delay

This is a two-step process, guarded by the `TimelockController`.

The minimum delay (`MIN_DELAY`) on the timelock controller is denominated in seconds.

### Schedule the update

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true MIN_DELAY=10 contracts/scripts/manage-verifier ScheduleUpdateDelay
   ```

2. Send the transaction for execution via Safe

### Finish the update

Execute the action:

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true MIN_DELAY=10  contracts/scripts/manage-verifier FinishUpdateDelay
   ```

2. Send the transaction for execution via Safe

3. Test the deployment.

   ```console
   FOUNDRY_PROFILE=deployment-test forge test --match-contract=VerifierDeploymentTest -vv --fork-url=${RPC_URL:?}
   ```

## Cancel a scheduled timelock operation

Use the following steps to cancel an operation that is pending on the `TimelockController`.

1. Identify the operation ID and set the environment variable.

   > TIP: One way to get the operation ID is to open the contract in Etherscan and look at the events.
   > On the `CallScheduled` event, the ID is labeled as `[topic1]`.
   >
   > ```sh
   > open ${ETHERSCAN_URL:?}/address/${TIMELOCK_CONTROLLER:?}#events
   > ```

   ```sh
   export OPERATION_ID="0x..." \
   ```

2. Print the transaction calldata to cancel the operation.

   ```sh
   GNOSIS_EXECUTE=true contracts/scripts/manage-verifier CancelOperation
   ```

3. Send the transaction for execution via Safe

## Grant access to the TimelockController

This is a two-step process, guarded by the `TimelockController`.

Three roles are supported:

- `proposer`
- `executor`
- `canceller`

### Schedule the update

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true \
   ROLE="executor" \
   ACCOUNT="0x00000000000000aabbccddeeff00000000000000" \
   bash contracts/scripts/manage-verifier ScheduleGrantRole
   ```

2. Send the transaction for execution via Safe

### Finish the update

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true \
   ROLE="executor" \
   ACCOUNT="0x00000000000000aabbccddeeff00000000000000" \
   bash contracts/scripts/manage-verifier FinishGrantRole
   ```

2. Send the transaction for execution via Safe.

3. Confirm the update:

   ```sh
   # Query the role code.
   cast call --rpc-url ${RPC_URL:?} \
       ${TIMELOCK_CONTROLLER:?} \
       'EXECUTOR_ROLE()(bytes32)'
   0xd8aa0f3194971a2a116679f7c2090f6939c8d4e01a2a8d7e41d55e5351469e63

   # Check that the account now has that role.
   cast call --rpc-url ${RPC_URL:?} \
       ${TIMELOCK_CONTROLLER:?} \
       'hasRole(bytes32, address)(bool)' \
       0xd8aa0f3194971a2a116679f7c2090f6939c8d4e01a2a8d7e41d55e5351469e63 \
       0x00000000000000aabbccddeeff00000000000000
   true
   ```

## Revoke access to the TimelockController

This is a two-step process, guarded by the `TimelockController`.

Three roles are supported:

- `proposer`
- `executor`
- `canceller`

### Schedule the update

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true \
   ROLE="executor" \
   ACCOUNT="0x00000000000000aabbccddeeff00000000000000" \
   bash contracts/scripts/manage-verifier ScheduleRevokeRole
   ```

2. Send the transaction for execution via Safe

Confirm the role code:

```sh
cast call --rpc-url ${RPC_URL:?} \
    ${TIMELOCK_CONTROLLER:?} \
    'EXECUTOR_ROLE()(bytes32)'
0xd8aa0f3194971a2a116679f7c2090f6939c8d4e01a2a8d7e41d55e5351469e63
```

### Finish the update

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true \
   ROLE="executor" \
   ACCOUNT="0x00000000000000aabbccddeeff00000000000000" \
   bash contracts/scripts/manage-verifier FinishRevokeRole
   ```

2. Send the transaction for execution via Safe

3. Confirm the update:

   ```sh
   # Query the role code.
   cast call --rpc-url ${RPC_URL:?} \
       ${TIMELOCK_CONTROLLER:?} \
       'EXECUTOR_ROLE()(bytes32)'
   0xd8aa0f3194971a2a116679f7c2090f6939c8d4e01a2a8d7e41d55e5351469e63

   # Check that the account no longer has that role.
   cast call --rpc-url ${RPC_URL:?} \
       ${TIMELOCK_CONTROLLER:?} \
       'hasRole(bytes32, address)(bool)' \
       0xd8aa0f3194971a2a116679f7c2090f6939c8d4e01a2a8d7e41d55e5351469e63 \
       0x00000000000000aabbccddeeff00000000000000
   false
   ```

## Renounce access to the TimelockController

If your private key is compromised, you can renounce your role(s) without waiting for the time delay. Repeat this action for any of the roles you might have, such as:

- proposer
- executor
- canceller

> ![WARNING]
> Renouncing authorization on the timelock controller may make it permanently inoperable.

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true \
   RENOUNCE_ROLE="executor" \
   RENOUNCE_ADDRESS="0x00000000000000aabbccddeeff00000000000000" \
   bash contracts/scripts/manage-verifier RenounceRole
   ```

2. Send the transaction for execution via Safe

3. Confirm:

   ```sh
   cast call --rpc-url ${RPC_URL:?} \
       ${TIMELOCK_CONTROLLER:?} \
       'hasRole(bytes32, address)(bool)' \
       0xd8aa0f3194971a2a116679f7c2090f6939c8d4e01a2a8d7e41d55e5351469e63 \
       ${RENOUNCE_ADDRESS:?}
   false
   ```

## Activate the emergency stop

Activate the emergency stop:

> ![WARNING]
> Activating the emergency stop will make that verifier permanently inoperable.

> ![NOTE]
> In order to send a transaction to the estop contract in Fireblocks, the addresses need to be added to the allow-list.
> If this has not already been done, do this as a pre-step.

1. Print the transaction calldata:

   ```sh
   GNOSIS_EXECUTE=true \
   VERIFIER_SELCTOR="0x..." \
   bash contracts/scripts/manage-verifier ActivateEstop
   ```

2. Send the transaction for execution via Safe

3. Test the activation:

   ```sh
   cast call --rpc-url ${RPC_URL:?} \
       ${VERIFIER_ESTOP:?} \
       'paused()(bool)'
   true
   ```

[yq-install]: https://github.com/mikefarah/yq?tab=readme-ov-file#install
[alloy-chains]: https://github.com/alloy-rs/chains/blob/main/src/named.rs
