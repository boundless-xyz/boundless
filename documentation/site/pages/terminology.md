---
title: Terminology
description: A list of terms used in the Boundless documentation.
---

# Terminology

## Aggregation

In order to amortize the Boundless' on-chain verification cost, a recursive verification protocol is used to verify multiple independent [receipts][r0-term-receipt] such that a single receipt attests to every claim in a set of proofs. Further improving efficiency of inclusion proofs on-chain, this process builds a binary Merkle tree of the receipt claims.

## Boundless Market

A coordination and clearing mechanism to that connects those requesting proofs generation, along with a commitment of payment, with and those able to fulfill with proof generation and receive payment.

In the initial 0th version, the Market is facilitated on-chain where one is [deployed][page-deployments], but it is expected to evolve into more efficient off-chain mechanisms in future versions.

See the [Market Section][page-boundless-market] for more details

## Preflight

Running a proof request's execution only (e.g. using [RISC Zero's dev-mode][r0-page-dev-mode]) in order to calculate the required [cycles][r0-term-clock-cycles] for the proof order.

This allows one to:

Validate an order is possible to fulfill at all (execution completes)
Confirm execution results match the proof order requirements
Calculate - based on custom heuristics - the bid to lock-in on a market order fulfillment.

## Prover

The market participant that fulfills proof order.

## Proof Request

A request - sometimes called an order - placed on the Boundless Market to that includes:

- A Unique ID for the request on the Market
- Proof Requirements for a this order to be fulfilled, including the [Image ID][r0-term-image-id]
- A URL where the [ELF Binary][r0-term-elf-binary] for the program with required Image ID's can be retrieved by the Prover
- [Guest Program][r0-term-guest-program] inputs
- An Offer specifying remuneration for successful order fulfillment

## Requestor

Also referred to as the Client in the context of contracts, the party submitting orders to the market proofs form the Boundless Market.

[page-bento]: ./prover-manual/bento/README.md
[page-boundless-market]: ./market/README.md
[page-broker]: ./prover-manual/broker/README.md
[page-deployments]: ./market/deployments.md
[r0-page-dev-mode]: https://dev.risczero.com/api/next/generating-proofs/dev-mode
[r0-term-clock-cycles]: https://dev.risczero.com/terminology#clock-cycles
[r0-term-elf-binary]: https://dev.risczero.com/terminology#elf-binary
[r0-term-guest-program]: https://dev.risczero.com/terminology#guest-program
[r0-term-image-id]: https://dev.risczero.com/terminology#image-id]
[r0-term-receipt]: https://dev.risczero.com/terminology#receipt
