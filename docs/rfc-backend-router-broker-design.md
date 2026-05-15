# RFC: Broker Backend Router

## Summary

This branch introduces a broker-side backend boundary for heterogeneous zkVM
support.

The broker owns orchestration: order lifecycle, DB state, retry/cancel,
capacity, pricing policy, batch timing, telemetry, and transaction submission.

Backends own proof semantics: supported selectors, per-order proof processing,
optional aggregation, assessor behavior, compression, claim-digest/seal
construction, and fulfillment artifacts.

Request evaluation is a smaller shared surface. It executes/preflights a request
and returns facts; broker pricing still decides whether to accept, skip, lock,
or defer an order.

The broker talks to `BackendRouter`, not directly to backend trait objects.
`BackendRouter` owns registrations and routes by selector or `BackendId`.

```text
                 broker-owned workers
       +--------------------------------------+
       |                                      |
       |  OrderProcessor   Aggregator        |
       |        \             |              |
       |         \            |              |
       |          +------ BackendRouter -----+----+
       |         /            |              |    |
       |  Submitter           |              |    |
       |                                      |    |
       +--------------------------------------+    |
                                                   v
                                        backend-owned implementation
                                      +--------------------------+
                                      | Backend                  |
                                      | - process orders         |
                                      | - update batch state     |
                                      | - close batches          |
                                      | - build fulfillments     |
                                      +--------------------------+
```

## Non-Goals

- Redesign the external requestor SDK API.
- Make the standard request builder fully zkVM-neutral.
- Change the current market contract path.
- Solve Cargo isolation for incompatible native zkVM SDK versions.

## Core Types

```text
BackendId          stable broker identity, e.g. "risc0_v3"
ProgramId          opaque 32-byte program identity
ProofId            durable backend proof handle
CompressedProofId  durable compressed proof handle
AssessorProofId    durable assessor proof handle
ClaimDigest        opaque 32-byte claim digest
BackendBatchState  opaque backend-defined persisted batch state
```

`BackendBatchState` is broker-persisted but backend-defined:

```text
BackendBatchState
  data                  backend-defined persisted JSON
  proof_id?             proof to compress for root/assessor seal
  compressed_proof_id?  compressed proof used for submission
  selector?             selector used for compressed proof/seal
```

RISC Zero stores guest state and claim digests inside `data`; the generic broker
boundary does not expose `Receipt`, `GuestState`, `Digest`, journals, or
set-builder types.

## Interfaces

Request evaluation:

```text
RequestEvaluator
  evaluate_request(EvaluationRequest, exec_limit_cycles, cache_key) -> RequestEvaluation
  evaluation_output(evaluation_id) -> bytes
  invalidate_evaluation(cache_key) -> ()

EvaluationRequest
  request_id
  program_url
  predicate
  input_type/input_data
  client_address

RequestEvaluation
  Success { evaluation_id, cycle_count, program_id, input_id }
  Skip { cached_limit }
```

Today the only implementation is RISC0-backed. `OrderPricingContext` consumes
`RequestEvaluator`; profitability, collateral, deadline, capacity, and
allow/deny-list policy stay outside the evaluator. This keeps a small trait that
can later be shared with the requestor SDK without making the broker backend
router responsible for pricing.

Single backend implementation:

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

`BackendEntry` contains only:

```text
BackendEntry
  id
  selectors
  backend
```

The router rejects duplicate backend IDs and duplicate selectors at
registration time.

Registration and routing:

```text
              +-------------------------------+
              | BackendRouter                 |
              |                               |
selector ---> | routes: selector -> BackendId | ---> BackendEntry
BackendId --> | backends: BackendId -> Entry  |        |
              +-------------------------------+        v
                                               +----------------+
                                               | Backend impl   |
                                               +----------------+
```

## Order Processing

