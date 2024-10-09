# Glossary

### Assessor

A [guest program][r0-term-guest-program] that will verify the application receipt through composition and check that it satisfies the given requirements. Using this method, the full request does not need to be provided as part of fulfillment, only the associated identifier.

### Aggregation

In order to amortize the Boundless' _on-chain verification cost_, a recursive verification protocol is used to verify multiple independent [receipts][r0-term-reciept] such that a single receipt attests to every claim in a set of proofs.
Further improving efficiency of inclusion proofs on-chain, this process builds a binary Merkle tree of the receipt claims.

> See `crates/aggregation-set/src/lib.rs` and `crates/guest/set-builder/set-builder-guest/src/main.rs` for details.

### Bento

A cluster of services that coordinate to search for, bid on, and attempt to fulfil [proof order](#proof-order)s.

> See the [Bento Documentation][page-bento] for moe details.

### Broker

A cluster of services that coordinate to search for, bid on, and attempt to fulfil [proof order](#proof-order)s.

> See the [Bento Documentation][page-bento] for moe details.

### Prover

The market participant that fulfills [proof order](#proof-order)

### Proof Order

<!-- TODO https://linear.app/risczero/issue/BM-201/replace-proof-request-with-order -->

An order placed on the [Boundless Market](#boundless-market) to that includes:

- A Unique ID for the request on the Market
- Proof Requirements for a this order to be fulfilled, including the [Image ID][r0-term-image-id]
- A URL where the [ELF Binary][r0-term-elf-binary] for the program with required Image ID's can be retrieved byt the [Prover](#prover)
- [Guest Program][r0-term-guest-program] inputs
- An Offer specifying remuneration for successful order fulfillment

See `contracts/src/IProofMarket.sol` for more details.

### Requestor

<!-- TODO https://linear.app/risczero/issue/BM-202/replace-instances-of-client-with-requestor -->

Also referred to as the Client in the context of contracts, the party submitting orders to the market proofs form the Boundless Market.

### Preflight

is this specific and need a name? (run execution to check cost and see if execution is valid at all)

[r0-term-image-id]: https://dev.risczero.com/terminology#image-id
[r0-term-guest-program]: https://dev.risczero.com/terminology#guest-program
[r0-term-elf-binary]: https://dev.risczero.com/terminology#elf-binary
[r0-term-reciept]: https://dev.risczero.com/terminology#receipt
[page-bento]: ./prover-manual/bento/README.md
