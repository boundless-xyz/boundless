# Boundless Indexer API

REST API for querying Boundless protocol data including PoVW rewards leaderboards and staking positions.

## Base URL

```
https://api.boundless.market/v1
```

## Endpoints Summary

### PoVW Endpoints
- `GET /v1/povw` - Get aggregate PoVW rewards leaderboard across all epochs
- `GET /v1/povw/epochs/:epoch` - Get PoVW rewards leaderboard for a specific epoch
- `GET /v1/povw/addresses/:address` - Get PoVW rewards history for a specific address
- `GET /v1/povw/addresses/:address/epochs/:epoch` - Get PoVW rewards for an address at a specific epoch

### Staking Endpoints
- `GET /v1/staking` - Get aggregate staking positions leaderboard across all epochs
- `GET /v1/staking/epochs/:epoch` - Get staking positions for a specific epoch
- `GET /v1/staking/addresses/:address` - Get staking history for a specific address
- `GET /v1/staking/addresses/:address/epochs/:epoch` - Get staking position for an address at a specific epoch

## Common Request/Response Structures

### Pagination Parameters
All list endpoints support pagination using query parameters:
- `offset` (integer, default: 0) - Number of items to skip
- `limit` (integer, default: 100, max: 1000) - Number of items to return

### Common Headers
- `Content-Type: application/json`
- `Accept: application/json`

### Common Response Fields
- `entries` - Array of items for the current page
- `pagination` - Pagination metadata object
  - `offset` - Current offset
  - `limit` - Current limit
  - `total` - Total number of items (if available)
- `summary` - Optional summary statistics object

## Detailed Endpoint Documentation

### PoVW Endpoints

#### 1. Get Aggregate PoVW Leaderboard

**Request:**
```
GET /v1/povw?offset=0&limit=100
```

**Query Parameters:**
- `offset` (integer, optional) - Start position for pagination
- `limit` (integer, optional) - Number of results to return

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "total_work_submitted": "1234567890123456789",
      "total_work_submitted_formatted": "1,234.57 MCycles",
      "total_actual_rewards": "9876543210987654321",
      "total_actual_rewards_formatted": "9.88 ZKC",
      "total_uncapped_rewards": "9876543210987654321",
      "total_uncapped_rewards_formatted": "9.88 ZKC",
      "epochs_participated": 5
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "total": 500
  },
  "summary": {
    "total_epochs_with_work": 10,
    "total_unique_work_log_ids": 150,
    "total_work_all_time": "98765432109876543210",
    "total_work_all_time_formatted": "98,765.43 MCycles",
    "total_emissions_all_time": "50000000000000000000000",
    "total_emissions_all_time_formatted": "50,000.00 ZKC",
    "total_capped_rewards_all_time": "45000000000000000000000",
    "total_capped_rewards_all_time_formatted": "45,000.00 ZKC",
    "total_uncapped_rewards_all_time": "50000000000000000000000",
    "total_uncapped_rewards_all_time_formatted": "50,000.00 ZKC",
    "last_updated_at": "2025-09-26T16:08:04.121573+00:00"
  }
}
```

**Entry Fields:**
- `rank` - Position in the leaderboard
- `work_log_id` - Ethereum address of the work log
- `total_work_submitted` - Raw amount of computational work (in cycles)
- `total_work_submitted_formatted` - Human-readable work amount
- `total_actual_rewards` - Actual rewards earned after caps (wei)
- `total_actual_rewards_formatted` - Human-readable actual rewards
- `total_uncapped_rewards` - Rewards before applying caps (wei)
- `total_uncapped_rewards_formatted` - Human-readable uncapped rewards
- `epochs_participated` - Number of epochs this address participated in

**Summary Fields:**
- `total_epochs_with_work` - Total epochs where work was submitted
- `total_unique_work_log_ids` - Number of unique participants
- `total_work_all_time` - Total computational work across all epochs
- `total_emissions_all_time` - Total ZKC tokens emitted
- `total_capped_rewards_all_time` - Total rewards after applying caps
- `total_uncapped_rewards_all_time` - Total rewards before caps
- `last_updated_at` - Timestamp of last data update

#### 2. Get PoVW by Epoch

**Request:**
```
GET /v1/povw/epochs/:epoch?offset=0&limit=100
```

**Path Parameters:**
- `epoch` (integer, required) - Epoch number

**Query Parameters:**
- `offset` (integer, optional) - Start position for pagination
- `limit` (integer, optional) - Number of results to return

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "work_submitted": "1234567890123456789",
      "work_submitted_formatted": "1,234.57 MCycles",
      "percentage": 25.5,
      "uncapped_rewards": "9876543210987654321",
      "uncapped_rewards_formatted": "9.88 ZKC",
      "reward_cap": "10000000000000000000",
      "reward_cap_formatted": "10.00 ZKC",
      "actual_rewards": "9876543210987654321",
      "actual_rewards_formatted": "9.88 ZKC",
      "is_capped": false,
      "staked_amount": "5000000000000000000",
      "staked_amount_formatted": "5.00 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "total": 25
  },
  "summary": {
    "epoch": 5,
    "total_work": "4938271560493827156",
    "total_work_formatted": "4,938.27 MCycles",
    "total_emissions": "5000000000000000000000",
    "total_emissions_formatted": "5,000.00 ZKC",
    "total_capped_rewards": "4500000000000000000000",
    "total_capped_rewards_formatted": "4,500.00 ZKC",
    "total_uncapped_rewards": "5000000000000000000000",
    "total_uncapped_rewards_formatted": "5,000.00 ZKC",
    "epoch_start_time": "2024-01-01T00:00:00Z",
    "epoch_end_time": "2024-01-08T00:00:00Z",
    "num_participants": 25,
    "last_updated_at": "2025-09-26T16:08:04.121573+00:00"
  }
}
```

