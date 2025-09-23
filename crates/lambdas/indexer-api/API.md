# Boundless Indexer API Documentation

## Overview

The Boundless Indexer API provides access to staking, delegation, and Proof of Verifiable Work (PoVW) data for the Boundless protocol. The API is RESTful and returns JSON responses.

## Rate Limiting

- CloudFront CDN: 75 requests per 5 minutes per IP

## Common Response Format

All list endpoints return paginated responses with this structure:

```json
{
  "entries": [...],
  "pagination": {
    "count": 10,
    "offset": 0,
    "limit": 50
  },
  "summary": {
    // Optional summary statistics (endpoint-specific)
  }
}
```

The `summary` field is optional and only included when summary data is available in the database.

## Documentation

### Interactive API Documentation

#### GET `/docs`

Interactive Swagger UI documentation with try-it-out functionality. Open this endpoint in a browser to explore and test the API.

## Endpoints

### Health Check

#### GET `/health`

Check API health status.

**Response:**
```json
{
  "status": "healthy",
  "service": "indexer-api"
}
```

---

## Staking Endpoints

### Get Aggregate Staking Leaderboard

#### GET `/v1/staking`

Get the current aggregate staking leaderboard across all epochs with global staking statistics.

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

**Response Structure:**
```typescript
{
  "entries": Array<{
    "rank": number,                           // Position in leaderboard (1-based)
    "staker_address": string,                 // Ethereum address (0x-prefixed)
    "total_staked": string,                   // Total amount staked (wei)
    "is_withdrawing": boolean,                // Current withdrawal status
    "rewards_delegated_to": string | null,    // Reward delegation address
    "votes_delegated_to": string | null,      // Vote delegation address
    "epochs_participated": number             // Number of epochs participated
  }>,
  "pagination": {
    "count": number,                           // Number of entries returned
    "offset": number,                          // Pagination offset used
    "limit": number                            // Pagination limit used
  },
  "summary": {                                 // Optional global statistics
    "current_total_staked": string,           // Total amount currently staked (wei)
    "total_unique_stakers": number,           // Total number of unique stakers all-time
    "current_active_stakers": number,         // Number of currently active stakers
    "current_withdrawing": number             // Number of stakers currently withdrawing
  }
}
```

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "staker_address": "0x2408e37489c231f883126c87e8aadbad782a040a",
      "total_staked": "788626950526189926000000",
      "is_withdrawing": false,
      "rewards_delegated_to": "0x0164ec96442196a02931f57e7e20fa59cff43845",
      "votes_delegated_to": null,
      "epochs_participated": 4
    }
  ],
  "pagination": {
    "count": 1,
    "offset": 0,
    "limit": 50
  },
  "summary": {
    "current_total_staked": "788626950526189926000000",
    "total_unique_stakers": 15,
    "current_active_stakers": 12,
    "current_withdrawing": 3
  }
}
```

### Get Staking by Epoch

#### GET `/v1/staking/epochs/{epoch}`

Get staking positions for a specific epoch with epoch-level summary statistics.

**Path Parameters:**
- `epoch` (required, number): The epoch number

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

**Response Structure:**
```typescript
{
  "entries": Array<{
    "rank": number,                           // Position in leaderboard (1-based)
    "staker_address": string,                 // Ethereum address (0x-prefixed)
    "epoch": number,                           // Epoch number
    "staked_amount": string,                  // Amount staked in this epoch (wei)
    "is_withdrawing": boolean,                // Withdrawal status during this epoch
    "rewards_delegated_to": string | null,    // Reward delegation address
    "votes_delegated_to": string | null       // Vote delegation address
  }>,
  "pagination": {
    "count": number,                           // Number of entries returned
    "offset": number,                          // Pagination offset used
    "limit": number                            // Pagination limit used
  },
  "summary": {                                 // Optional epoch statistics
    "epoch": number,                           // The epoch number
    "total_staked": string,                   // Total staked in this epoch (wei)
    "num_stakers": number,                     // Number of stakers in this epoch
    "num_withdrawing": number                  // Number withdrawing in this epoch
  }
}
```

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "staker_address": "0x2408e37489c231f883126c87e8aadbad782a040a",
      "epoch": 5,
      "staked_amount": "788626950526189926000000",
      "is_withdrawing": false,
      "rewards_delegated_to": "0x0164ec96442196a02931f57e7e20fa59cff43845",
      "votes_delegated_to": null
    }
  ],
  "pagination": {
    "count": 1,
    "offset": 0,
    "limit": 50
  },
  "summary": {
    "epoch": 5,
    "total_staked": "1500000000000000000000000",
    "num_stakers": 8,
    "num_withdrawing": 2
  }
}
```