```text
         +------------------+
Order -> | BackendRouter    |
         +------------------+
                  |
                  v
         +------------------+
         | Backend          |
         +------------------+
           |       |       |
           |       |       +--> Completed(ProcessedOrder)
           |       |
           |       +----------> Compressed { proof_id, compressed_proof_id }
           |
           +------------------> Started { proof_id }
```

The broker persists durable proof handles between calls. The backend decides
whether an order needs compression and which next broker order state to use:

```text
ProcessedOrder
  backend_id
  order_id
  proof_id
  compressed_proof_id?
  next_status
```

The broker does not classify selectors as Groth16, set-builder, or assessor
only. It just routes through the backend and persists the returned state.

## Broker-Owned Batching

Batching is a broker lifecycle concern. Aggregation is an optional backend
operation.

```text
             broker-owned lifecycle
 +---------------------------------------------+
 | claim batch-ready orders                    |
 | group by BackendId                          |
 | decide when each batch closes               |
 | persist batch/order status                  |
 | submit transactions                         |
 +----------------------+----------------------+
                        |
                        v
             backend-owned batch semantics
 +---------------------------------------------+
 | accept new BatchOrder values                |
 | update opaque BackendBatchState             |
 | decide which selectors aggregate            |
 | run assessor/final proof work               |
 | produce fulfillment artifacts               |
 +---------------------------------------------+
```

Current DB queries are still RISC Zero-shaped:

```text
DB:
  get_aggregation_proofs(backend_id)  \
                                      +--> Vec<BatchOrder> --> update_batch
  get_groth16_proofs(backend_id)      /
```

That split is a storage compatibility detail. The aggregator converts both into
neutral `BatchOrder` values and calls:

```text
UpdateBatch
  batch_id
  existing_order_ids
  state?
  new_orders
  finalize

BatchOrder
  order_id
  proof_id
  compressed_proof_id?
  expiration
  fee
  fulfillment_type
  request_id
  lock_expiration

BatchUpdate
  state
  assessor_proof_id?
  set_builder_proving_secs?
  assessor_proving_secs?
```

When finalization is needed:

```text
BackendRouter.close_batch(backend_id, CloseBatch)
       |
       v
BatchClose
  compressed_proof_id
  compression_secs
```

## Submission

Submitter asks the backend to build submission artifacts:

```text
Submitter
   |
   v
BackendRouter.build_fulfillments(FulfillmentBatch)
   |
   v
FulfillmentArtifacts
   |-- root_submission?
   |-- fulfillments
   `-- assessor
```

`assessor` is backend-neutral at the broker boundary:

```text
SubmissionAssessorArtifact
  seal
  selectors
  callbacks
```

The current `Submitter` still adapts this into the current market
`AssessorReceipt`. A dedicated market submission adapter is a later cleanup.

## Batch Statuses

```text
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

Failed is terminal from any state.
```

`Open` means the broker batch can still receive backend updates or be finalized.
It does not imply every order needs set-builder aggregation.

`ReadyToSubmit` means backend artifacts are ready and the submitter can claim
the batch.

## Current RISC Zero Adapter

`Risc0Backend` is the first backend implementation. It is configured with
prover handles, downloader/priority requestor policy, set-builder program ID,
and an internal `Risc0BatchProcessor`.

```text
BackendRouter
  |
  +-- BackendEntry("risc0_v3", selectors_v3, Risc0Backend(config_v3))
  |
  `-- BackendEntry("risc0_v4", selectors_v4, Risc0Backend(config_v4))
```

Two RISC Zero versions can share the same implementation if their native crates
can coexist in one broker binary. If not, that becomes an adapter/Cargo
packaging problem, not a broker routing problem.

## Remaining Work

- Rename old DB/query concepts like `get_aggregation_proofs` and
  `get_groth16_proofs` to neutral batch-ready terminology.
- Extract current-market submission adaptation out of `Submitter`.
- Move request evaluation to its final SDK module once the requestor API design
  is settled.
- Decide where third-party backend crates should live once the trait surface is
  stable enough for out-of-tree implementations.
