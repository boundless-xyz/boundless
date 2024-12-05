---
title: Build a Program
description: This page covers the Boundless Foundry Template program as a quick start.
---

# Build a Program

At its core, Boundless allows app developers to receive zero-knowledge (ZK) proofs for their programs. Before [requesting a proof](/build/request-a-proof), developers should have a program that compiles and executes successfully. This page covers the Boundless Foundry Template as a quick start.

If you want to read more about the zkVM, it is highly recommended to read the following pages on the [zkVM dev docs](https://dev.risczero.com/api):

- [Introduction](https://dev.risczero.com/api)
- [What is a zkVM Program?](https://dev.risczero.com/api/zkvm)
- zkVM
  - [Installation](https://dev.risczero.com/api/zkvm/install)
  - [Quick Start](https://dev.risczero.com/api/zkvm/quickstart)
  - [Hello World](https://dev.risczero.com/api/zkvm/tutorials/hello-world)

For some more intermediate and advanced reading on the zkVM, please refer to:

- [Optimizing your zkVM Program](https://dev.risczero.com/api/zkvm/profiling)
- [zkVM Technical Specification](https://dev.risczero.com/api/zkvm/zkvm-specification)
- [The RISC Zero STARK Protocol](https://dev.risczero.com/proof-system/proof-system-sequence-diagram)

:::tip[Tip]
If you have any technical questions about the zkVM, please join [RISC Zero's Discord](https://discord.com/invite/risczero) and ask away. 👋
:::

## Boundless Foundry Template

The Boundless Foundry Template builds on [RISC Zero's Foundry Template](https://github.com/risc0/risc0-foundry-template) to incorporate Boundless with a simple example app. It consists of three main parts:

- the zkVM program, [`is-even`](#is-even)
- the application smart contract, [`EvenNumber.sol`](#evennumbersol)
- the application backend, [`app`](#appssrcmainrs)
- [`guests/is-even/src/main.rs`](https://github.com/boundless-xyz/boundless-foundry-template/blob/main/guests/is-even/src/main.rs)

The example app executes the following steps:

1. Uploads the guest program and creates the proof request.
2. Sends the proof request to Boundless.
3. Retrieves the proof from Boundless.
4. Sends the proof to the application contract.

@TODO--PICTURE\_GOES\_HERE

The zkVM program for this example app is a simple program that takes an input number, checks if it is even and if so, outputs the number to the public outputs of the computation (known as the [journal](https://dev.risczero.com/terminology#journal)).

The entire program is only \~20 lines, so let's run through it:

### `is-even`

```rust
use std::io::Read;

use alloy_primitives::U256;
use alloy_sol_types::SolValue;
use risc0_zkvm::guest::env;

fn main() {
    // Read the input data for this application.
    let mut input_bytes = Vec::<u8>::new();
    env::stdin().read_to_end(&mut input_bytes).unwrap();

    // Decode and parse the input
    let number = <U256>::abi_decode(&input_bytes, true).unwrap();

    // Run the computation.
    // In this case, asserting that the provided number is even.
    assert!(!number.bit(0), "number is not even");

    // Commit the journal that will be received by the application contract.
    // Journal is encoded using Solidity ABI for easy decoding in the app contract.
    env::commit_slice(number.abi_encode().as_slice());
}
```

First, the guest receive inputs from the host program, and then these input bytes are decoded and parsed:

```rust
fn main() {
    // Read the input data for this application. // [!code focus]
    let mut input_bytes = Vec::<u8>::new(); // [!code focus]
    env::stdin().read_to_end(&mut input_bytes).unwrap(); // [!code focus]

    // Decode and parse the input // [!code focus]
    let number = <U256>::abi_decode(&input_bytes, true).unwrap(); // [!code focus]
    ....
}
```

To check if the number is even:

```rust
fn main() {
    ....
    // Run the computation. // [!code focus]
    // In this case, asserting that the provided number is even. // [!code focus]
    assert!(!number.bit(0), "number is not even"); // [!code focus]
    ....
}
```

Finally, if the number is even, the assert will pass and the number can be committed to the [journal](https://dev.risczero.com/terminology#journal).

### [`EvenNumber.sol`](https://github.com/boundless-xyz/boundless-foundry-template/blob/main/contracts/src/EvenNumber.sol)

The `EvenNumber` smart contract holds a `uint256` variable called `number`. This number is guaranteed to be even, however the smart contract itself never checks the number's parity directly. It verifies a ZK proof of a program that has checked if the number is even. This is done in the `set` function:

```solidity
/// @notice Set the even number stored on the contract. Requires a RISC Zero proof that the number is even.
function set(uint256 x, bytes calldata seal) public {
    // Construct the expected journal data. Verify will fail if journal does not match.
    bytes memory journal = abi.encode(x);
    verifier.verify(seal, imageId, sha256(journal));
    number = x;
}
```

This `set` function is called with two arguments, a number `x`, and the `seal`. The number `x` is the number that the ZK proof has proven is even, and it is used to reconstruct the journal (the [journal](https://dev.risczero.com/terminology#journal) refers to the public outputs of the program). The `seal` is the proof. In [zkVM terminology](https://dev.risczero.com/terminology#seal), the seal usually refers to a zk-STARK or SNARK directly. In Boundless, the seal is actually a Merkle inclusion proof that the program's proof verification has been included in the Batch Verification Tree. See [Proof Lifecycle](introduction/proof-lifecycle) to read more about this.

#### Proof Verification Using the VerifierRouter Contract

The main logic of this function is carried out with `verifier.verify`:

```solidity
/// @notice Set the even number stored on the contract. Requires a RISC Zero proof that the number is even.
function set(uint256 x, bytes calldata seal) public {
    // Construct the expected journal data. Verify will fail if journal does not match.
    bytes memory journal = abi.encode(x);
    verifier.verify(seal, imageId, sha256(journal)); // [!code focus]
    number = x;
}
```

- Verifier
  - The `verifier` variable points to the [RISC Zero Verifier Router contract](https://dev.risczero.com/api/blockchain-integration/contracts/verifier). This contract supports verification of seals that are either proofs returned from Boundless or zkSNARKs directly from the zkVM.
- [ImageID](https://dev.risczero.com/terminology#image-id)
  - The image ID is a unique identifier for a program in the zkVM. It is checked during proof verification to ensure proof integrity. Concretely, by saving the image ID to an immutable variable in the smart contract on deployment, this makes sure that only proofs from the correct program (in this case, `is-even`) are valid.
- [Journal](https://dev.risczero.com/terminology#journal)
  - The journal refers to the public outputs of the program. Similar to the image ID, the journal ensures proof consistency by verifying that the journal in the [receipt claim](https://dev.risczero.com/terminology#receipt-claim) (the seal cryptographically attests to the receipt claim) matches the expected journal.

:::note[Summary]
The `EvenNumber.sol` smart contract has one main function: `set`. This function will update the state `number` _if and only if_ a valid ZK proof is verified from the correct zkVM program (identified via the image ID).
:::

### [`apps/src/main.rs`](https://github.com/boundless-xyz/boundless-foundry-template/blob/main/apps/src/main.rs)

The app is run as a CLI, with the arguments:

- `number`: The value to run through the guest program and then to publish to the `EvenNumber` contract.
- `rpc_url`: Ethereum node RPC endpoint.
- `wallet_private_key`: The address used for on-chain interactions (i.e. requesting a proof, sending the proof to the `EvenNumber` contract for verification).
- `even_number_address`: Deployed `EvenNumber` contract address.
- `set_verifier_address`: Deployed `SetVerifier` contract address.
- `proof_market_address`: Deployed `ProofMarket` contract address.

For the relevant contract address, please see the [Public Deployments](/public-deployments) page.

For a detailed walkthrough of this app file, please refer to [Request A Proof](/build/request-a-proof), where every step of this program is explained in detail.
