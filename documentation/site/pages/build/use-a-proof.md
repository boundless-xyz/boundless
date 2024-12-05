---
title: Use a Proof
description: After receiving the Boundless proof, you are ready to use it in your app to access verifiable compute directly.
---

# Use a Proof

> Prerequisite Reading: [Request A Proof](/build/request-a-proof), [Proof Lifecycle](/introduction/proof-lifecycle)

@TODO: VERIFY THIS BLOCK

:::info
A proof returned from Boundless is not directly a zk-STARK/SNARK. It is an attestation that we call a “set inclusion proof/receipt”. Concretely, it is a proof that the given receipt is in a verified set (see setVerifier). This is an efficient way to aggregate proofs directly within the Boundless Market.

The term “proof verification” on this page refers to verifying the Merkle Proof, and not verification of a ZK proof. Nevertheless, this proof has to still be verified on-chain by using the RiscZeroVerifierRouter. To read more, please refer to Proof Lifecycle.
:::

@TODO-END

After [requesting a proof](/build/request-a-proof), the next step is to use that proof in the application's workflow. The exact way proofs are used will vary depending on the architecture and complexity of the application. However, there will be a common pattern; once a proof is received from the Boundless market, the next step will be to verify that proof on-chain.

## Proof Verification

The [Boundless Foundry Template](https://github.com/boundless-xyz/boundless-foundry-template/blob/main/contracts/src/EvenNumber.sol), walks through a simple application which, with an input number, `x`:

1. Uses a simple guest program to check if `x` is even.
2. Requests, and receives, a proof of `x` being even from the Boundless Market.
3. Calls the `set` function on the `EvenNumber` smart contract with the arguments: `x` and the seal (the proof bytes).
4. The `set` function verifies the proof that `x` is even; if the proof is valid, the number variable (in smart contract state) is set to equal `x`.

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

Set Function on `EvenNumber.sol`

```solidity [EvenNumber.sol]
/// @notice Set the even number stored on the contract. Requires a RISC Zero proof that the number is even.
function set(uint256 x, bytes calldata seal) public {
  bytes memory journal = abi.encode(x);
  verifier.verify(seal, imageId, sha256(journal));
  number = x;
}
```

Calling the [`set` function](https://github.com/boundless-xyz/boundless-foundry-template/blob/859248fd748f1c8ca88f9540de389c4d86bd959a/contracts/src/EvenNumber.sol#L34C1-L40C6) will verify the proof (via RISC Zero's [verifier router](https://dev.risczero.com/api/blockchain-integration/contracts/verifier)). The `verify` call will revert if the proof is invalid, otherwise the number variable will be updated to `x`, which is now certainly even.
