# Eigen full and partial withdrawal example

This example simulates an EigenPod where Ethereum validator can restake their stake (on the Beacon Chain) to earn some yields.
We have mocked the Beacon Chain and integrated within the EigenPod contract for simplicity.
For each block, a validator earns the 0.01% of its stake. Yields can be redeemed by submitting a partial withdrawal request.
Moreover, a validator can withdraw both the stake and the yield, by submitting a full withdrawal request.
Both of these requests require to present a valid zk proof attesting valid credentials and membership on the beacon chain (VS presenting the proof in the form of multiple inclusion paths as currently implemented in the real EigenPod contract).

The goal of the example is to exercise the user experience of a validator that wishes to redeem the earned yield using the RISC Zero zkVM by interacting with the Boundless market.

> **NOTE: all the following commands must be issued from the `examples/eigen-withdrawal` folder**

## Build

<!-- TODO: DOCKER BUILD seems broken on MacOS Sequoia -->

To build the project run:

```bash
cargo build --release
```

Enabling the env variable `RISC0_USE_DOCKER` will make sure to build the `eigen_withdrawal` guest using reproducible build (requires having Docker installed).
This step is especially important when interacting with a deployment on Sepolia (vs local devnet) as the `eigen_withdrawal` imageID on each proving request must match with what used during the contract deployment.

## Deploy the EigenPod contract (only owner)

To deploy the contract, you can run:

```bash
# For local development, use anvil with RPS at http://localhost:8545
# https://book.getfoundry.sh/reference/anvil/
anvil

# We assume local development in default .env 
source .env
forge script contracts/scripts/Deploy.s.sol --rpc-url $RPC_URL --broadcast -vv
```

This will output something like:

```console
Script ran successfully.

== Logs ==
  Deployed EigenPod to 0x3De59F36e04FA01768053BD795DA9B3764BE4282

...
```

make sure to look at the env variable used in the deploy script and adjust them if needed.

### Contract management

As the contract's owner, you can top up as well as drain the contract's funds.
Topping up is required to have funds from which the yields will be distributed,
while draining is useful to avoid wasting testnet funds.

#### TopUp the contract funds

To top up, run

```bash
cast send --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL $EIGEN_POD_ADDRESS --value 10ether "topUp()"
```

#### Drain the contract funds

To drain, run

```bash
cast send --private-key $ADMIN_PRIVATE_KEY --rpc-url $RPC_URL $EIGEN_POD_ADDRESS "drain()"
```

## Interact with the EigenPod contract

We provide a simple `eigen-cli` to interact with the contract. The following shows how to use it.
If you are using a local devnet all the env variables you need should be in the `.env` file, except:

```bash
# Enforce https://dev.risczero.com/api/next/generating-proofs/dev-mode
# Thus we mock all proofs and contracts to test end-to-end.
export RISC0_DEV_MODE=1
```

