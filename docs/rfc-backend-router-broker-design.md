# RFC: Backend Router and Broker-Owned Batching

## Summary

This RFC describes the backend abstraction currently being implemented for the
broker. The design keeps protocol-facing data types generic, lets each backend
own proof semantics, and keeps broker batch lifecycle orchestration in the
broker.

The central split is:

- **Requestor SDK:** may use the standard RISC Zero-oriented builder today, or a
  fully custom `RequestBuilder` for another zkVM.
- **Broker:** receives heterogeneous orders, routes them by selector through
  `BackendRouter`, and manages per-backend batches.
- **Backend:** owns proving semantics, claim-digest computation, compression,
  seal encoding, aggregation, and assessor behavior.

## Goals

- Allow multiple backend registrations in one broker process.
- Keep selector routing in one broker-side `BackendRouter`.
- Keep batch lifecycle decisions in the broker.
- Keep aggregation selection inside the backend; the broker should not ask
  whether a selector needs aggregation.
- Avoid RISC Zero-specific receipt, journal, and set-builder types at the
  generic broker boundary.

## Non-Goals

- Redesign the external requestor SDK API.
- Make the standard request builder fully zkVM-neutral.
- Support two incompatible native zkVM SDK versions in one adapter crate if
  Cargo cannot compile them cleanly together.
- Change the on-chain protocol in this phase.

## Core Types

Generic market boundary types:

```text
ProgramId          opaque 32-byte program identity
PublicOutput       opaque execution output bytes
ClaimDigest        opaque 32-byte claim digest
ProofId            opaque backend primitive proof handle
CompressedProofId  opaque backend compressed proof handle
BackendId          stable broker-side backend identity
```

Broker-facing backend:

```text
Backend
  resolve(selector) -> BackendId
  evaluate_order(EvaluateOrder) -> OrderEvaluation
  process_order(ProcessOrder) -> OrderProcessProgress
  update_batch(UpdateBatch) -> BatchProgress
  close_batch(CloseBatch) -> CloseBatchProgress
```

The broker depends only on `Backend`. A single backend and a multi-backend
router can both implement this same interface:

```text
Broker -> Backend
            |
            +--> Risc0Backend
            |
            +--> BackendRouter -> BackendEntry -> backend implementation
```

Backend registration owns router metadata and optional internal capabilities:

```text
BackendEntry
  id
  selectors
  backend
  batch_processor?
```

`BackendEntry` is the internal registration record. The broker only needs the
resolved `BackendId` to compute backend-specific policy limits, group batches,
and route later backend calls. Proof type, if needed, is derived from the input
`VerifierSelector`; it is not duplicated in `BackendEntry`.

Order evaluation uses minimal broker-provided limits:

```text
EvaluateOrder
  order_request
  limits

EvaluationLimits
  max_cycles?          optional preflight execution limit
  max_download_bytes?  optional image/input download limit

OrderEvaluation
  backend_id
  program_id
  input_id?
  public_output
  claim_digest
  work_units
  fulfillment_data_bytes?
  estimated_duration_secs?
  estimated_cost_wei?

ProcessOrder
  backend_id
  order
  evaluation

OrderProcessProgress
  Started { proof_id }
  Compressed { proof_id, compressed_proof_id }
  Completed(ProcessedOrder)

ProcessedOrder
  backend_id
  order_id
  proof_id
  program_id
  public_output
  claim_digest
  compressed_proof_id?

UpdateBatch
  backend_id
  state?
  new_orders
  domain?
  prover_address?

BatchProgress
  backend_id
  state?
  accepted_orders
  rejected_orders
  aggregation_secs?
  assessor_secs?

CloseBatch
  backend_id
  state?
  orders

CloseBatchProgress
  Running { state }
  Closed(ClosedBatch)

ClosedBatch
  backend_id
  state?
  root_submission?
  sub_batches

FulfillmentSubBatch
  prover
  requests
  assessor_seal
  legacy_assessor_selectors?
  legacy_assessor_callbacks?
  fulfillments

FulfillmentRequest
  order_id
  request
  client_sig?

OrderFulfillment
  order_id
  request_id
  fulfillment
```

The broker computes `EvaluationLimits` from `BackendId`, requestor policy, and
market config. Priority-requestor bypass is represented by omitting the relevant
limit. The backend only enforces the limits it receives; it does not know why a
limit is present or absent.

