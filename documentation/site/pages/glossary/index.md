---
title: Glossary
description: A list of terms used in the Boundless documentation.
---

# Glossary

## Assessor

A [guest program][r0-term-guest-program] that will verify the application receipt through composition and check that it satisfies the given requirements. Using this method, the full request does not need to be provided as part of fulfillment, only the associated identifier.

## Aggregation

In order to amortize the Boundless' _on-chain verification cost_, a recursive verification protocol is used to verify multiple independent [receipts][r0-term-receipt] such that a single receipt attests to every claim in a set of proofs.
Further improving efficiency of inclusion proofs on-chain, this process builds a binary Merkle tree of the receipt claims.

> See `crates/aggregation-set/src/lib.rs` and `crates/guest/set-builder/set-builder-guest/src/main.rs` for details.

## Boundless Market

A coordination and clearing mechanism to that connects those requesting proofs generation, along with a commitment of payment, with and those able to fulfill with proof generation and receive payment.

In the initial 0th version, the Market is facilitated on-chain where one is \[deployed]\[page-deployments], but it is expected to evolve into more efficient off-chain mechanisms in future versions.

> See the [Market Section][page-boundless-market] for more details.

## Bento

A cluster of services that coordinate to search for, bid on, and attempt to fulfil [proof orders](#proof-order).

> See the [Bento Section][page-bento] for more details.

## Broker

The Broker monitors a [deployment][page-deployments] of the [Boundless Market](#boundless-market) and, based on customizable criteria, bids on and locks-in on proof requests. Proof generation jobs are subsequently passed to an instance of [Bento](#bento), and ultimately are the request(s) are fulfilled it on the Market.

> See the [Broker Section][page-broker] for more details.

## Preflight

Running a proof request's execution _only_ via [Bento](#bento) (essential using [RISC Zero's `dev-mode`][r0-page-dev-mode]) in order to calculate the required [cycles][r0-term-clock-cycles] for the [proof order](#proof-order).

This allows one to:

- Validate an order is possible to fulfill at all (execution completes)
- Confirm execution results match the [proof order](#proof-order) requirements
- Calculate - based on custom heuristics - the bid to lock-in on a [market](#boundless-market) order fulfillment.

## Prover

The market participant that fulfills [proof orders](#proof-order).

## Proof Order

<!-- TODO https://linear.app/risczero/issue/BM-201/replace-proof-request-with-order -->

An order - also called a request - placed on the [Boundless Market](#boundless-market) to that includes:

- A Unique ID for the request on the Market
- Proof Requirements for a this order to be fulfilled, including the [Image ID][r0-term-image-id]
- A URL where the [ELF Binary][r0-term-elf-binary] for the program with required Image ID's can be retrieved by the [Prover](#prover)
- [Guest Program][r0-term-guest-program] inputs
- An Offer specifying remuneration for successful order fulfillment

See `contracts/src/IBoundlessMarket.sol` for more details.

## Requestor

<!-- TODO https://linear.app/risczero/issue/BM-202/replace-instances-of-client-with-requestor -->

Also referred to as the Client in the context of contracts, the party submitting orders to the market proofs form the Boundless Market.

[page-bento]: /prover-manual/bento/introduction
[page-boundless-market]: /market/introduction
[page-broker]: /prover-manual/broker/introduction
[page-deployments]: /market/public-deployments
[r0-page-dev-mode]: https://dev.risczero.com/api/next/generating-proofs/dev-mode
[r0-term-clock-cycles]: https://dev.risczero.com/terminology#clock-cycles
[r0-term-elf-binary]: https://dev.risczero.com/terminology#elf-binary
[r0-term-guest-program]: https://dev.risczero.com/terminology#guest-program
[r0-term-image-id]: https://dev.risczero.com/terminology#image-id
[r0-term-receipt]: https://dev.risczero.com/terminology#receipt
