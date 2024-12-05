---
title: Market Design
description: Market contract, order matching, and more.
---

# Market Design

## Market Contract and Guest

Market operations such as the auction and settlement are implemented in a smart contract.

### Request Placement

Request broadcast: Requestors will initiate an request by broadcasting a ProvingRequest to the provers. Requestors have a choice of two broadcast channels depending on their needs:

Broadcast via EVM calldata. This has the highest possible assurance for data availability and censorship resistance.

Broadcast via an off-chain broadcast channel. This has the lowest cost and latency.

Off-chain broadcast channel is not a requirement for the initial MVP deployment. It is however important to keep it in mind. Submitting a request cannot alter EVM state.

### Authentication

Requests are signed by the requestor EOA with an EIP-712 signature.

## Request Matching

### Auction

Provers bid on requests in a reverse Dutch auction, where the price the requestor is offering starts at some low initial amount, and is raised over the period of time that the auction is open until some max price is reached. If at any point in the auction a prover submits a bid, the current price is used as the final price and the auction is closed. In this way, only a single bid is ever sent to the blockchain. This occurs in the form of a lock-in request that also grants the prover exclusive rights to be paid for the request.

### Exclusivity

A prover may submit a transaction to lock-in a request. In order to do so, they must supply an amount of stake (i.e. collateral) specified by the requestor. If the prover does not deliver a proof by the deadline specified in the request, the prover's stake is burned. In this way, provers can be assured they will be paid for their proving work, as opposed to having to race others, while ensuring provers are incentivized only to take work they can actually complete.

### Escrow

As part of the lock-in, funds are deducted from both the requestor and prover accounts. Funds are deducted from the requestor account and held in escrow on the market to ensure the prover will be paid upon delivering the proofs. Funds are deducted from the prover account to cover any lock-in stake specified by the requestor. These funds are sent to the prover upon fulfillment of the request. If the deadline passes, the price of the proof can be returned to the requestor, and the prover stake is burned.

## Request Fulfillment

### Requirements Checking

Once the prover has a proof that satisfies the requirements they will run the Assessor, which is a guest program that will verify the application receipt through composition and check that it satisfies the given requirements. Using this method, the full request does not need to be provided as part of fulfillment, only the associated identifier.

### Guaranteed Delivery

In order to settle the requesr and receive payment, the prover must submit a receipt that meets the requirements of the request. This receipt is posted to the chain in calldata as part of the settlement transaction as a form of "guaranteed delivery". In this way, the receipt is now public such that the requestor can query the blockchain to receive it. In general, any data availability solution would also work here.

### Aggregation and Verification Caching

One of the most expensive parts of using a SNARK in a smart contract application is the verification cost. This cost is amortized across a batch of requests by recursively verifying a set of receipts, and constructing a Merkle tree over the claims for efficient inclusion proofs. Additionally, it is assumed that the requestor will use the receipt they receive to drive on-chain functionality which will require them to verify the receipt as part of that application flow. In order to guarantee this is efficient, the market uses a set verifier that caches the root of the receipt claim Merkle tree, and the receipt used by the application is a Merkle inclusion path against that root.

## Proof Aggregation

Verification of SNARKs in the EVM is expensive. In the case of our Groth16 receipts, it costs around 250k gas, which makes it the largest single cost for more applications.

In order to amortize this on-chain verification cost, we use recursive verification to verify a set of receipts such that a single receipt attests to every claim in the set. In order to make inclusion proofs efficient, this process builds a binary Merkle tree of the receipt claims.

### Set Verifier Contract

In the EVM, the Groth16 receipt of set builder execution for the root of the Merkle tree is verified, and the verified root is written to EVM storage. Once this root is recorded, a Merkle inclusion path can act as a proof for the individual claims in the set. In this way, the set verifier implements the IRiscZeroVerifier interface, accepting this Merkle inclusion path as the seal in the verify call.

## Assessor

In order to fulfill a request, the prover must show that they have a signed request, and a receipt that meets the requirements of that request. This could be accomplished by having the prover post the signed request, including offer and requirements, in the fulfillment transaction and verify this invariant on-chain. However, this is expensive and results in data posted that is not strictly needed to transition the state of the contract (i.e. to pay the prover and mark the request as fulfilled).

In order to make this more efficient, we introduce the Assessor, which is a guest program that enforces these checks.

As input, the Assessor accepts:

- A list of requests, including offer and requirements, that the prover has completed.
- A list of image IDs and journals which the prover has assembled into a Merkle tree using the Aggregator.
- The smart contract address for the market that will be posted to. This smart contract address is used solely to construct the EIP-712 Domain and complete signature checks on the requests.
- As a special case, the Assessor will accept a single request and receipt claim as a list of one.

During execution, the Assessor checks:

- Each request is signed by the requestor address embedded in the request ID.
- The journal and image ID provided to fulfill the request meet the requirements (i.e. that the image ID matches, and the journal satisfies the provided predicate).
- The root of the Merkle tree constructed from the list of receipt claims, is verified by a receipt from the set builder guest. This step is checked by composition using the env::verify API.
- The Assessor commits to the journal an EVM ABI encoded structure that includes the request IDs of the requests it checked, as well as the root for the Merkle tree formed by the receipt claims.

After the Assessor is executed and proven, it is added to a running batch for efficient on-chain verification. A diagram showing the Merkle tree constructed by the set builder for aggregate verification.