`resolve` starts the routing chain. The broker persists the resolved
`BackendId` on accepted orders and carries it through later service commands.
The router dispatches by selector for `evaluate_order`; later calls dispatch
directly by `backend_id`.

`process_order` may return durable progress before the order is complete. The
broker persists returned proof handles and may call `process_order` again after
a restart or retry. `Started` means the primitive proof has a durable handle;
`Compressed` means any required wrapping/compression also has a durable handle.
Once `ProcessedOrder` is returned, the broker persists the public output and
other backend artifacts needed by later lifecycle commands. This avoids making
the service a generic proof-store interface while keeping long-running proof
work restart-safe.

Internally, an implementation may still decompose into a prover, batch
processor, claim-digest helper, and seal encoder. Those are implementation
details behind `Backend`, not broker dependencies.

`BatchProcessor`, when used internally, is not the broker batcher. The broker
owns batch lifecycle and decides when a backend-specific batch closes.
`BatchProcessor` is the backend-owned interpreter for the proof work inside that
broker batch.
`BatchState` is passive persisted data; it does not own prover handles or know
how to evolve itself.

The internal batch processor separates three operations without making them part
of the broker-facing API:

```text
----------------------+-----------------------------------------------+
| Aggregation state   | add_orders                                    |
| Assessor proof      | finalize                                      |
| Fulfillment build   | compress, close_batch                        |
+----------------------+-----------------------------------------------+
```

`add_orders` receives every newly claimed order for the backend batch. The
backend internally decides which orders update aggregation state and which only
participate in the assessor proof. The broker does not classify selectors into
"aggregation" and "assessor-only" paths.

`finalize` receives the current state and all orders in the broker batch. It
does not receive a separate `new_orders` list; any newly claimed orders must be
passed through `add_orders` and persisted before finalization.

Assessor journal/public-output fetching and decoding stays internal to the
backend. The broker should not need a generic `assessor_journal` method or an
assessor program id just to submit a batch.

Backend batch state carries backend proof handles:

```text
BatchState
  state                 backend-defined persisted bytes
  proof_id              optional proof to compress for root/assessor seal
  assessor_proof_id     optional proof for the assessor output
  compressed_proof_id   optional compressed proof used for submission
  selector              optional selector used for compressed proof/seal
```

`proof_id` is optional because a broker batch may contain orders before the
backend has produced an aggregation/root proof. In RISC Zero, assessor-only
batches do not get a proof to compress until finalization produces the
assessor proof.

## Broker Routing

The router implements `Backend` by selecting a `BackendEntry` from
selector or backend id metadata. A backend instance may claim many selectors.

```text
                  +----------------+
Broker request -> | Backend |
                  +----------------+
                         |
                         v
                 +----------------+
                 | BackendRouter  |
                 +----------------+
                         |
                         v
                +------------------+
                | BackendEntry     |
                | - BackendId      |
                | - selectors      |
                | - Backend object |
                | - BatchProcessor |
                +------------------+
                         |
                         v
                    Backend impl
```

The router rejects duplicate backend IDs and duplicate selectors at
registration time.

## Request Evaluation And Proving

The broker owns policy. The backend owns zkVM execution semantics.

```text
Request observed
    |
    v
Backend.resolve(request.selector)
    |
    v
Broker computes backend-specific EvaluationLimits
    |
    v
Backend.evaluate_order(EvaluateOrder { order_request, limits })
    |
    v
Broker pricing / capacity / allowlist policy
    |
    v
Order accepted
    |
    v
Backend.process_order(ProcessOrder { backend_id, order, evaluation })
    |
    +--> OrderProcessProgress::Started
    |       proof_id persisted on order
    |       broker retries/resumes later
    |
    +--> OrderProcessProgress::Compressed
    |       proof_id persisted on order
    |       compressed_proof_id persisted on order
    |       broker retries/resumes later
    |
    +--> OrderProcessProgress::Completed
            |
            v
ProcessedOrder persisted on order
    - proof_id
    - backend_id
    - program_id
    - public_output
    - claim_digest
    - optional compressed proof handle
            |
            v
        Order enters batch lifecycle:
            PendingBatch      -> ready to be claimed into a backend batch
```

The broker no longer needs to know whether a selector is "Groth16",
"set-builder", or any other backend-specific proof mode. Once `process_order`
completes, the order is simply ready to enter a backend-owned batch.

## Broker-Owned Batching

Batching is the broker lifecycle. Aggregation is an optional backend operation.
Assessment is the final backend proof over the batch fulfillment envelope.

