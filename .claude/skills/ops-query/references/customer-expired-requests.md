# Customer Expired Requests

Investigate why a customer's proof requests are expiring. Takes a customer requestor address, correlates indexer data with broker telemetry and prover logs to determine why public provers are skipping or failing to fulfill these orders.

## Presentation: Investigation Summary

After collecting all data, lead with a concise **Investigation Summary** covering:

**1. Customer Profile**: How active is this requestor? Total requests submitted, fulfilled, expired, fulfillment rate. Is this a known customer (check address labels)? How does their fulfillment rate compare to the market average?

**2. Expiration Patterns**: Are expirations concentrated in a specific time window, or spread out? Do expired requests share common traits (same image ID, similar cycle counts, similar pricing)? Are expirations recent or historical?

**3. Why Provers Aren't Fulfilling**: From telemetry, what are brokers doing with this customer's orders? Are they skipping (pricing, capacity, input issues), dropping (race losses, insufficient balance, tight deadlines), or locking and then failing (proving errors, estimation mismatches, deadline misses)? Which provers are seeing these orders and what decisions are they making?

**4. Secondary Fulfillment**: For orders that were locked but not fulfilled by the locking prover, did any prover attempt secondary fulfillment? Secondary fulfillment is when a prover fulfills an order after the original locking prover's lock has expired, earning the slash collateral as reward. Check whether our BP provers attempted secondary fulfillment on these orders -- did they see the opportunity, did they skip it, did they fail?

**5. Our Provers Specifically**: For provers we operate (BP-prefixed), detailed breakdown of their interaction with this customer's orders -- skips, drops, failures, and specific reasons from both telemetry and logs. Include both primary and secondary fulfillment attempts.

**6. Root Cause & Recommendations**: Synthesize findings into actionable insights. Is the customer underpricing? Are their programs too large? Is there an input availability issue? Are provers hitting bugs on their specific workload?

After the summary, include the detailed data tables for reference.

## Step 1: Customer overview from the indexer

Use the ops-indexer-query skill to gather:

- **Requestor leaderboard** (`/v1/market/requestors?period=7d`): Find the customer's entry and pull their `orders_requested`, `orders_locked`, `orders_fulfilled`, `orders_expired`, `orders_not_locked_and_expired`, `acceptance_rate`, `locked_order_fulfillment_rate`, `median_lock_price_per_cycle`.
- **Market aggregates** (`/v1/market/aggregates?aggregation=daily&limit=7`): Get the market's `locked_orders_fulfillment_rate` for comparison.

Key things to note:

- **`orders_not_locked_and_expired`**: Orders that expired without any prover even attempting to lock them. High counts here suggest pricing or compatibility issues.
- **`acceptance_rate`**: Fraction of submitted orders that got locked. Low acceptance means provers are not interested.
- **`locked_order_fulfillment_rate`**: Of orders that were locked, how many were fulfilled. Low rate here means provers are locking but failing.

## Step 2: Expiration trends and patterns from the indexer

Use the ops-indexer-query skill to gather:

- **Per-requestor aggregates** (`/v1/market/requestors/:address/aggregates`): Hourly (last 48h) and/or daily (last 14d) to find when expirations cluster. Look for spikes in `total_expired` / `total_locked_and_expired`, periods with zero locks, or sudden fulfillment rate changes.
- **Requestor's requests** (`/v1/market/requestors/:address/requests`): Filter for `request_status == "expired"` or `"slashed"`. Collect the expired request IDs for telemetry correlation.

Summarize patterns across expired requests:

- Are all expired requests using the same `image_id`? Provers may not support that guest program.
- What are the `program_cycles` / `total_cycles`? Very large cycle counts may exceed prover capacity.
- What is the pricing (`min_price`, `max_price`)? Compare against the market's median and percentile lock prices from the aggregates. Also compare pricing between the customer's fulfilled vs expired requests.
- Were any expired requests locked first (`lock_prover_address` is set)? If so, a prover tried but failed.

## Step 3: Telemetry analysis

Use the ops-telemetry-query skill to query the customer's orders across all three telemetry dimensions. Filter by `requestor_address` and a relevant time window (e.g. last 7 days). For provers we operate, also filter by their `broker_address` (from `network_address_labels.json`) to get a dedicated view.

### Evaluation summary

Query `telemetry.request_evaluations` grouped by `broker_address` to get per-prover counts of `Locked`, `Skipped`, `Committed`, and `Dropped` outcomes for this customer's orders.

### Skip breakdown

Query `telemetry.request_evaluations` where `outcome = 'Skipped'`, grouped by `broker_address` and `skip_code`. Common skip codes and what they mean for the customer:

- **`[S-008]`** (unprofitable): Customer is not offering enough price for the cycle count.
- **`[S-007]`** (order too large): Customer's programs exceed the prover's configured max cycle limit.
- **`[S-OP-009]`** (input fetch failure): The prover cannot download the customer's input data.
- **`[S-OP-002]`** (deny list): The customer's address is on a prover's deny list.
- **`[S-OP-004]`** / **`[S-OP-005]`** (already locked/fulfilled): Normal competitive behavior.
- **`[S-OP-008]`** (insufficient collateral): The prover doesn't have enough collateral to lock this order.