**Entry Fields:**
- `rank` - Position in the epoch leaderboard
- `work_log_id` - Ethereum address of the work log
- `epoch` - Epoch number
- `work_submitted` - Computational work submitted this epoch
- `percentage` - Percentage of total epoch work
- `uncapped_rewards` - Rewards before applying staking cap
- `reward_cap` - Maximum rewards based on staking
- `actual_rewards` - Final rewards after applying cap
- `is_capped` - Whether rewards were limited by cap
- `staked_amount` - Amount of ZKC staked

**Summary Fields:**
- `epoch` - Epoch number
- `total_work` - Total work submitted in epoch
- `total_emissions` - Total ZKC emitted for epoch
- `total_capped_rewards` - Sum of actual (capped) rewards
- `total_uncapped_rewards` - Sum of rewards before caps
- `epoch_start_time` - Epoch start timestamp
- `epoch_end_time` - Epoch end timestamp
- `num_participants` - Number of unique participants
- `last_updated_at` - Timestamp of last data update

#### 3. Get PoVW History by Address

**Request:**
```
GET /v1/povw/addresses/:address?offset=0&limit=100
```

**Path Parameters:**
- `address` (string, required) - Ethereum address (0x-prefixed)

**Query Parameters:**
- `offset` (integer, optional) - Start position for pagination
- `limit` (integer, optional) - Number of results to return

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "work_submitted": "1234567890123456789",
      "work_submitted_formatted": "1,234.57 MCycles",
      "percentage": 25.5,
      "uncapped_rewards": "9876543210987654321",
      "uncapped_rewards_formatted": "9.88 ZKC",
      "reward_cap": "10000000000000000000",
      "reward_cap_formatted": "10.00 ZKC",
      "actual_rewards": "9876543210987654321",
      "actual_rewards_formatted": "9.88 ZKC",
      "is_capped": false,
      "staked_amount": "5000000000000000000",
      "staked_amount_formatted": "5.00 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "total": 10
  },
  "summary": {
    "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
    "total_work_submitted": "1234567890123456789",
    "total_work_submitted_formatted": "1,234.57 MCycles",
    "total_actual_rewards": "9876543210987654321",
    "total_actual_rewards_formatted": "9.88 ZKC",
    "total_uncapped_rewards": "9876543210987654321",
    "total_uncapped_rewards_formatted": "9.88 ZKC",
    "epochs_participated": 5
  }
}
```

**Entry Fields:**
Same as epoch endpoint, showing each epoch's performance

**Summary Fields:**
- `work_log_id` - The queried address
- `total_work_submitted` - Total work across all epochs
- `total_actual_rewards` - Total rewards earned
- `total_uncapped_rewards` - Total rewards before caps
- `epochs_participated` - Number of epochs participated

#### 4. Get PoVW by Address and Epoch

**Request:**
```
GET /v1/povw/addresses/:address/epochs/:epoch
```

**Path Parameters:**
- `address` (string, required) - Ethereum address (0x-prefixed)
- `epoch` (integer, required) - Epoch number

**Response Structure:**
```json
{
  "rank": 0,
  "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
  "epoch": 5,
  "work_submitted": "1234567890123456789",
  "work_submitted_formatted": "1,234.57 MCycles",
  "percentage": 25.5,
  "uncapped_rewards": "9876543210987654321",
  "uncapped_rewards_formatted": "9.88 ZKC",
  "reward_cap": "10000000000000000000",
  "reward_cap_formatted": "10.00 ZKC",
  "actual_rewards": "9876543210987654321",
  "actual_rewards_formatted": "9.88 ZKC",
  "is_capped": false,
  "staked_amount": "5000000000000000000",
  "staked_amount_formatted": "5.00 ZKC"
}
```

**Response Fields:**
Same as epoch leaderboard entry, but for a single address/epoch combination. Returns `null` if no data exists for the specified combination.

### Staking Endpoints

#### 1. Get Aggregate Staking Leaderboard

**Request:**
```
GET /v1/staking?offset=0&limit=100
```

**Query Parameters:**
- `offset` (integer, optional) - Start position for pagination
- `limit` (integer, optional) - Number of results to return

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "staker_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "total_staked": "10000000000000000000000",
      "total_staked_formatted": "10,000.00 ZKC",
      "is_withdrawing": false,
      "rewards_delegated_to": "0x1234567890123456789012345678901234567890",
      "votes_delegated_to": "0x1234567890123456789012345678901234567890",
      "epochs_participated": 5,
      "total_rewards_earned": "1000000000000000000",
      "total_rewards_earned_formatted": "1.00 ZKC",
      "total_rewards_generated": "5000000000000000000",
      "total_rewards_generated_formatted": "5.00 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "total": 500
  },
  "summary": {
    "current_total_staked": "1000000000000000000000000",
    "current_total_staked_formatted": "1,000,000.00 ZKC",
    "total_unique_stakers": 500,
    "current_active_stakers": 450,
    "current_withdrawing": 50,
    "total_staking_emissions_all_time": "50000000000000000000000",
    "total_staking_emissions_all_time_formatted": "50,000.00 ZKC",
    "last_updated_at": "2025-09-26T16:08:04.121573+00:00"
  }
}
```

