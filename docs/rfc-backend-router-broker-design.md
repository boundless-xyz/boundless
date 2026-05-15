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

## Responsibility Boundary

The design goal is clean ownership, not simply moving functions behind a trait.
The broker owns orchestration. Backends own proof semantics. Contract adapters
own transaction and ABI shape.

```text
Broker workers
  - observe market/order-stream events
  - decide whether to accept work
  - enforce broker policy, capacity, pricing, allowlists, and deadlines
  - persist order and batch lifecycle state
  - decide when backend-specific batches close
  - retry, cancel, and record telemetry
  - submit transactions

Backend implementations
  - identify supported selectors
  - evaluate backend-specific feasibility and cost signals
  - produce per-order proof artifacts
  - maintain backend-specific batch state
  - compute backend-specific claim digests and seals
  - produce batch/fulfillment artifacts consumed by submission

Market submission adapters
  - map backend artifacts to deployed contract calls
  - bridge current market contracts and future verifier-router contracts
  - compute market-domain request digests when needed by the contract path
```

This means broker workers may have names like `OrderEvaluator`,
`OrderProcessor`, `AggregatorService`, and `Submitter`, but they are not backend
implementations. `OrderProcessor` owns DB transitions, cancellation, retry,
telemetry, fulfillment-event cancellation, and capacity slot completion, then
delegates zkVM-specific proof work to `BackendRouter`.

`Submitter` remains the broker submission worker. It should eventually consume
backend-prepared submission artifacts and use a market adapter, but it still
owns transaction retry/classification and order completion side effects.

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
BackendId          stable broker-side backend identity, e.g. "risc0_v3"
```

Single-backend implementation contract:

```text
Backend
  id() -> BackendId
  supports(selector) -> bool
  process_order(ProcessOrder) -> OrderProcessProgress
  cancel_order(Order) -> ()
  estimate_batch_size(order_ids) -> BatchSizeEstimate
  update_batch(UpdateBatch) -> BatchUpdate
  close_batch(CloseBatch) -> BatchClose
  build_fulfillments(FulfillmentBatch) -> FulfillmentArtifacts
```

Broker-facing router:

```text
BackendRouter
  register_backend(BackendEntry)
  backend_ids() -> Vec<BackendId>
  process_order(ProcessOrder) -> OrderProcessProgress
  cancel_order(Order) -> ()
  estimate_batch_size(BackendId, order_ids) -> BatchSizeEstimate
  update_batch(BackendId, UpdateBatch) -> BatchUpdate
  close_batch(BackendId, CloseBatch) -> BatchClose
  build_fulfillments(FulfillmentBatch) -> FulfillmentArtifacts
```

Broker services depend on `BackendRouter`. `BackendRouter` is the registry and
selector router: it owns the backend objects, resolves selector/order-routed
calls, and exposes router methods keyed by `BackendId` for per-backend batch
processing. `BackendObj` is internal to the router module; broker workers should
not receive backend trait objects directly.

```text
OrderProcessor  -> BackendRouter -> Backend
Aggregator      -> BackendRouter -> Backend
Submitter       -> BackendRouter -> Backend
```

Backend registration owns only router metadata plus the backend object:

```text
BackendEntry
  id
  selectors
  backend
```

`BackendEntry` is the internal registration record. Batch processing is a
backend capability, not a second object registered beside the backend. The
broker keeps the resolved `BackendId` to compute backend-specific policy
limits, group batches, and route later backend calls. Proof type, if needed, is
derived from the input `VerifierSelector`; it is not duplicated in
`BackendEntry`.

Future request evaluation can use minimal broker-provided limits:

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
  order

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
  batch_id
  existing_order_ids
  aggregation_state?
  new_proofs
  new_compressed_proofs
  finalize

BatchUpdate
  aggregation_state
  assessor_proof_id?
  set_builder_proving_secs?
  assessor_proving_secs?

CloseBatch
  batch_id
  aggregation_proof_id
  order_ids

BatchClose
  compressed_proof_id
  compression_secs

FulfillmentBatch
  backend_id
  aggregation_state?
  assessor_proof_id?
  eip712_domain
  prover_address
  orders

FulfillmentArtifacts
  root_submission?
  fulfillments
  assessor_receipt
```

This evaluation boundary is not fully implemented in the current broker MVP.
The current branch focuses on order processing, batch processing, and
submission artifact construction.

The broker computes `EvaluationLimits` from `BackendId`, requestor policy, and
market config. Priority-requestor bypass is represented by omitting the relevant
limit. The backend only enforces the limits it receives; it does not know why a
limit is present or absent.

Selector routing starts the chain. `BackendId` is a typed broker value, not a
raw string in service code. The broker persists the resolved `BackendId` on
accepted orders and carries it through later service commands. The router
dispatches by selector for order processing/cancellation; later batch and
submission calls dispatch through explicit router methods keyed by `BackendId`.

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

