# Smart Contract Requestor Example

This example shows how the Boundless Market's support for smart contract signatures can be used to enable permissionless proof request submission by 3rd parties that are authorized for payment by a smart contract.

This is a useful pattern for enabling DAO-like entities to receive proofs to drive the operation of their protocol. This pattern can also be used to create service agreements, where the contract authorizes the funding of proofs that meet a certain criteria.

# Example

In this simple example, our Smart Contract Requestor agrees to pay for one proof of the Echo guest per day. For each day, it additionally requires that the input to the guest is an integer representing the number of days since the unix epoch.

See `apps/src/main.rs` for logic for constructing the proof request.
See `contracts/src/SmartContractClient.sol` for the client logic that validates the request and authorizes payment.

# How it works

### Entities

- Request Builder
  - Responsible for building the proof request and submitting it to the market. This is a fully permissionless role. Request builders are expected to be incentivized to fulfill this role outside of the Boundless protocol.
- Smart Contract Requestor
  - An ERC-1271 contract that contains the logic for authorizing a proof request, and has deposited funds to Boundless Market for fulfilling requests.
- Provers
  - Regular provers in the market who see these requests and are responsible for fulfilling by the deadline.

### High Level Flow

1. The Request Builder constructs the request, ensuring it abides by the criteria specified in the Smart Contract Requestor (e.g. uses a particular image id, has a particular requirement, etc.)
2. The Request Builder submits the request, specifying the client of the request to be the address of the Smart Contract Requestor. They also provide a signature that encodes the data the Smart Contract Requestor requires to validate the request submitted meets its criteria.
3. When the Boundless Market validates the request, it calls ERC-1271's `isValidSignature` function on the Smart Contract Requestor, providing a hash of the request it received, and the data that was submitted by the Request Builder.
4. The Smart Contract Requestor uses the data provided to reconstruct the hash, and checks it matches the hash provided from Boundless Market. If so it authorizes the request, and the Boundless Market takes payment.
5. Provers see the request and fulfill it as normal.

### Request ID

In Boundless, Request IDs are specified by the request builder. The Boundless Market contract ensures that only one payment will ever be issued for each request id.

For Smart Contract Requestors, the Request ID is especially important as it acts as a nonce, ensuring the requestor does not pay twice for the same batch of work. It is important to design a nonce structure that maps each batch of work to a particular nonce value, and for the Smart Contract Requestor to validate that the work specified by the Request ID matches the work specified in the proof request.

In this example, we use the Request ID to represent "days since epoch". Our zkVM guest program outputs the input that it was called with, so we use this property to ensure that the program was run with the correct input for the day.

## Build

```bash
forge build
cargo build
```

## Test

To run an end to end test locally using Anvil and R0 developer mode use:

```bash
RUST_LOG=info RISC0_DEV_MODE=true cargo test
```
