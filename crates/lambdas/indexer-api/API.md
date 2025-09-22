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
  }
}
```

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

Get the current aggregate staking leaderboard across all epochs.

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

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
  }
}
```

### Get Staking by Epoch

#### GET `/v1/staking/epochs/{epoch}`

Get staking positions for a specific epoch.

**Path Parameters:**
- `epoch` (required, number): The epoch number

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

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
  }
}
```

### Get Staking History by Address

#### GET `/v1/staking/addresses/{address}`

Get complete staking history for a specific address across all epochs.

**Path Parameters:**
- `address` (required, string): Ethereum address (0x-prefixed, checksummed)

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)
- `start_epoch` (optional, number): Filter results from this epoch onwards
- `end_epoch` (optional, number): Filter results up to this epoch

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

### Get Aggregate PoVW Leaderboard

#### GET `/v1/povw`

Get the current aggregate PoVW rewards leaderboard across all epochs.

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

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
  }
}
```

### Get PoVW by Epoch

#### GET `/v1/povw/epochs/{epoch}`

Get PoVW rewards for a specific epoch.

**Path Parameters:**
- `epoch` (required, number): The epoch number

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)

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
  }
}
```

### Get PoVW History by Address

#### GET `/v1/povw/addresses/{address}`

Get complete PoVW rewards history for a specific work log ID across all epochs.

**Path Parameters:**
- `address` (required, string): Work log ID (Ethereum address format)

**Query Parameters:**
- `limit` (optional, number): Maximum number of results (default: 50, max: 100)
- `offset` (optional, number): Pagination offset (default: 0)
- `start_epoch` (optional, number): Filter results from this epoch onwards
- `end_epoch` (optional, number): Filter results up to this epoch

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
