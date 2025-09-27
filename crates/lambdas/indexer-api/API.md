# Boundless Indexer API

REST API for querying Boundless protocol data including PoVW rewards leaderboards, staking positions, and delegation powers.

## Base URL

```
https://d2h2e3zcs9hfb5.cloudfront.net
```

## Endpoints Summary

### PoVW Endpoints
- `GET /v1/povw` - Get global PoVW summary statistics
- `GET /v1/povw/addresses` - Get all-time PoVW leaderboard
- `GET /v1/povw/epochs` - Get list of all epochs with summary data
- `GET /v1/povw/epochs/:epoch` - Get summary for a specific epoch
- `GET /v1/povw/epochs/:epoch/addresses` - Get PoVW leaderboard for a specific epoch
- `GET /v1/povw/epochs/:epoch/addresses/:address` - Get PoVW rewards for an address at a specific epochstaking_positions_by_epoch
- `GET /v1/povw/addresses/:address` - Get PoVW rewards history for a specific address

### Staking Endpoints
- `GET /v1/staking` - Get global staking summary statistics
- `GET /v1/staking/addresses` - Get all-time staking leaderboard
- `GET /v1/staking/epochs` - Get list of all epochs with staking summary
- `GET /v1/staking/epochs/:epoch` - Get staking summary for a specific epoch
- `GET /v1/staking/epochs/:epoch/addresses` - Get staking leaderboard for a specific epoch
- `GET /v1/staking/epochs/:epoch/addresses/:address` - Get staking position for an address at a specific epoch
- `GET /v1/staking/addresses/:address` - Get staking history for a specific address

### Delegation Endpoints

#### Vote Delegations
- `GET /v1/delegations/votes` - Get aggregate vote delegation leaderboard
- `GET /v1/delegations/votes/epochs/:epoch` - Get vote delegation leaderboard for a specific epoch
- `GET /v1/delegations/votes/addresses/:address` - Get vote delegation history for a specific delegate
- `GET /v1/delegations/votes/addresses/:address/epochs/:epoch` - Get vote delegation for a delegate at a specific epoch

#### Reward Delegations
- `GET /v1/delegations/rewards` - Get aggregate reward delegation leaderboard
- `GET /v1/delegations/rewards/epochs/:epoch` - Get reward delegation leaderboard for a specific epoch
- `GET /v1/delegations/rewards/addresses/:address` - Get reward delegation history for a specific delegate
- `GET /v1/delegations/rewards/addresses/:address/epochs/:epoch` - Get reward delegation for a delegate at a specific epoch

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
  - `count` - Number of items returned
- `summary` - Optional summary statistics object

## Detailed Endpoint Documentation

### PoVW Endpoints

#### 1. Get Global PoVW Summary

**Request:**
```
GET /v1/povw
```

**Response Structure:**
```json
{
  "summary": {
    "total_epochs_with_work": 10,
    "total_unique_work_log_ids": 50,
    "total_work_all_time": "1000000000000000000000",
    "total_work_all_time_formatted": "1,000.00",
    "total_emissions_all_time": "5000000000000000000000",
    "total_emissions_all_time_formatted": "5,000.00 ZKC",
    "total_capped_rewards_all_time": "4500000000000000000000",
    "total_capped_rewards_all_time_formatted": "4,500.00 ZKC",
    "total_uncapped_rewards_all_time": "5000000000000000000000",
    "total_uncapped_rewards_all_time_formatted": "5,000.00 ZKC"
  }
}
```

**Response Fields:**
- `total_epochs_with_work` - Number of epochs where work was submitted
- `total_unique_work_log_ids` - Total unique addresses that have submitted work
- `total_work_all_time` - Sum of all work submitted across all epochs
- `total_emissions_all_time` - Total PoVW emissions distributed
- `total_capped_rewards_all_time` - Total rewards after applying caps
- `total_uncapped_rewards_all_time` - Total rewards before applying caps

#### 2. Get All-Time PoVW Leaderboard

**Request:**
```
GET /v1/povw/addresses?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "total_work_submitted": "100000000000000000000",
      "total_work_submitted_formatted": "100.00",
      "total_actual_rewards": "500000000000000000000",
      "total_actual_rewards_formatted": "500.00 ZKC",
      "total_uncapped_rewards": "600000000000000000000",
      "total_uncapped_rewards_formatted": "600.00 ZKC",
      "epochs_participated": 5
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 50
  }
}
```