### Drop breakdown

Query `telemetry.request_evaluations` where `commitment_outcome = 'Dropped'`, grouped by `broker_address` and `commitment_skip_code`. Watch for:

- **`[B-OM-020]`** / **`[B-OM-023]`** (fulfilled/locked by another prover): Normal, but if this is the _only_ activity and orders still expire, it suggests a timing issue.
- **`[B-OM-010]`** (insufficient balance): Provers want to lock but can't afford the collateral.
- **`[B-OM-026]`** (can't complete by deadline): The customer's deadline is too tight for the cycle count.
- **`[B-OM-007]`** (lock tx failed): On-chain lock transaction failed.

### Completion outcomes

Query `telemetry.request_completions` grouped by `broker_address`, `outcome`, and `error_code`. Include average `total_duration_secs`, `actual_total_proving_time_secs`, and `estimated_proving_time_secs` to spot estimation mismatches.

If there are failures, also query individual failed completions to compare `estimated_proving_time_secs` vs `actual_total_proving_time_secs` (the ratio reveals estimation accuracy) and check `concurrent_proving_jobs_start` (high values suggest overload).

## Step 4: Secondary fulfillment analysis

For expired requests that were locked but not fulfilled (i.e. `lock_prover_address` is set but the request still expired), investigate whether secondary fulfillment was attempted. Secondary fulfillment is when another prover fulfills the order after the locking prover's lock expires, earning the slash collateral as a reward.

### From the indexer

Check the request data from Step 2. Requests where `lock_prover_address` is set but `fulfill_prover_address` is null (or differs from the lock prover) are candidates. Also check market aggregates for `total_secondary_fulfillments` to see if any secondary fulfillments occurred for this customer's orders.

### From telemetry

Query `telemetry.request_evaluations` for the expired request IDs, filtering for `fulfillment_type = 'FulfillAfterLockExpire'`. This shows which brokers evaluated these orders for secondary fulfillment and what they decided. Check:

- Did our BP provers see the secondary fulfillment opportunity?
- Did they skip it (and why -- check `skip_code`)?
- Did they attempt it but get dropped (check `commitment_skip_code`)?

Also query `telemetry.request_completions` for these request IDs with `fulfillment_type = 'FulfillAfterLockExpire'` to see if any prover attempted secondary fulfillment but failed.

### From logs

If our BP provers skipped or failed secondary fulfillment, search their CloudWatch logs for the relevant request IDs to get detailed error messages.

## Step 5: Our provers' logs

Use the ops-logs-query skill to search CloudWatch logs for provers we operate. Pick a few representative expired request IDs from Step 2 and search for them in the relevant bento prover log groups. This provides detailed error messages and stack traces that telemetry doesn't capture.

If the request ID doesn't appear in logs at all, the prover never even saw the order (likely skipped at the evaluation stage). Also try:

- Searching for the customer's requestor address directly.
- Searching for input fetch errors (`?"input" ?"fetch" ?"error"`) to see if our provers are failing to download this customer's inputs.

## Step 6: Synthesize root cause

Combine all findings into a root cause analysis. Common patterns:

| Pattern                                                       | Indicators                                                           | Likely Root Cause                                                |
| ------------------------------------------------------------- | -------------------------------------------------------------------- | ---------------------------------------------------------------- |
| All provers skip with `[S-008]`                               | Low max_price relative to cycle count and market rates               | Customer is underpricing their requests                          |
| All provers skip with `[S-007]`                               | Very high program_cycles / total_cycles                              | Customer's programs exceed prover capacity limits                |
| Provers skip with `[S-OP-009]`                                | Input fetch errors in logs, input_type is URL-based                  | Customer's input data is not accessible to provers               |
| Orders never locked (`orders_not_locked_and_expired` is high) | No evaluations in telemetry, or all skipped                          | No prover is willing or able to take these orders                |
| Orders locked but not fulfilled                               | `lock_prover_address` set, `locked_order_fulfillment_rate` low       | Provers are trying but failing -- check completion errors        |
| Failures with `ExpiredWhileProving`                           | `actual_total_proving_time_secs` >> `estimated_proving_time_secs`    | Proving takes longer than expected, deadline too tight           |
| Drops with `[B-OM-026]`                                       | Tight deadlines relative to cycle count                              | Customer's deadline doesn't give provers enough time             |
| Drops with `[B-OM-010]`                                       | Provers have insufficient collateral                                 | Market-wide collateral issue, not customer-specific              |
| All expired requests share one `image_id`                     | Unique image_id not seen in fulfilled orders                         | Provers may not support this guest program                       |
| Expirations only during specific hours                        | Clustered timestamps in aggregates                                   | Prover capacity issue during peak times                          |
| Locked but no secondary fulfillment attempted                 | No `FulfillAfterLockExpire` evaluations in telemetry for the request | Provers not configured for secondary fulfillment, or skipping it |
