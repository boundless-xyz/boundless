# Indexer API Endpoints Reference

All endpoints are `GET` only.

Two indexer types exist with non-overlapping endpoint sets:

- **Market Indexers** (L2 chains): market, requestor, prover, efficiency endpoints. Use `$MARKET_INDEXER_URL`.
- **ZKC Indexers** (Ethereum L1): staking, PoVW, delegation endpoints. Use `$ZKC_INDEXER_URL`.

**Note**: This reference may be out of date. The source of truth for response schemas and query parameters is the codebase:

- **Market response types**: `crates/boundless-market/src/indexer_types.rs`
- **Staking/PoVW/delegation/efficiency types**: `crates/lambdas/indexer-api/src/models.rs`
- **Route definitions**: `crates/lambdas/indexer-api/src/routes/`
- **OpenAPI spec**: `$INDEXER_URL/openapi.yaml` (live, always current)

If a field is missing from this reference, check those files or fetch the live OpenAPI spec.

## Table of Contents

**Market Indexer** (`$MARKET_INDEXER_URL`):

- [Market Endpoints](#market-endpoints)
- [Requestor Endpoints](#requestor-endpoints)
- [Prover Endpoints](#prover-endpoints)
- [Efficiency Endpoints](#efficiency-endpoints)

**ZKC Indexer** (`$ZKC_INDEXER_URL`):

- [Staking Endpoints](#staking-endpoints)
- [PoVW Endpoints](#povw-endpoints)
- [Delegation Endpoints](#delegation-endpoints)

**Both**:

- [Utility Endpoints](#utility-endpoints)

## Market Endpoints

> Market Indexer only (`$MARKET_INDEXER_URL`)

### GET /v1/market

Indexing status. Returns `chain_id`, `last_indexed_block`, `last_indexed_block_timestamp`, `last_indexed_block_timestamp_iso`.

### GET /v1/market/requests

Paginated list of all proof requests.

Query params:

- `limit`: max 500, default 50
- `cursor`: Base64 opaque cursor from previous response
- `sort_by`: `created_at` (default) or `updated_at`

Response: `{ chain_id, data: [RequestStatus...], next_cursor, has_more }`

Each request contains: `request_digest`, `request_id`, `request_status` (submitted/locked/fulfilled/slashed/expired), `source` (onchain/offchain), `client_address`, `lock_prover_address`, `fulfill_prover_address`, timestamps (`created_at`, `locked_at`, `fulfilled_at`, `slashed_at` -- both Unix and ISO), block numbers, pricing fields (`min_price`, `max_price`, `lock_price`, `lock_price_per_cycle`, `lock_collateral` -- all with `_formatted` variants), cost fields (`fixed_cost`, `variable_cost_per_cycle`), cycle counts (`program_cycles`, `total_cycles`), performance (`effective_prove_mhz`, `prover_effective_prove_mhz`), tx hashes, and request details (`image_id`, `image_url`, `selector`, `predicate_type`, `predicate_data`, `input_type`, `input_data`).

### GET /v1/market/requests/:request_id

All rows matching a request ID (can return multiple if the ID was reused). Same response shape as request list but filtered.

### GET /v1/market/aggregates

Time-series aggregate market stats.

Query params:

- `aggregation`: `hourly`, `daily`, `weekly`, `monthly` (default), `epoch`
- `limit`: max 500, default 50
- `cursor`: Base64 cursor
- `sort`: `asc` or `desc` (default)
- `before`: Unix timestamp upper bound
- `after`: Unix timestamp lower bound

Response includes per-period: `total_fulfilled`, `total_requests_submitted` (with onchain/offchain breakdown), `total_requests_locked`, `total_requests_slashed`, `total_expired`, `total_locked_and_expired`, `total_locked_and_fulfilled`, `total_secondary_fulfillments`, `locked_orders_fulfillment_rate`, `locked_orders_fulfillment_rate_adjusted`, lock price percentiles (p5/p10/p25/p50/p75/p90/p95/p99), fee totals, collateral totals, cycle totals, cost breakdowns, and `unique_provers_locking_requests` / `unique_requesters_submitting_requests`.

Top-level also includes: `total_collateral_deposited`, `eligible_prover_count`, `active_eligible_prover_count`.

### GET /v1/market/cumulatives

Running cumulative totals over time.

Query params: `cursor`, `limit` (max 500, default 50), `sort` (`asc`/`desc`), `before`, `after`.

Response has same fields as aggregates but values are cumulative.

## Requestor Endpoints

> Market Indexer only (`$MARKET_INDEXER_URL`)

### GET /v1/market/requestors

Requestor leaderboard sorted by orders requested.

Query params:

- `period`: `1h`, `1d`, `3d`, `7d`, `all` (default)
- `cursor`: Base64 cursor
- `limit`: max 100, default 50

Each entry: `requestor_address`, `orders_requested`, `orders_locked`, `orders_fulfilled`, `orders_expired`, `orders_not_locked_and_expired`, `cycles_requested`, `median_lock_price_per_cycle`, cost percentiles, `acceptance_rate`, `locked_order_fulfillment_rate`, `last_activity_time`.

### GET /v1/market/requestors/:address/requests

Requests submitted by a specific requestor. Same params as `/v1/market/requests`.

### GET /v1/market/requestors/:address/aggregates

Per-requestor time-series aggregates. Same params as market aggregates except `monthly` aggregation is not supported.

### GET /v1/market/requestors/:address/cumulatives

Per-requestor cumulative totals. Same params as market cumulatives.

## Prover Endpoints

> Market Indexer only (`$MARKET_INDEXER_URL`)

### GET /v1/market/provers

Prover leaderboard sorted by fees earned.

Query params: same as requestor leaderboard (`period`, `cursor`, `limit`).

Each entry: `prover_address`, `orders_locked`, `orders_fulfilled`, `cycles`, `fees_earned`, `collateral_earned`, `collateral_slashed`, `median_lock_price_per_cycle`, `best_effective_prove_mhz`, `locked_order_fulfillment_rate`, `collateral_deposited_zkc`, `collateral_available_zkc`, `last_activity_time`.

### GET /v1/market/provers/:address/requests

Requests fulfilled by a specific prover. Same params as `/v1/market/requests`.

### GET /v1/market/provers/:address/aggregates

Per-prover time-series aggregates. Same params as market aggregates except `monthly` aggregation is not supported for provers.

### GET /v1/market/provers/:address/cumulatives

Per-prover cumulative totals. Same params as market cumulatives.

## Staking Endpoints

> ZKC Indexer only (`$ZKC_INDEXER_URL`)

All staking endpoints use offset pagination: `limit` (default 50, max 100), `offset` (default 0).

### GET /v1/staking

Global staking summary: `current_total_staked`, `total_unique_stakers`, `current_active_stakers`, `current_withdrawing`, `total_staking_emissions_all_time`.

### GET /v1/staking/epochs

Paginated list of epoch staking summaries: `total_staked`, `num_stakers`, `num_withdrawing`, `total_staking_emissions`, `total_staking_power`, `num_reward_recipients`, `epoch_start_time`, `epoch_end_time`.

### GET /v1/staking/epochs/:epoch

Single epoch staking summary.

### GET /v1/staking/epochs/:epoch/addresses

Staking leaderboard for a specific epoch. Each entry: `staker_address`, `staked_amount`, `is_withdrawing`, `rewards_delegated_to`, `votes_delegated_to`, `rewards_generated`.

### GET /v1/staking/epochs/:epoch/addresses/:address

Staking data for one address at a specific epoch.

### GET /v1/staking/addresses

All-time staking leaderboard. Each entry: `staker_address`, `total_staked`, `is_withdrawing`, delegation info, `epochs_participated`, `total_rewards_generated`.

### GET /v1/staking/addresses/:address

Staking history and summary for a specific address. Returns epoch entries plus an aggregate summary.

## PoVW Endpoints

> ZKC Indexer only (`$ZKC_INDEXER_URL`)

All PoVW endpoints use offset pagination: `limit` (default 50, max 100), `offset` (default 0).

### GET /v1/povw

Global PoVW summary: `total_epochs_with_work`, `total_unique_work_log_ids`, `total_work_all_time`, `total_emissions_all_time`, `total_capped_rewards_all_time`, `total_uncapped_rewards_all_time`.

### GET /v1/povw/epochs

Paginated epoch PoVW summaries: `total_work`, `total_emissions`, `total_capped_rewards`, `total_uncapped_rewards`, `num_participants`, time range.

### GET /v1/povw/epochs/:epoch

Single epoch PoVW summary.

### GET /v1/povw/epochs/:epoch/addresses

PoVW leaderboard for a specific epoch. Each entry: `work_log_id`, `work_submitted`, `percentage`, `uncapped_rewards`, `reward_cap`, `actual_rewards`, `is_capped`, `staked_amount`.

### GET /v1/povw/epochs/:epoch/addresses/:address

PoVW data for one address at a specific epoch.

### GET /v1/povw/addresses

All-time PoVW leaderboard. Each entry: `work_log_id`, `total_work_submitted`, `total_actual_rewards`, `total_uncapped_rewards`, `epochs_participated`.

### GET /v1/povw/addresses/:address

PoVW history and summary for a specific address.

## Delegation Endpoints

> ZKC Indexer only (`$ZKC_INDEXER_URL`)

All delegation endpoints use offset pagination: `limit` (default 50, max 100), `offset` (default 0).

Two delegation types: **votes** and **rewards**, with parallel endpoint structures.

### Vote Delegations

- `GET /v1/delegations/votes/addresses` - Aggregate vote delegation leaderboard
- `GET /v1/delegations/votes/epochs/:epoch/addresses` - Vote delegations by epoch
- `GET /v1/delegations/votes/epochs/:epoch/addresses/:address` - Vote delegation for specific address+epoch
- `GET /v1/delegations/votes/addresses/:address` - Vote delegation history for an address

### Reward Delegations

- `GET /v1/delegations/rewards/addresses` - Aggregate reward delegation leaderboard
- `GET /v1/delegations/rewards/epochs/:epoch/addresses` - Reward delegations by epoch
- `GET /v1/delegations/rewards/epochs/:epoch/addresses/:address` - Reward delegation for specific address+epoch
- `GET /v1/delegations/rewards/addresses/:address` - Reward delegation history for an address

Each delegation entry: `delegate_address`, `power`, `delegator_count`, `delegators` (address list), `epochs_participated` (aggregates only), `epoch` (epoch-specific only).

## Efficiency Endpoints

> Market Indexer only (`$MARKET_INDEXER_URL`)

### GET /v1/market/efficiency

Summary efficiency stats.

Query params:

- `type`: `raw` (default), `gas_adjusted`, `gas_adjusted_with_exclusions`

Returns: `latest_hourly_efficiency_rate`, `latest_daily_efficiency_rate`, `total_requests_analyzed`, `most_profitable_locked`, `not_most_profitable_locked`.

### GET /v1/market/efficiency/aggregates

Time-series efficiency aggregates.

Query params:

- `granularity`: `hourly` (default) or `daily`
- `before`, `after`: Unix timestamps
- `limit`: max 500, default 50
- `cursor`: Base64 cursor
- `sort`: `asc` or `desc` (default)
- `type`: same as summary

Each entry: `period_timestamp`, `period_timestamp_iso`, `num_most_profitable_locked`, `num_not_most_profitable_locked`, `efficiency_rate`.

### GET /v1/market/efficiency/requests

Per-request efficiency data.

Query params:

- `before`, `after`: Unix timestamps
- `limit`: max 500, default 50
- `cursor`: Base64 cursor
- `sort`: `asc` or `desc` (default)
- `efficiency_type`: `raw`, `gas_adjusted`, `gas_adjusted_with_exclusions`

Each entry: `request_digest`, `request_id`, `locked_at`, `lock_price`, `program_cycles`, `lock_price_per_cycle`, `is_most_profitable`, `num_requests_more_profitable`, `num_requests_less_profitable`, `more_profitable_sample` (up to 5 entries).

### GET /v1/market/efficiency/requests/:request_id

Single request efficiency record. Returns 404 if not found.

## Utility Endpoints

> Available on both indexer types

### GET /health

Returns `{ status, service }`.

### GET /docs

Swagger UI (interactive API documentation).

### GET /openapi.yaml

OpenAPI specification in YAML format.

### GET /openapi.json

OpenAPI specification in JSON format (used by Swagger UI).