```text
                         broker-owned
        +--------------------------------------------+
        | claim candidates                           |
        | group by BackendId                         |
        | decide when each backend batch closes      |
        | submit roots/fills/seals                   |
        +--------------------------------------------+
                         |
                        v
                       backend-owned
        +--------------------------------------------+
        | accept new batch orders                    |
        | update aggregation state where needed      |
        | finalize batch and store assessor proof id |
        | fetch/decode assessor output               |
        | compress final proof artifact              |
        | prepare submission seals and fills         |
        +--------------------------------------------+
```

Per-backend batches are tracked with `Batch.backend_id`.

```text
BackendId("risc0_v1") -> Batch #10 -> orders [...]
BackendId("risc0_v2") -> Batch #11 -> orders [...]
BackendId("other")    -> Batch #12 -> orders [...]
```

## Candidate Claiming

The DB exposes one generic claim path:

```text
claim_batch_candidates()
```

This returns all completed orders ready for backend batch handling. The DB no
longer splits candidates into "aggregation proofs" and "Groth16 proofs".

The broker groups candidates only by `BackendId` and calls:

```text
Backend.update_batch(UpdateBatch {
  backend_id,
  state,
  new_orders,
  domain,
  prover_address,
})
```

The backend decides internally how each selector participates:

```text
selector A -> update aggregation state, also assessed at finalization
selector B -> skip aggregation state, assessed at finalization
selector C -> reject as unsupported by this backend
```

This keeps selector-specific aggregation policy out of broker orchestration.

`update_batch` may return a state with no `proof_id` if the backend accepted
the orders but did not need to produce an intermediate aggregation proof.
The broker marks only `accepted_orders` as part of the broker batch. Any
`rejected_orders` are failed individually; the rest of the batch can continue.

## Batch Lifecycle

```text
OrderStatus:

PendingProving
    |
    v
Proving
    |
    v
PendingBatch
    |
    v
Batching
    |
    v
PendingSubmission
    |
    v
Done
```

```text
BatchStatus:

Open
  |
  v
PendingCompression
  |
  v
Complete
  |
  v
PendingSubmission
  |
  v
Submitted
```

`Open` means the broker batch is still accepting or waiting on orders.
`PendingCompression` means the backend has finalized/assessed the batch and the
final proof artifact must be compressed before submission.

Closing a batch is the explicit finalization call:

```text
Backend.close_batch(CloseBatch {
  backend_id,
  state,
  orders,
})
    |
    +--> CloseBatchProgress::Running
    |       state persisted on batch
    |       broker retries/resumes later
    |
    +--> CloseBatchProgress::Closed
            |
            v
ClosedBatch
  state.assessor_proof_id = proof over assessor envelope
  state.proof_id          = proof to compress for submission
  root_submission?
  sub_batches
```

The broker persists returned backend state whenever `close_batch` returns
progress. It does not persist or thread a separate assessor proof id outside the
backend state.

## Submission

The submitter gets backend-prepared artifacts instead of reconstructing
backend-specific seals itself. A small market submission adapter translates the
backend's generic fulfillment sub-batches into the contract shape deployed on
the target chain.

```text
CloseBatch
  backend_id
  state?     optional backend batch state
  orders     completed orders to fulfill
```

For a direct single-order fulfillment, `state` is `None` and `orders.len() == 1`.
For broker batch fulfillment, `state` is the persisted backend batch state and
`orders` contains the batch orders.

```text
Batch complete
    |
    v
Backend.close_batch(CloseBatch {
  backend_id,
  state,
  orders,
})
    |
    +--> optional root submission
    |       root
    |       root seal
    |
    +--> sub-batches
            - prover
            - fulfillment requests
            - assessor seal, empty if the verifier class requires none
            - optional legacy assessor selectors/callbacks
            - fulfillments keyed by order id
```

`close_batch` receives completed orders and optional persisted backend batch
state. For batched fulfillment, the backend reads its own `assessor_proof_id`
when it has one, fetches and decodes any backend-specific assessor output,
computes any backend-specific claim digests or inclusion proofs, and returns
protocol-facing fulfillment sub-batches. A sub-batch may carry an assessor seal
for classes that require an assessor adapter, or an empty assessor seal for
joint/on-chain classes that bind fulfillments without a separate assessor
receipt. The broker submitter should not need a separate per-order seal callback.

