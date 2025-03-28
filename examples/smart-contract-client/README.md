# Smart Contract Client Example

This example shows how the Boundless Market's support for smart contract signatures can be used to enable permissionless proof request submission by 3rd parties that are authorized for payment by a smart contract.

This is a useful pattern for enabling DAO-like entities to receive proofs to drive the operation of their protocol. This pattern can also be used to create service agreements, where the contract authorizes the funding of proofs that meet a certain criteria.

# Example

In this simple example, our Smart Contract Client agrees to pay for one proof of the Echo guest per day. For each day, it additionally requires that the input to the guest is an integer representing the number of days since the unix epoch.

See `apps/src/main.rs` for logic for constructing the proof request.
See `contracts/src/SmartContractClient.sol` for the client logic that validates the request and authorizes payment.

# How it works

### Entities

- Request Builder
  - Responsible for building the proof request and submitting it to the market. This is a fully permissionless role. Request builders are expected to be incentivized to fufill this role outside of the Boundless protocol.
- Smart Contract Client
  - An ERC-1271 contract that contains the logic for authorizing a proof request, and has deposited the funds for fulfilling requests.
- Provers
  - Regular provers in the market who see these requests and are responsible for fulfilling them in time.

### High Level Flow

1. The Request Builder constructs the request, ensuring it abides by the criteria specified in the Smart Contract Client (e.g. uses a particular image id, has a particular requirement, etc.)
2. The Request Builder submits the request, specifying the client of the request to be the address of the Smart Contract Client. They also provide a signature that encodes the data the Smart Contract Client requires to validate the request submitted meets its criteria.
3. When the Boundless Market validates the request, it calls ERC-1271's `isValidSignature` function on the Smart Contract Client, providing a hash of the request it received, and the data that was submitted by the Request Builder.
4. The Smart Contract Client uses the data provided to reconstruct the hash, and checks it matches the hash provided from Boundless Market. If so it authorizes the request, and the Boundless Market takes payment.
5. Provers see the request and fulfill it as normal.

### Request ID

In Boundless, Request IDs are specified by the request builder. The Boundless Market ensures that only one payment will ever be issued for each request id.

For Smart Contract Clients, the Request ID is especially important as it acts as a nonce. It is important to design a nonce structure that maps each batch of work to a particular nonce value, and for the Smart Contract Client to validate that the work specified by the Request ID matches the work specified by the proof request.

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