If instead you are using a deployment on sepolia, get access to an Ethereum node running on Sepolia testnet (in this example, we will be using [Alchemy](https://www.alchemy.com/) as our Ethereum node provider), create an account on [Pinata](https://www.pinata.cloud) (used as the IPFS provider) if you don't have one already, and export the following environment variables::

```bash
export ALCHEMY_APY_KEY="YOUR_ALCHEMY_APY_KEY"
export RPC_URL=https://eth-sepolia.g.alchemy.com/v2/${ALCHEMY_API_KEY:?}
# Anvil sets up a number of default wallets, and this private key is one of them.
export WALLET_PRIVATE_KEY="YOUR_SEPOLIA_PRIVATE_KEY"
# Proof Market contract address on Sepolia
export PROOF_MARKET_ADDRESS="0x1074Dc9CaEa49B5830D3b9A625CdEA9C1038FC45"
# Eigen Pod contract address on Sepolia
export EIGEN_POD_ADDRESS="0x18E0Dc75c318e88dc3f397e41fa715dbE7D41436"
# The JWT from your Pinata account, used to host guest binaries.
export PINATA_JWT="YOUR_PINATA_JWT"
# Optional: the IPFS Gateway URL, e.g., https://silver-adjacent-louse-491.mypinata.cloud
# default value is: https://dweb.link
export IPFS_GATEWAY_URL="YOUR_IPFS_GATEWAY_URL"
```

### Stake as a new validator

Staking requires at least 1 ether.

```bash
RUST_LOG=info ./target/release/eigen-cli stake 1
```

should output something similar to

```console
2024-09-17T17:04:20.547570Z  INFO eigen_cli: Validator created with index: 0 and stake: 1000000000Gwei
```

### Update your stake

```bash
RUST_LOG=info ./target/release/eigen-cli update-stake 0 0.1
```

should output something similar to

```console
2024-09-17T17:04:48.360064Z  INFO eigen_cli: Validator updated with index: 0 and stake: 1100000000Gwei
```

### View your current stake

```bash
RUST_LOG=info ./target/release/eigen-cli view-stake 0
```

should output something similar to

```console
2024-09-17T17:05:05.084524Z  INFO eigen_cli: Current stake: 1100000000Gwei
```

### View your yield

```bash
RUST_LOG=info ./target/release/eigen-cli view-yield 0
```

should output something similar to

```console
2024-09-17T17:05:18.944336Z  INFO eigen_cli: Current yield: 3050000
```

### Redeem your yields (aka partial withdrawal)

```bash
RUST_LOG=info ./target/release/eigen-cli redeem 0 --wait
```

should output something similar to

```console
2024-09-18T10:41:16.288582Z  INFO eigen_cli: Image ID: f9cc7ad9458fff1f60fff836967e212a4617b63eb587e244c445acc2231dcceb
2024-09-18T10:41:17.868108Z  INFO eigen_cli: Input URL: https://silver-adjacent-louse-491.mypinata.cloud/ipfs/Qmb5gXPS2aedKLePU683adcPW6uxbx5g7pRMeS9Xz6Wsak
2024-09-18T10:41:18.627363Z  INFO eigen_cli: Proving request ID: 3554585979324098154284013313896898623039163403618679259138
2024-09-18T10:45:20.554513Z  INFO eigen_cli: Partial withdrawal amount: 28900000
```

If you wish to redeem without waiting for the proof to be ready you can run this alternative

```bash
RUST_LOG=info ./target/release/eigen-cli redeem 0
```

that should output something like:

```console
2024-09-18T10:41:16.288582Z  INFO eigen_cli: Image ID: f9cc7ad9458fff1f60fff836967e212a4617b63eb587e244c445acc2231dcceb
2024-09-18T10:41:17.868108Z  INFO eigen_cli: Input URL: https://silver-adjacent-louse-491.mypinata.cloud/ipfs/Qmb5gXPS2aedKLePU683adcPW6uxbx5g7pRMeS9Xz6Wsak
2024-09-18T10:41:18.627363Z  INFO eigen_cli: Proving request ID: 3554585979324098154284013313896898623039163403618679259138
```

Now you can query the status of your proving request with ID `3554585979324098154284013313896898623039163403618679259138` by running:

```bash
RUST_LOG=info ./target/release/eigen-cli proof-status 3554585979324098154284013313896898623039163403618679259138
```

that, when fulfilled, should output something like

```console
2024-09-18T10:46:56.289445Z  INFO eigen_cli: Status: Fulfilled
```

And finally you can use that proof to redeem your funds:

```bash
RUST_LOG=info ./target/release/eigen-cli redeem-with-id 3554585979324098154284013313896898623039163403618679259138
```

that should output something like:

```console
2024-09-18T10:35:42.459282Z  INFO eigen_cli: Partial withdrawal amount: 1600000
```

### Full withdraw

```bash
RUST_LOG=info ./target/release/eigen-cli full-withdraw 0 --wait
```

should output something similar to

```console
2024-09-18T10:54:20.370887Z  INFO eigen_cli: Image ID: f9cc7ad9458fff1f60fff836967e212a4617b63eb587e244c445acc2231dcceb
2024-09-18T10:54:21.947712Z  INFO eigen_cli: Input URL: https://silver-adjacent-louse-491.mypinata.cloud/ipfs/QmUf8jQ7WugeAg87LAvnDSoRb7fEwsFEFRmymUaiGapzDr
2024-09-18T10:54:22.458994Z  INFO eigen_cli: Proving request ID: 3554585979324098154284013313896898623039163403618679259214
2024-09-18T10:58:24.574295Z  INFO eigen_cli: Full withdrawal for validator 0 with amount: 1039200000000000000
```

As for the `redeem` command, you can strip the `--wait` option and use the non-blocking approach.