**Entry Fields:**
- `rank` - Position in the leaderboard
- `staker_address` - Ethereum address of the staker
- `total_staked` - Total amount staked (wei)
- `total_staked_formatted` - Human-readable staked amount
- `is_withdrawing` - Whether currently withdrawing
- `rewards_delegated_to` - Address receiving staking rewards
- `votes_delegated_to` - Address receiving voting power
- `epochs_participated` - Number of epochs staked
- `total_rewards_earned` - Total staking rewards earned
- `total_rewards_generated` - Total rewards generated for delegatees

**Summary Fields:**
- `current_total_staked` - Current total staked across all users
- `total_unique_stakers` - Total number of unique stakers ever
- `current_active_stakers` - Currently active stakers
- `current_withdrawing` - Stakers currently withdrawing
- `total_staking_emissions_all_time` - Total staking emissions
- `last_updated_at` - Timestamp of last data update

#### 2. Get Staking by Epoch

**Request:**
```
GET /v1/staking/epochs/:epoch?offset=0&limit=100
```

**Path Parameters:**
- `epoch` (integer, required) - Epoch number

**Query Parameters:**
- `offset` (integer, optional) - Start position for pagination
- `limit` (integer, optional) - Number of results to return

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "staker_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "staked_amount": "10000000000000000000000",
      "staked_amount_formatted": "10,000.00 ZKC",
      "is_withdrawing": false,
      "rewards_delegated_to": "0x1234567890123456789012345678901234567890",
      "votes_delegated_to": "0x1234567890123456789012345678901234567890",
      "rewards_generated": "500000000000000000",
      "rewards_generated_formatted": "0.50 ZKC",
      "rewards_earned": "100000000000000000",
      "rewards_earned_formatted": "0.10 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "total": 250
  },
  "summary": {
    "epoch": 5,
    "total_staked": "500000000000000000000000",
    "total_staked_formatted": "500,000.00 ZKC",
    "num_stakers": 250,
    "num_withdrawing": 25,
    "total_staking_emissions": "5000000000000000000000",
    "total_staking_emissions_formatted": "5,000.00 ZKC",
    "total_staking_power": "450000000000000000000000",
    "total_staking_power_formatted": "450,000.00 ZKC",
    "num_reward_recipients": 200,
    "epoch_start_time": "2024-01-01T00:00:00Z",
    "epoch_end_time": "2024-01-08T00:00:00Z",
    "last_updated_at": "2025-09-26T16:08:04.121573+00:00"
  }
}
```

**Entry Fields:**
- `rank` - Position in the epoch leaderboard
- `staker_address` - Ethereum address of the staker
- `epoch` - Epoch number
- `staked_amount` - Amount staked in this epoch
- `is_withdrawing` - Withdrawal status during epoch
- `rewards_delegated_to` - Rewards delegation address
- `votes_delegated_to` - Voting delegation address
- `rewards_generated` - Rewards generated for delegatees
- `rewards_earned` - Staking rewards earned

**Summary Fields:**
- `epoch` - Epoch number
- `total_staked` - Total amount staked in epoch
- `num_stakers` - Number of stakers
- `num_withdrawing` - Number withdrawing
- `total_staking_emissions` - Total staking rewards emitted
- `total_staking_power` - Total effective staking power
- `num_reward_recipients` - Number of reward recipients
- `epoch_start_time` - Epoch start timestamp
- `epoch_end_time` - Epoch end timestamp
- `last_updated_at` - Timestamp of last data update

#### 3. Get Staking History by Address

**Request:**
```
GET /v1/staking/addresses/:address?offset=0&limit=100
```

**Path Parameters:**
- `address` (string, required) - Ethereum address (0x-prefixed)

**Query Parameters:**
- `offset` (integer, optional) - Start position for pagination
- `limit` (integer, optional) - Number of results to return

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "staker_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "staked_amount": "10000000000000000000000",
      "staked_amount_formatted": "10,000.00 ZKC",
      "is_withdrawing": false,
      "rewards_delegated_to": "0x1234567890123456789012345678901234567890",
      "votes_delegated_to": "0x1234567890123456789012345678901234567890",
      "rewards_generated": "500000000000000000",
      "rewards_generated_formatted": "0.50 ZKC",
      "rewards_earned": "100000000000000000",
      "rewards_earned_formatted": "0.10 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "total": 10
  },
  "summary": {
    "staker_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
    "total_staked": "10000000000000000000000",
    "total_staked_formatted": "10,000.00 ZKC",
    "is_withdrawing": false,
    "rewards_delegated_to": "0x1234567890123456789012345678901234567890",
    "votes_delegated_to": "0x1234567890123456789012345678901234567890",
    "epochs_participated": 5,
    "total_rewards_earned": "1000000000000000000",
    "total_rewards_earned_formatted": "1.00 ZKC",
    "total_rewards_generated": "5000000000000000000",
    "total_rewards_generated_formatted": "5.00 ZKC"
  }
}
```