**Response Fields:**
- `rank` - Position in the leaderboard
- `work_log_id` - Ethereum address of the work submitter
- `total_work_submitted` - Total work submitted across all epochs
- `total_actual_rewards` - Total rewards earned after caps
- `total_uncapped_rewards` - Total rewards before caps
- `epochs_participated` - Number of epochs this address participated in

#### 3. Get Epochs List

**Request:**
```
GET /v1/povw/epochs?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "epoch": 5,
      "total_work": "200000000000000000000",
      "total_work_formatted": "200.00",
      "total_emissions": "1000000000000000000000",
      "total_emissions_formatted": "1,000.00 ZKC",
      "total_capped_rewards": "950000000000000000000",
      "total_capped_rewards_formatted": "950.00 ZKC",
      "total_uncapped_rewards": "1000000000000000000000",
      "total_uncapped_rewards_formatted": "1,000.00 ZKC",
      "epoch_start_time": 1704067200,
      "epoch_end_time": 1704672000,
      "num_participants": 25
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 10
  }
}
```

#### 4. Get Epoch PoVW Summary

**Request:**
```
GET /v1/povw/epochs/:epoch
```

**Response Structure:**
```json
{
  "summary": {
    "epoch": 5,
    "total_work": "200000000000000000000",
    "total_work_formatted": "200.00",
    "total_emissions": "1000000000000000000000",
    "total_emissions_formatted": "1,000.00 ZKC",
    "total_capped_rewards": "950000000000000000000",
    "total_capped_rewards_formatted": "950.00 ZKC",
    "total_uncapped_rewards": "1000000000000000000000",
    "total_uncapped_rewards_formatted": "1,000.00 ZKC",
    "epoch_start_time": 1704067200,
    "epoch_end_time": 1704672000,
    "num_participants": 25
  }
}
```

#### 5. Get Epoch PoVW Leaderboard

**Request:**
```
GET /v1/povw/epochs/:epoch/addresses?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "work_submitted": "50000000000000000000",
      "work_submitted_formatted": "50.00",
      "percentage": 25.0,
      "uncapped_rewards": "250000000000000000000",
      "uncapped_rewards_formatted": "250.00 ZKC",
      "reward_cap": "200000000000000000000",
      "reward_cap_formatted": "200.00 ZKC",
      "actual_rewards": "200000000000000000000",
      "actual_rewards_formatted": "200.00 ZKC",
      "is_capped": true,
      "staked_amount": "10000000000000000000000",
      "staked_amount_formatted": "10,000.00 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 25
  },
  "summary": {
    "epoch": 5,
    "total_work": "200000000000000000000",
    "total_emissions": "1000000000000000000000",
    "num_participants": 25
  }
}
```

#### 6. Get PoVW by Address and Epoch

**Request:**
```
GET /v1/povw/epochs/:epoch/addresses/:address
```

**Response Structure:**
```json
{
  "rank": 1,
  "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
  "epoch": 5,
  "work_submitted": "50000000000000000000",
  "work_submitted_formatted": "50.00",
  "percentage": 25.0,
  "uncapped_rewards": "250000000000000000000",
  "uncapped_rewards_formatted": "250.00 ZKC",
  "reward_cap": "200000000000000000000",
  "reward_cap_formatted": "200.00 ZKC",
  "actual_rewards": "200000000000000000000",
  "actual_rewards_formatted": "200.00 ZKC",
  "is_capped": true,
  "staked_amount": "10000000000000000000000",
  "staked_amount_formatted": "10,000.00 ZKC"
}
```

#### 7. Get PoVW History by Address

**Request:**
```
GET /v1/povw/addresses/:address?offset=0&limit=100
```