The backend interface separates batch operations into:

```text
----------------------+-----------------------------------------------+
| Batch size          | estimate_batch_size                           |
| Aggregation state   | update_batch                                  |
| Root compression    | close_batch                                   |
| Fulfillment build   | build_fulfillments                            |
+----------------------+-----------------------------------------------+
```

`update_batch` receives the current backend state plus newly claimed proofs for
the backend batch. The backend internally decides which orders update
aggregation state and which only participate in the assessor proof. The broker
does not classify selectors into "aggregation" and "assessor-only" paths.

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

The router is the broker-facing facade. It selects a `BackendEntry` from
selector or backend-id metadata and calls the registered backend implementation.
A backend instance may claim many selectors.

```text
                  +----------------+
Broker request -> +----------------+
                 | BackendRouter  |
                 +----------------+
                         |
                         v
                +------------------+
                | BackendEntry     |
                | - BackendId      |
                | - selectors      |
                | - Backend object |
                +------------------+
                         |
                         v
                    Backend impl
```

The router rejects duplicate backend IDs and duplicate selectors at
registration time.

## Request Evaluation And Proving

The broker owns policy. The backend owns zkVM execution semantics. The current
implementation keeps request evaluation/preflight mostly in existing broker/SDK
paths; a future SDK/requestor API pass can move more of that into a dedicated
evaluation boundary.

```text
Request observed
    |
    v
BackendRouter resolves request.selector
    |
    v
Broker computes backend-specific EvaluationLimits
    |
    v
Existing evaluation / pricing path
    |
    v
Broker pricing / capacity / allowlist policy
    |
    v
Order accepted
    |
    v
BackendRouter.process_order(ProcessOrder { order })
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
    - optional compressed proof handle
            |
            v
        Order enters batch lifecycle:
            PendingAgg or SkipAggregation
```

The broker no longer needs to know whether a selector is "Groth16",
"set-builder", or any other backend-specific proof mode. Once `process_order`
completes, the order is ready to enter broker-owned batching. Today the
database still has `PendingAgg` and `SkipAggregation` order states; those names
come from the RISC Zero set-builder path, but the backend-neutral meaning is:

- `PendingAgg`: proof output that should be sent through backend batch update.
- `SkipAggregation`: proof output that skips backend accumulation but still
  participates in assessor/submission batching.

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
BackendId("risc0_v3") -> Batch #10 -> orders [...]
BackendId("risc0_v4") -> Batch #11 -> orders [...]
BackendId("other")    -> Batch #12 -> orders [...]
```

## Candidate Claiming

The current DB still has two RISC Zero-shaped pending-proof queries, but they
are scoped by `BackendId`:

```text
get_aggregation_proofs(backend_id)
get_groth16_proofs(backend_id)
```

This split is a storage compatibility detail, not a broker policy decision. The
aggregator filters both sets for expired/already-fulfilled orders, then passes
both lists to the backend:

```text
BackendRouter.update_batch(backend_id, UpdateBatch {
  batch_id,
  existing_order_ids,
  aggregation_state?,
  new_proofs,
  new_compressed_proofs,
  finalize,
})
```

The backend decides internally how each selector participates:

```text
selector A -> update aggregation state, also assessed at finalization
selector B -> skip aggregation state, assessed at finalization
selector C -> fail inside backend if inconsistent with registration
```

This keeps selector-specific aggregation policy out of broker orchestration.

`update_batch` returns the new backend aggregation state and optionally an
assessor proof id when the broker asked the backend to finalize. The broker
persists that state and associates the successfully processed proof rows with
the broker batch.

## Batch Lifecycle

```text
OrderStatus:

PendingProving
    |
    v
Proving
    |
    v
PendingAgg / Aggregating / SkipAggregation
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
ReadyToSubmit
  |
  v
PendingSubmission
  |
  v
Submitted

Failed
```

`BatchStatus::Open` means an open broker batch. It does not imply that every
order in the batch needs set-builder aggregation. A backend may accept some
orders into aggregation state and route others directly to the
assessor/submission envelope. The common broker meaning is "this batch can
still receive backend updates or be finalized."

`BatchStatus::ReadyToSubmit` means backend artifacts are ready and the batch is
waiting to be claimed by the submitter.

`PendingCompression` means the backend has finalized/assessed the batch and the
final proof artifact must be compressed before submission.

Closing a batch is the explicit finalization call:

```text
BackendRouter.close_batch(backend_id, CloseBatch {
  batch_id,
  aggregation_proof_id,
  order_ids,
})
    |
    v
BatchClose
  compressed_proof_id
  compression_secs