`FulfillmentRequest` is the submission-time request payload. It is equivalent to
the current `UnlockedRequest` shape plus the broker `order_id`, and is distinct
from the order-stream `Order` inbound payload. The market submission adapter
computes request digests from this request payload when a contract path needs
them; backend implementations should not own market-domain digest construction.

Contract-version compatibility stays outside the backend:

```text
ClosedBatch
    |
    v
MarketSubmissionAdapter
    |
    +--> CurrentMarketAdapter
    |       one legacy AssessorReceipt per submission
    |       uses legacy_assessor_selectors / legacy_assessor_callbacks
    |
    +--> RouterMarketAdapter
            one or more router SubBatch values
            ignores legacy assessor metadata
```

The current-market adapter is a migration bridge. It may submit one compatible
legacy transaction when the closed batch contains a single sub-batch, split
multiple sub-batches into multiple legacy transactions, or reject unsupported
mixed-class submissions. Once the router-based market is deployed everywhere,
the adapter and legacy assessor metadata can be removed.

## Current Broker Mapping

The current broker services map to the proposed service surface as follows:

```text
Market monitor
  observes ProofRequest + signature
  creates OrderRequest
  no backend-specific work

Order pricer
  resolve(selector) -> BackendId
  computes broker policy limits:
    max_cycles?
    max_download_bytes?
  evaluate_order(EvaluateOrder) -> OrderEvaluation
  applies broker policy:
    gas, collateral, allowlists, capacity, deadlines, lock strategy
  persists accepted Order with:
    backend_id
    program_id / image_id
    input_id?
    work_units / total_cycles
    fulfillment_data_bytes / journal_bytes

Proving service
  process_order(ProcessOrder) -> OrderProcessProgress
  persists Running proof handles:
    Started.proof_id
    Compressed.compressed_proof_id
  persists Completed order artifacts:
    proof_id
    compressed_proof_id?
    public_output
    claim_digest
  moves order to PendingBatch

Aggregator service
  groups PendingBatch orders by BackendId
  update_batch(UpdateBatch) -> BatchProgress
  persists backend BatchState
  fails rejected_orders individually
  broker decides when the batch closes from:
    size, time, fees, deadlines, journal/fulfillment-data size

Submitter / batch closer
  close_batch(CloseBatch) -> CloseBatchProgress
  persists Running BatchState and retries after restart
  on Closed, submits:
    optional root submission
    fulfillment sub-batches
  marks fulfilled orders complete
```

The backend interface intentionally does not own:

```text
on-chain event monitoring
lock / fulfill strategy
gas price lookup
collateral balance checks
global broker capacity
batch close timing
transaction submission and retry policy
order/batch DB schema
```

## RISC Zero Adapter

The RISC Zero implementation is one reusable backend implementation configured
with selectors, prover handles, image IDs, and wrap selector.

```text
Risc0Backend
  - prover
  - snark_prover
  - request pricing config
  - implements Backend directly, or is wrapped by BackendRouter

Risc0BatchProcessor
  - set-builder aggregation
  - assessor proving
  - assessor output decoding
  - wrap selector
```

Two RISC Zero versions should normally be two registrations of the same
implementation with different config behind a router:

```text
BackendEntry("risc0_v1", selectors_v1, Risc0Backend(config_v1), Risc0BatchProcessor(config_v1))
BackendEntry("risc0_v2", selectors_v2, Risc0Backend(config_v2), Risc0BatchProcessor(config_v2))
```

If two versions require incompatible native RISC Zero crate versions in the
same broker binary, that becomes a Cargo/adapter packaging problem rather than
a generic protocol problem.

## Requestor SDK Boundary

The requestor does not need broker-style routing by default. A requestor usually
knows which zkVM it is targeting and can use a backend-specific builder.

```text
Application
    |
    +--> StandardRequestBuilder       (current RISC Zero-oriented path)
    |
    +--> ThirdPartyRequestBuilder     (custom zkVM path)
```

The stable requirement is that any request builder produces protocol-valid
`ProofRequest`s and fulfillment data that the selected backend/verifier expects.

## Open Questions

- Should `BatchCandidate.status` remain visible to the aggregator service, or
  should the service only receive neutral candidate data?
- Should telemetry preserve old field names for compatibility, or migrate fully
  to `aggregation_secs`, `assessor_secs`, and
  `batch_compression_secs`?
- Should `Backend::evaluate_order` eventually return mandatory
  normalized cost/duration fields instead of optional estimates?
- Where should the explicit non-RISC-Zero requestor API live once the SDK
  redesign starts?