**Optional Query Parameters:**
- `start_epoch` (integer) - Filter results from this epoch onwards
- `end_epoch` (integer) - Filter results up to this epoch

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "work_submitted": "50000000000000000000",
      "work_submitted_formatted": "50.00",
      "percentage": 25.0,
      "uncapped_rewards": "250000000000000000000",
      "uncapped_rewards_formatted": "250.00 ZKC",
      "reward_cap": "200000000000000000000",
      "reward_cap_formatted": "200.00 ZKC",
      "actual_rewards": "200000000000000000000",
      "actual_rewards_formatted": "200.00 ZKC",
      "is_capped": true,
      "staked_amount": "10000000000000000000000",
      "staked_amount_formatted": "10,000.00 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 5
  },
  "summary": {
    "work_log_id": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
    "total_work_submitted": "100000000000000000000",
    "total_work_submitted_formatted": "100.00",
    "total_actual_rewards": "500000000000000000000",
    "total_actual_rewards_formatted": "500.00 ZKC",
    "total_uncapped_rewards": "600000000000000000000",
    "total_uncapped_rewards_formatted": "600.00 ZKC",
    "epochs_participated": 5
  }
}
```

### Staking Endpoints

#### 1. Get Global Staking Summary

**Request:**
```
GET /v1/staking
```

**Response Structure:**
```json
{
  "summary": {
    "current_total_staked": "100000000000000000000000",
    "current_total_staked_formatted": "100,000.00 ZKC",
    "total_unique_stakers": 500,
    "current_active_stakers": 450,
    "current_withdrawing": 50,
    "total_staking_emissions_all_time": "50000000000000000000000",
    "total_staking_emissions_all_time_formatted": "50,000.00 ZKC"
  }
}
```

#### 2. Get All-Time Staking Leaderboard

**Request:**
```
GET /v1/staking/addresses?offset=0&limit=100
```

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
      "total_rewards_generated": "5000000000000000000",
      "total_rewards_generated_formatted": "5.00 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 450
  }
}
```

#### 3. Get Staking Epochs List

**Request:**
```
GET /v1/staking/epochs?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "epoch": 5,
      "total_staked": "100000000000000000000000",
      "total_staked_formatted": "100,000.00 ZKC",
      "num_stakers": 450,
      "num_withdrawing": 45,
      "total_staking_emissions": "10000000000000000000000",
      "total_staking_emissions_formatted": "10,000.00 ZKC",
      "total_staking_power": "95000000000000000000000",
      "total_staking_power_formatted": "95,000.00 ZKC",
      "num_reward_recipients": 425,
      "epoch_start_time": 1704067200,
      "epoch_end_time": 1704672000
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 10
  }
}
```

#### 4. Get Epoch Staking Summary

**Request:**
```
GET /v1/staking/epochs/:epoch
```

**Response Structure:**
```json
{
  "summary": {
    "epoch": 5,
    "total_staked": "100000000000000000000000",
    "total_staked_formatted": "100,000.00 ZKC",
    "num_stakers": 450,
    "num_withdrawing": 45,
    "total_staking_emissions": "10000000000000000000000",
    "total_staking_emissions_formatted": "10,000.00 ZKC",
    "total_staking_power": "95000000000000000000000",
    "total_staking_power_formatted": "95,000.00 ZKC",
    "num_reward_recipients": 425,
    "epoch_start_time": 1704067200,
    "epoch_end_time": 1704672000
  }
}
```

#### 5. Get Epoch Staking Leaderboard