```

The broker stores the returned compressed proof id on the batch and then moves
the batch toward submission. The assessor proof id is currently stored as a
batch field because the deployed contract path still needs it for RISC Zero
submission artifacts.

## Submission

The submitter gets backend-prepared artifacts instead of reconstructing
backend-specific seals itself. The current implementation still submits through
the existing market contract shape directly from `Submitter`; a dedicated market
submission adapter remains a later cleanup.

```text
FulfillmentBatch
  backend_id
  aggregation_state?
  assessor_proof_id?
  eip712_domain
  prover_address
  orders
```

```text
Batch complete
    |
    v
BackendRouter.build_fulfillments(FulfillmentBatch {
  backend_id,
  aggregation_state,
  assessor_proof_id,
  orders,
})
    |
    +--> optional root submission
    |       root
    |       root seal
    |
    +--> fulfillments keyed by order id
    |
    +--> legacy AssessorReceipt for current market contract
```

`build_fulfillments` receives completed orders and optional persisted backend
batch state. The backend fetches and decodes any backend-specific assessor
output, computes backend-specific claim digests or inclusion proofs, and returns
protocol-facing fulfillment artifacts. The broker submitter should not need a
separate per-order seal, claim digest, or assessor decoding callback.

`FulfillmentRequest` is the submission-time request payload. It is equivalent to
the current `UnlockedRequest` shape plus the broker `order_id`, and is distinct
from the order-stream `Order` inbound payload. The market submission adapter
computes request digests from this request payload when a contract path needs
them; backend implementations should not own market-domain digest construction.

Contract-version compatibility should eventually stay outside the backend:

```text
FulfillmentArtifacts
    |
    v
MarketSubmissionAdapter
    |
    +--> CurrentMarketAdapter        (future extraction)
    |       one legacy AssessorReceipt per submission
    |       uses legacy_assessor_selectors / legacy_assessor_callbacks
    |
    +--> RouterMarketAdapter         (future contract path)
            one or more router SubBatch values
            ignores legacy assessor metadata
```

The current-market adapter is a proposed migration bridge. Today the RISC Zero
backend returns artifacts compatible with the current `FulfillmentTx` path.

## Current Broker Mapping

The current broker services map to the proposed service surface as follows:

```text
Market monitor
  observes ProofRequest + signature
  creates OrderRequest
  no backend-specific work

Order pricer
  selector resolves to BackendId through accepted order path
  computes broker policy limits:
    max_cycles?
    max_download_bytes?
  existing preflight/pricing path
  applies broker policy:
    gas, collateral, allowlists, capacity, deadlines, lock strategy
  persists accepted Order with:
    backend_id
    image_id
    input_id?
    work_units / total_cycles
    fulfillment_data_bytes / journal_bytes

Order processor worker
  BackendRouter.process_order(ProcessOrder) -> OrderProcessProgress
  cancel_order(Order) on broker cancellation
  persists Running proof handles:
    Started.proof_id
    Compressed.compressed_proof_id
  persists Completed order artifacts:
    proof_id
    compressed_proof_id?
  moves order to PendingAgg or SkipAggregation using current DB states

Current implementation note: broker startup registers the single current
`Risc0Backend` behind `BackendRouter`. `OrderProcessor`, `AggregatorService`,
and `Submitter` hold `BackendRouter`. Backend trait objects are internal router
state.

Aggregator service
  groups batch-ready orders by BackendId
  BackendRouter.estimate_batch_size(backend_id, order_ids) -> BatchSizeEstimate
  BackendRouter.update_batch(backend_id, UpdateBatch) -> BatchUpdate
  BackendRouter.close_batch(backend_id, CloseBatch) -> BatchClose
  persists AggregationState and assessor_proof_id on Batch
  broker decides when the batch closes from:
    size, time, fees, deadlines, journal/fulfillment-data size

Submitter / batch closer
  BackendRouter.build_fulfillments(FulfillmentBatch) -> FulfillmentArtifacts
  submits:
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

Conversely, the broker interface intentionally does not own:

```text
zkVM receipt construction or verification formulas
selector-specific compression mechanics
claim digest formulas
set-builder / aggregation state semantics
assessor proof construction and journal decoding
seal encoding
```

## RISC Zero Adapter

The RISC Zero implementation is one reusable backend implementation configured
with selectors, prover handles, image IDs, and wrap selector.

```text
Risc0Backend
  - prover
  - snark_prover
  - downloader and priority requestor policy
  - set-builder program id
  - internal Risc0BatchProcessor delegate
  - implements Backend

Risc0BatchProcessor
  - set-builder aggregation
  - assessor proving
  - assessor output decoding
  - batch/root proof compression
```

Two RISC Zero versions should normally be two registrations of the same
implementation with different config behind a router:

```text
BackendEntry("risc0_v3", selectors_v3, Risc0Backend(config_v3))
BackendEntry("risc0_v4", selectors_v4, Risc0Backend(config_v4))
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
