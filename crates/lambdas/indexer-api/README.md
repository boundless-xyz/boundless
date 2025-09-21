# Indexer API Lambda

AWS Lambda function providing REST API access to the indexed PoVW rewards data.

## Environment Variables

- `DB_URL` (required) - PostgreSQL connection string to the indexer database
- `LOG_LEVEL` (optional) - Tracing log level (default: info)

## API Endpoints

### Health Check
- `GET /health` - Returns health status

### PoVW Rewards Leaderboard

#### Aggregate Leaderboard
- `GET /v1/rewards/povw/leaderboard`
  - Query parameters:
    - `limit` (optional): Number of results to return (default: 50, max: 100)
    - `offset` (optional): Number of results to skip (default: 0)
  - Returns: Paginated list of work log IDs sorted by total rewards across all epochs

#### Epoch-specific Leaderboard
- `GET /v1/rewards/povw/leaderboard/epoch/{epoch_num}`
  - Path parameters:
    - `epoch_num`: The epoch number to query
  - Query parameters:
    - `limit` (optional): Number of results to return (default: 50, max: 100)
    - `offset` (optional): Number of results to skip (default: 0)
  - Returns: Paginated list of work log IDs sorted by actual rewards for the specified epoch

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