**Request:**
```
GET /v1/staking/epochs/:epoch/addresses?offset=0&limit=100
```

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
      "rewards_generated_formatted": "0.50 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 450
  }
}
```

#### 6. Get Staking by Address and Epoch

**Request:**
```
GET /v1/staking/epochs/:epoch/addresses/:address
```

**Response Structure:**
```json
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
  "rewards_generated_formatted": "0.50 ZKC"
}
```

#### 7. Get Staking History by Address

**Request:**
```
GET /v1/staking/addresses/:address?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
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
      "rewards_generated_formatted": "0.50 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 5
  },
  "summary": {
    "staker_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
    "total_staked": "10000000000000000000000",
    "total_staked_formatted": "10,000.00 ZKC",
    "is_withdrawing": false,
    "rewards_delegated_to": "0x1234567890123456789012345678901234567890",
    "votes_delegated_to": "0x1234567890123456789012345678901234567890",
    "epochs_participated": 5,
    "total_rewards_generated": "5000000000000000000",
    "total_rewards_generated_formatted": "5.00 ZKC"
  }
}
```

### Delegation Endpoints

#### Vote Delegations

##### 1. Get Aggregate Vote Delegations

**Request:**
```
GET /v1/delegations/votes?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "power": "50000000000000000000000",
      "power_formatted": "50,000.00 ZKC",
      "delegator_count": 10,
      "delegators": [
        "0x1111111111111111111111111111111111111111",
        "0x2222222222222222222222222222222222222222"
      ],
      "epochs_participated": 5
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 50
  }
}
```

##### 2. Get Epoch Vote Delegations

**Request:**
```
GET /v1/delegations/votes/epochs/:epoch?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "power": "50000000000000000000000",
      "power_formatted": "50,000.00 ZKC",
      "delegator_count": 10,
      "delegators": [
        "0x1111111111111111111111111111111111111111",
        "0x2222222222222222222222222222222222222222"
      ]
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 50
  }
}
```

##### 3. Get Vote Delegation History by Address

**Request:**
```
GET /v1/delegations/votes/addresses/:address?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 0,
      "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "power": "50000000000000000000000",
      "power_formatted": "50,000.00 ZKC",
      "delegator_count": 10,
      "delegators": [
        "0x1111111111111111111111111111111111111111"
      ]
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 5
  }
}
```

##### 4. Get Vote Delegation by Address and Epoch

**Request:**
```
GET /v1/delegations/votes/addresses/:address/epochs/:epoch
```

**Response Structure:**
```json
{
  "rank": 1,
  "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
  "epoch": 5,
  "power": "50000000000000000000000",
  "power_formatted": "50,000.00 ZKC",
  "delegator_count": 10,
  "delegators": [
    "0x1111111111111111111111111111111111111111",
    "0x2222222222222222222222222222222222222222"
  ]
}
```

#### Reward Delegations

##### 1. Get Aggregate Reward Delegations

**Request:**
```
GET /v1/delegations/rewards?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "power": "50000000000000000000000",
      "power_formatted": "50,000.00 ZKC",
      "delegator_count": 10,
      "delegators": [
        "0x1111111111111111111111111111111111111111",
        "0x2222222222222222222222222222222222222222"
      ],
      "epochs_participated": 5,
      "rewards_earned": "1000000000000000000",
      "rewards_earned_formatted": "1.00 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 50
  }
}
```

**Response Fields:**
- `rewards_earned` - Total rewards earned by this delegate (from delegations + own stake if not delegated)

##### 2. Get Epoch Reward Delegations

**Request:**
```
GET /v1/delegations/rewards/epochs/:epoch?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 1,
      "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "power": "50000000000000000000000",
      "power_formatted": "50,000.00 ZKC",
      "delegator_count": 10,
      "delegators": [
        "0x1111111111111111111111111111111111111111",
        "0x2222222222222222222222222222222222222222"
      ],
      "rewards_earned": "100000000000000000",
      "rewards_earned_formatted": "0.10 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 50
  }
}
```

##### 3. Get Reward Delegation History by Address

**Request:**
```
GET /v1/delegations/rewards/addresses/:address?offset=0&limit=100
```

**Response Structure:**
```json
{
  "entries": [
    {
      "rank": 0,
      "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
      "epoch": 5,
      "power": "50000000000000000000000",
      "power_formatted": "50,000.00 ZKC",
      "delegator_count": 10,
      "delegators": [
        "0x1111111111111111111111111111111111111111"
      ],
      "rewards_earned": "100000000000000000",
      "rewards_earned_formatted": "0.10 ZKC"
    }
  ],
  "pagination": {
    "offset": 0,
    "limit": 100,
    "count": 5
  }
}
```

##### 4. Get Reward Delegation by Address and Epoch

**Request:**
```
GET /v1/delegations/rewards/addresses/:address/epochs/:epoch
```

**Response Structure:**
```json
{
  "rank": 1,
  "delegate_address": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
  "epoch": 5,
  "power": "50000000000000000000000",
  "power_formatted": "50,000.00 ZKC",
  "delegator_count": 10,
  "delegators": [
    "0x1111111111111111111111111111111111111111",
    "0x2222222222222222222222222222222222222222"
  ],
  "rewards_earned": "100000000000000000",
  "rewards_earned_formatted": "0.10 ZKC"
}
```

## Error Responses

All endpoints may return the following error responses:

### 400 Bad Request
```json
{
  "error": "Invalid address format"
}
```

### 404 Not Found
```json
{
  "error": "Address not found"
}
```

### 500 Internal Server Error
```json
{
  "error": "Internal server error"
}
```

## Rate Limiting

The API currently does not enforce rate limits, but this may change in the future. Please implement reasonable request throttling in your applications.

## Data Freshness

- Data is indexed from the Ethereum blockchain
- There may be a slight delay between on-chain events and API updates
- The `last_updated_at` field (when present) indicates the timestamp of the most recent data