### Get Staking History by Address

#### GET `/v1/staking/addresses/{address}`

Get complete staking history for a specific address across all epochs with lifetime summary statistics.

**Path Parameters:**
- `address` (required, string): Ethereum address (0x-prefixed, checksummed)

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)
- `start_epoch` (optional, number): Filter results from this epoch onwards
- `end_epoch` (optional, number): Filter results up to this epoch

**Response Structure:**
```typescript
{
  "entries": Array<{
    "rank": number,                           // Position in results (1-based)
    "staker_address": string,                 // Ethereum address (0x-prefixed)
    "epoch": number,                           // Epoch number
    "staked_amount": string,                  // Amount staked in this epoch (wei)
    "is_withdrawing": boolean,                // Withdrawal status during this epoch
    "rewards_delegated_to": string | null,    // Reward delegation address
    "votes_delegated_to": string | null       // Vote delegation address
  }>,
  "pagination": {
    "count": number,                           // Number of entries returned
    "offset": number,                          // Pagination offset used
    "limit": number                            // Pagination limit used
  },
  "summary": {                                 // Optional lifetime statistics for this address
    "staker_address": string,                 // The staker's address
    "total_staked": string,                   // Current total staked (wei)
    "is_withdrawing": boolean,                // Current withdrawal status
    "rewards_delegated_to": string | null,    // Current reward delegation
    "votes_delegated_to": string | null,      // Current vote delegation
    "epochs_participated": number             // Total epochs participated
  }
}
```

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "staker_address": "0x2408e37489c231f883126c87e8aadbad782a040a",
      "epoch": 5,
      "staked_amount": "788626950526189926000000",
      "is_withdrawing": false,
      "rewards_delegated_to": "0x0164ec96442196a02931f57e7e20fa59cff43845",
      "votes_delegated_to": null
    },
    {
      "rank": 2,
      "staker_address": "0x2408e37489c231f883126c87e8aadbad782a040a",
      "epoch": 4,
      "staked_amount": "750500230877756033000000",
      "is_withdrawing": false,
      "rewards_delegated_to": "0x0164ec96442196a02931f57e7e20fa59cff43845",
      "votes_delegated_to": null
    }
  ],
  "pagination": {
    "count": 2,
    "offset": 0,
    "limit": 50
  },
  "summary": {
    "staker_address": "0x2408e37489c231f883126c87e8aadbad782a040a",
    "total_staked": "788626950526189926000000",
    "is_withdrawing": false,
    "rewards_delegated_to": "0x0164ec96442196a02931f57e7e20fa59cff43845",
    "votes_delegated_to": null,
    "epochs_participated": 4
  }
}
```

### Get Staking by Address and Epoch

#### GET `/v1/staking/addresses/{address}/epochs/{epoch}`

Get staking position for a specific address at a specific epoch.

**Path Parameters:**
- `address` (required, string): Ethereum address (0x-prefixed, checksummed)
- `epoch` (required, number): The epoch number

**Response Example:**
```json
{
  "staker_address": "0x2408e37489c231f883126c87e8aadbad782a040a",
  "epoch": 5,
  "staked_amount": "788626950526189926000000",
  "is_withdrawing": false,
  "rewards_delegated_to": "0x0164ec96442196a02931f57e7e20fa59cff43845",
  "votes_delegated_to": null
}
```

---

## PoVW (Proof of Verifiable Work) Endpoints

### Important Notes on Emissions vs Rewards

The PoVW system allocates emissions for each epoch, but rewards are only distributed when work is actually submitted:

- **Emissions** (`total_emissions`): The amount of tokens allocated by the smart contract for an epoch. These are set regardless of whether any work is submitted.
- **Uncapped Rewards** (`total_uncapped_rewards`): The sum of proportional rewards calculated based on actual work submitted. When no work is submitted, this will be 0.
- **Why they differ**: If an epoch has no work submitted (common in early epochs like epoch 0), the emissions remain unspent. These unallocated tokens are never distributed.

**Example**: Epoch 0 typically has emissions allocated but no work submitted, resulting in `total_emissions > 0` but `total_uncapped_rewards = 0`.

---

### Get Aggregate PoVW Leaderboard

#### GET `/v1/povw`

Get the current aggregate PoVW rewards leaderboard across all epochs with global PoVW statistics.

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

**Response Structure:**
```typescript
{
  "entries": Array<{
    "rank": number,                           // Position in leaderboard (1-based)
    "work_log_id": string,                    // Work log ID (Ethereum address)
    "total_work_submitted": string,           // Total work submitted (wei)
    "total_actual_rewards": string,           // Total actual rewards earned (wei)
    "total_uncapped_rewards": string,         // Total uncapped rewards (wei)
    "epochs_participated": number             // Number of epochs participated
  }>,
  "pagination": {
    "count": number,                           // Number of entries returned
    "offset": number,                          // Pagination offset used
    "limit": number                            // Pagination limit used
  },
  "summary": {                                 // Optional global PoVW statistics
    "total_epochs_with_work": number,         // Number of epochs with work submitted
    "total_unique_work_log_ids": number,      // Number of unique work log IDs
    "total_work_all_time": string,            // Total work submitted all-time (wei)
    "total_emissions_all_time": string,       // Total emissions allocated by contract all-time (wei)
                                              // Note: Includes emissions for epochs with no work (e.g., epoch 0)
    "total_capped_rewards_all_time": string,  // Total actual rewards distributed after caps (wei)
    "total_uncapped_rewards_all_time": string // Total proportional rewards calculated from work (wei)
                                              // Note: Only includes rewards when work > 0
                                              // Will be less than emissions if any epochs had no work
  }
}
```

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x94072d2282cb2c718d23d5779a5f8484e2530f2a",
      "total_work_submitted": "1000000000000000000",
      "total_actual_rewards": "500000000000000000000",
      "total_uncapped_rewards": "600000000000000000000",
      "epochs_participated": 3
    }
  ],
  "pagination": {
    "count": 1,
    "offset": 0,
    "limit": 50
  },
  "summary": {
    "total_epochs_with_work": 5,
    "total_unique_work_log_ids": 12,
    "total_work_all_time": "5000000000000000000",
    "total_emissions_all_time": "2500000000000000000000",
    "total_capped_rewards_all_time": "2000000000000000000000",
    "total_uncapped_rewards_all_time": "2500000000000000000000"
  }
}
```

