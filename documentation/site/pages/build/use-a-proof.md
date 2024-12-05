---
title: Use a Proof
description: After receiving the Boundless proof, you are ready to use it in your app to access verifiable compute directly.
---

# Use a Proof

> Prerequisite Reading: [Request a Proof](/build/request-a-proof), [Proof Lifecycle](/introduction/proof-lifecycle)

After [requesting a proof](/build/request-a-proof), the next step is to use that proof in the application's workflow. The exact way proofs are used will vary depending on the architecture of the application. However, there is a common pattern; once a proof is received from the Boundless market, the next step will be to verify that proof on-chain.

@TODO--PICTURE\_GOES\_HERE

It is recommended that the application contract calls the [RiscZeroVerifierRouter](https://dev.risczero.com/api/blockchain-integration/contracts/verifier) for verification. This allows handling many types of proofs, and proof system versions seamlessly. In Boundless, the seal, which is often a zk-STARK or SNARK,  will usually be Merkle inclusion proof into an aggregated proof. These Merkle inclusion proofs are cheap to verify, and reuse a cached verification result from a batch of proofs verified with a single SNARK.

## Proof Verification

The [Boundless Foundry Template](https://github.com/boundless-xyz/boundless-foundry-template/blob/main/contracts/src/EvenNumber.sol), walks through a simple application which, with an input number, _x_:

1. Uses a simple guest program to check if _x_ is even.
2. Requests, and receives, a proof of _x_ being even from the Boundless Market.
3. Calls the `set` function on the `EvenNumber` smart contract with the arguments: _x_ and the seal (the proof bytes).
4. The `set` function verifies the proof that _x_ is even; if the proof is valid, the `number` variable (in smart contract state) is set to equal _x_.

Concretely, receiving the proof ([see code](https://github.com/boundless-xyz/boundless-foundry-template/blob/859248fd748f1c8ca88f9540de389c4d86bd959a/apps/src/main.rs#L149)) from the Boundless Market returns a journal and a seal:

```rust
let (_journal, seal) = boundless_client
    .wait_for_request_fulfillment(request_id, Duration::from_secs(5), None)
    .await?;
```

Using Alloys [sol! Macro](https://alloy.rs/examples/sol-macro/index.html), the rust types/bindings are generated for the [`EvenNumber.sol`](https://github.com/boundless-xyz/boundless-foundry-template/blob/main/contracts/src/EvenNumber.sol) contract.
To create an `EvenNumber` contract instance:

```rust
let even_number = IEvenNumberInstance::new(
    args.even_number_address,
    boundless_client.provider().clone(),
);
```

To call the `set` function on the `EvenNumber` contract, a “set number” transaction is created:

```rust
let set_number = even_number
    .set(U256::from(args.number), seal)
    .from(boundless_client.caller());
```

Finally, this transaction is broadcasted with:

```rust
let pending_tx = set_number.send().await.context("failed to broadcast tx")?;
let tx_hash = pending_tx
    .with_timeout(Some(TX_TIMEOUT))
    .watch()
    .await
    .context("failed to confirm tx")?;
tracing::info!("Tx {:?} confirmed", tx_hash);
```

Definition of the `set` function on [`EvenNumber.sol`](https://github.com/boundless-xyz/boundless-foundry-template/blob/main/contracts/src/EvenNumber.sol):

```solidity [EvenNumber.sol]
/// @notice Set the even number stored on the contract. Requires a RISC  Zero proof that the number is even.
function set(uint256 x, bytes calldata seal) public {
  bytes memory journal = abi.encode(x);
  verifier.verify(seal, imageId, sha256(journal));
  number = x;
}
```

Calling the [`set` function](https://github.com/boundless-xyz/boundless-foundry-template/blob/859248fd748f1c8ca88f9540de389c4d86bd959a/contracts/src/EvenNumber.sol#L34C1-L40C6) will verify the proof via the [RISC Zero verifier contract](https://dev.risczero.com/api/blockchain-integration/contracts/verifier). The `verify` call will revert if the proof is invalid, otherwise the number variable will be updated to x, which is now certainly even.

Each application will have its own requirements and flows, but this is a common pattern and a good starting point for building your own application.

> Relevant links: [Boundless Foundry Template](https://github.com/boundless-xyz/boundless-foundry-template/tree/main), [Journal](https://dev.risczero.com/terminology#journal), [Seal](https://dev.risczero.com/terminology#seal)