**Entry Fields:**
Same as epoch endpoint, showing each epoch's staking position

**Summary Fields:**
- `staker_address` - The queried address
- `total_staked` - Current total staked amount
- `is_withdrawing` - Current withdrawal status
- `rewards_delegated_to` - Current rewards delegation
- `votes_delegated_to` - Current voting delegation
- `epochs_participated` - Number of epochs staked
- `total_rewards_earned` - Total staking rewards earned
- `total_rewards_generated` - Total rewards generated

#### 4. Get Staking by Address and Epoch

**Request:**
```
GET /v1/staking/addresses/:address/epochs/:epoch
```

**Path Parameters:**
- `address` (string, required) - Ethereum address (0x-prefixed)
- `epoch` (integer, required) - Epoch number

**Response Structure:**
```json
{
  "rank": 0,
  "staker_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
  "epoch": 5,
  "staked_amount": "10000000000000000000000",
  "staked_amount_formatted": "10,000.00 ZKC",
  "is_withdrawing": false,
  "rewards_delegated_to": "0x1234567890123456789012345678901234567890",
  "votes_delegated_to": "0x1234567890123456789012345678901234567890",
  "rewards_generated": "500000000000000000",
  "rewards_generated_formatted": "0.50 ZKC",
  "rewards_earned": "100000000000000000",
  "rewards_earned_formatted": "0.10 ZKC"
}
```

**Response Fields:**
Same as epoch staking entry, but for a single address/epoch combination. Returns `null` if no staking position exists for the specified combination.

## Common Field Descriptions

### Pagination Fields
- `offset` - Number of items skipped
- `limit` - Maximum items per page
- `total` - Total items available (when known)

### Formatting Fields
- `*_formatted` - Human-readable format with units
  - ZKC amounts: "1,234.57 ZKC"
  - Cycles: "1,234.57 MCycles"
- `last_updated_at` - ISO 8601 timestamp of last data update

### Address Fields
- `work_log_id` - Ethereum address for PoVW submissions
- `staker_address` - Ethereum address of staker
- `rewards_delegated_to` - Address receiving staking rewards
- `votes_delegated_to` - Address receiving voting power

### Numeric Fields
- Raw values: String representation of wei/cycles
- Formatted values: Human-readable with commas and units
- `rank` - Position in leaderboard (1-indexed)
- `epoch` - Epoch number (0-indexed, weekly periods)

## Error Response Structure

```json
{
  "error": "<error message>"
}
```

### Common Error Codes
- `400 Bad Request` - Invalid parameters or address format
- `404 Not Found` - Resource not found
- `429 Too Many Requests` - Rate limit exceeded
- `500 Internal Server Error` - Server error

## API Behavior

### Rate Limiting
- 100 requests per minute per IP address
- Returns 429 status when exceeded

### Caching
Response headers include cache control:
- Current/aggregate data: 60 second cache
- Historical/epoch data: 5 minute cache

### Data Freshness
- `last_updated_at` field indicates data currency
- Updates occur when new blockchain events are indexed
- Typical delay: 1-2 minutes from on-chain activity