### Get PoVW by Epoch

#### GET `/v1/povw/epochs/{epoch}`

Get PoVW rewards for a specific epoch with epoch-level summary statistics.

**Path Parameters:**
- `epoch` (required, number): The epoch number

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

**Response Structure:**
```typescript
{
  "entries": Array<{
    "rank": number,                           // Position in leaderboard (1-based)
    "work_log_id": string,                    // Work log ID (Ethereum address)
    "epoch": number,                           // Epoch number
    "work_submitted": string,                 // Work submitted in this epoch (wei)
    "percentage": number,                      // Percentage of total work (0-100)
    "uncapped_rewards": string,               // Rewards before cap (wei)
    "reward_cap": string,                     // Maximum allowed rewards (wei)
    "actual_rewards": string,                 // Actual rewards after cap (wei)
    "is_capped": boolean,                     // Whether rewards were capped
    "staked_amount": string                   // Staked amount for cap calculation (wei)
  }>,
  "pagination": {
    "count": number,                           // Number of entries returned
    "offset": number,                          // Pagination offset used
    "limit": number                            // Pagination limit used
  },
  "summary": {                                 // Optional epoch PoVW statistics
    "epoch": number,                           // The epoch number
    "total_work": string,                     // Total work in this epoch (wei)
    "total_emissions": string,                // Emissions allocated for this epoch (wei)
                                              // Note: Set by contract regardless of work submitted
    "total_capped_rewards": string,           // Total actual rewards after caps (wei)
    "total_uncapped_rewards": string,         // Total proportional rewards before caps (wei)
                                              // Note: Will be 0 if no work was submitted (e.g., epoch 0)
    "epoch_start_time": number,               // Unix timestamp of epoch start
    "epoch_end_time": number,                 // Unix timestamp of epoch end
    "num_participants": number                // Number of participants
  }
}
```

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x94072d2282cb2c718d23d5779a5f8484e2530f2a",
      "epoch": 2,
      "work_submitted": "500000000000000000",
      "percentage": 25.5,
      "uncapped_rewards": "300000000000000000000",
      "reward_cap": "250000000000000000000",
      "actual_rewards": "250000000000000000000",
      "is_capped": true,
      "staked_amount": "100000000000000000000"
    }
  ],
  "pagination": {
    "count": 1,
    "offset": 0,
    "limit": 50
  },
  "summary": {
    "epoch": 2,
    "total_work": "2000000000000000000",
    "total_emissions": "1000000000000000000000",
    "total_capped_rewards": "900000000000000000000",
    "total_uncapped_rewards": "1000000000000000000000",
    "epoch_start_time": 1704067200,
    "epoch_end_time": 1704672000,
    "num_participants": 8
  }
}
```

### Get PoVW History by Address

#### GET `/v1/povw/addresses/{address}`

Get complete PoVW rewards history for a specific work log ID across all epochs with lifetime summary statistics.

**Path Parameters:**
- `address` (required, string): Work log ID (Ethereum address format)

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)
- `start_epoch` (optional, number): Filter results from this epoch onwards
- `end_epoch` (optional, number): Filter results up to this epoch

**Response Structure:**
```typescript
{
  "entries": Array<{
    "rank": number,                           // Position in results (1-based)
    "work_log_id": string,                    // Work log ID (Ethereum address)
    "epoch": number,                           // Epoch number
    "work_submitted": string,                 // Work submitted in this epoch (wei)
    "percentage": number,                      // Percentage of total work (0-100)
    "uncapped_rewards": string,               // Rewards before cap (wei)
    "reward_cap": string,                     // Maximum allowed rewards (wei)
    "actual_rewards": string,                 // Actual rewards after cap (wei)
    "is_capped": boolean,                     // Whether rewards were capped
    "staked_amount": string                   // Staked amount for cap calculation (wei)
  }>,
  "pagination": {
    "count": number,                           // Number of entries returned
    "offset": number,                          // Pagination offset used
    "limit": number                            // Pagination limit used
  },
  "summary": {                                 // Optional lifetime statistics for this address
    "work_log_id": string,                    // The work log ID
    "total_work_submitted": string,           // Lifetime total work submitted (wei)
    "total_actual_rewards": string,           // Lifetime total actual rewards (wei)
    "total_uncapped_rewards": string,         // Lifetime total uncapped rewards (wei)
    "epochs_participated": number             // Total epochs participated
  }
}
```

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "work_log_id": "0x94072d2282cb2c718d23d5779a5f8484e2530f2a",
      "epoch": 3,
      "work_submitted": "600000000000000000",
      "percentage": 30.0,
      "uncapped_rewards": "350000000000000000000",
      "reward_cap": "350000000000000000000",
      "actual_rewards": "350000000000000000000",
      "is_capped": false,
      "staked_amount": "150000000000000000000"
    }
  ],
  "pagination": {
    "count": 1,
    "offset": 0,
    "limit": 50
  },
  "summary": {
    "work_log_id": "0x94072d2282cb2c718d23d5779a5f8484e2530f2a",
    "total_work_submitted": "1000000000000000000",
    "total_actual_rewards": "500000000000000000000",
    "total_uncapped_rewards": "600000000000000000000",
    "epochs_participated": 3
  }
}
```

