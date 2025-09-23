# Indexer API Lambda

AWS Lambda function providing REST API access to Boundless protocol staking, delegation, and PoVW rewards data.

## Environment Variables

- `DB_URL` (required) - PostgreSQL connection string to the indexer database
- `RUST_LOG` (optional) - Tracing log level (default: info)

## API Documentation

### Interactive Documentation

- **`GET /docs`** - Interactive Swagger UI documentation. Open this in a browser to explore and test all API endpoints.

### API Specifications

- `GET /openapi.yaml` - OpenAPI 3.0 specification (YAML format)
- `GET /openapi.json` - OpenAPI 3.0 specification (JSON format)
- `GET /health` - Health check endpoint

## Main API Endpoints

### Staking Endpoints (`/v1/staking`)
- `GET /v1/staking` - Aggregate staking leaderboard
- `GET /v1/staking/epochs/{epoch}` - Staking positions by epoch
- `GET /v1/staking/addresses/{address}` - Staking history for address
- `GET /v1/staking/addresses/{address}/epochs/{epoch}` - Specific position

### PoVW Endpoints (`/v1/povw`)
- `GET /v1/povw` - Aggregate PoVW rewards leaderboard
- `GET /v1/povw/epochs/{epoch}` - PoVW rewards by epoch
- `GET /v1/povw/addresses/{address}` - PoVW history for address
- `GET /v1/povw/addresses/{address}/epochs/{epoch}` - Specific rewards

### Delegation Endpoints (`/v1/delegations`)
- `GET /v1/delegations/votes` - Vote delegation powers
- `GET /v1/delegations/votes/epochs/{epoch}` - Vote delegations by epoch
- `GET /v1/delegations/rewards` - Reward delegation powers
- `GET /v1/delegations/rewards/epochs/{epoch}` - Reward delegations by epoch

All endpoints support pagination with `limit` and `offset` query parameters.

## Response Format

### Aggregate Leaderboard Entry
```json
{
  "rank": 1,
  "work_log_id": "0x...",
  "total_work_submitted": "1234567890",
  "total_rewards": "9876543210",
  "epochs_participated": 10
}
```

### Epoch Leaderboard Entry
```json
{
  "rank": 1,
  "work_log_id": "0x...",
  "epoch": 5,
  "work_submitted": "123456",
  "percentage": 25.5,
  "uncapped_rewards": "987654",
  "reward_cap": "654321",
  "actual_rewards": "654321",
  "is_capped": true
}
```

### Response Wrapper
All leaderboard responses are wrapped with pagination metadata:
```json
{
  "entries": [...],
  "pagination": {
    "count": 50,
    "offset": 0,
    "limit": 50
  }
}
```

## Deployment

This Lambda function is designed to be deployed with AWS Lambda and API Gateway.
Build for Lambda deployment using cargo-lambda or similar tools.