### Get PoVW by Address and Epoch

#### GET `/v1/povw/addresses/{address}/epochs/{epoch}`

Get PoVW rewards for a specific work log ID at a specific epoch.

**Path Parameters:**
- `address` (required, string): Work log ID (Ethereum address format)
- `epoch` (required, number): The epoch number

**Response Example:**
```json
{
  "work_log_id": "0x94072d2282cb2c718d23d5779a5f8484e2530f2a",
  "epoch": 2,
  "work_submitted": "500000000000000000",
  "percentage": 25.5,
  "uncapped_rewards": "300000000000000000000",
  "reward_cap": "250000000000000000000",
  "actual_rewards": "250000000000000000000",
  "is_capped": true,
  "staked_amount": "100000000000000000000"
}
```

---

## Delegation Endpoints

### Vote Delegations

#### GET `/v1/delegations/votes`

Get aggregate vote delegation powers across all epochs.

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "delegate_address": "0x1234567890123456789012345678901234567890",
      "power": "1000000000000000000000",
      "delegator_count": 5,
      "delegators": [
        "0xaaaa...",
        "0xbbbb...",
        "0xcccc...",
        "0xdddd...",
        "0xeeee..."
      ],
      "epochs_participated": 3
    }
  ],
  "pagination": {
    "count": 1,
    "offset": 0,
    "limit": 50
  }
}
```

#### GET `/v1/delegations/votes/epochs/{epoch}`

Get vote delegation powers for a specific epoch.

**Path Parameters:**
- `epoch` (required, number): The epoch number

#### GET `/v1/delegations/votes/addresses/{address}`

Get vote delegation history for a specific delegate address.

**Path Parameters:**
- `address` (required, string): Delegate address (0x-prefixed, checksummed)

#### GET `/v1/delegations/votes/addresses/{address}/epochs/{epoch}`

Get vote delegation power for a specific delegate at a specific epoch.

### Reward Delegations

#### GET `/v1/delegations/rewards`

Get aggregate reward delegation powers across all epochs.

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

**Response Example:**
```json
{
  "entries": [
    {
      "rank": 1,
      "delegate_address": "0x0164ec96442196a02931f57e7e20fa59cff43845",
      "power": "788626950526189926000000",
      "delegator_count": 1,
      "delegators": [
        "0x2408e37489c231f883126c87e8aadbad782a040a"
      ],
      "epochs_participated": 4
    }
  ],
  "pagination": {
    "count": 1,
    "offset": 0,
    "limit": 50
  }
}
```

#### GET `/v1/delegations/rewards/epochs/{epoch}`

Get reward delegation powers for a specific epoch.

#### GET `/v1/delegations/rewards/addresses/{address}`

Get reward delegation history for a specific delegate address.

#### GET `/v1/delegations/rewards/addresses/{address}/epochs/{epoch}`

Get reward delegation power for a specific delegate at a specific epoch.

---

## Caching

The API uses CloudFront CDN for caching:

- Current data (aggregate endpoints): 1 minute cache
- Historical epoch data: 5 minutes cache
