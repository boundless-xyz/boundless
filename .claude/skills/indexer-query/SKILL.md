---
name: indexer-query
description: Query the Boundless Indexer REST API for on-chain market data, staking, PoVW rewards, delegations, and market efficiency on prod/staging environments. Use when the user asks about proof requests, provers, requestors, staking data, PoVW rewards, delegations, market aggregates, leaderboards, or wants to fetch data from the indexer API on live networks. Also use when the user mentions "indexer" and wants to look up addresses, requests, or market statistics on deployed environments. Do NOT use for debugging local code changes, reviewing PRs, or investigating issues in the codebase itself.
---

# Indexer Query

Query the Boundless Indexer REST API to retrieve on-chain data across multiple chains.

## Two Types of Indexer

There are two distinct indexer types with different endpoint sets:

1. **Market Indexers** -- Deployed per L2 chain. Provide market, requestor, prover, and efficiency endpoints.
2. **ZKC Indexers** -- Deployed per ZKC token deployment (Ethereum L1). Provide staking, PoVW, and delegation endpoints.

These endpoint sets do **not** overlap. Market endpoints only work on market indexers; staking/PoVW/delegation endpoints only work on ZKC indexers.

## Network Selection

If the user does not specify which network to query, ask them which network they want. Use the query topic to determine whether you need a market indexer or ZKC indexer (or both).

### Market Indexers

| Network      | Environment | Base URL                                | ZKC Source          |
| ------------ | ----------- | --------------------------------------- | ------------------- |
| Base         | prod        | `https://d2mdvlnmyov1e1.cloudfront.net` | Eth Mainnet Prod    |
| Taiko        | prod        | `https://d29nqt0gudcxhl.cloudfront.net` | Eth Mainnet Prod    |
| Base Sepolia | prod        | `https://d3kkukmpiqlzm1.cloudfront.net` | Eth Sepolia Prod    |
| Taiko        | staging     | `https://d1wuyzcum0d4x4.cloudfront.net` | Eth Sepolia Staging |
| Base Sepolia | staging     | `https://d29i5cy5rhdodq.cloudfront.net` | Eth Sepolia Staging |

Available endpoints: `/v1/market/**`, `/v1/market/requestors/**`, `/v1/market/provers/**`, `/v1/market/efficiency/**`

### ZKC Indexers

| Network     | Environment | Base URL                                | Serves ZKC for                      |
| ----------- | ----------- | --------------------------------------- | ----------------------------------- |
| Eth Mainnet | prod        | `https://d3ig5wxf8178te.cloudfront.net` | Base Prod, Taiko Prod               |
| Eth Sepolia | prod        | `https://d3jjbcwhlw21k7.cloudfront.net` | Base Sepolia Prod                   |
| Eth Sepolia | staging     | `https://d3dtnqspyflstc.cloudfront.net` | Taiko Staging, Base Sepolia Staging |

Available endpoints: `/v1/staking/**`, `/v1/povw/**`, `/v1/delegations/**`

### Choosing the Right Indexer

- For proof requests, provers, requestors, market stats, efficiency -> use the **market indexer** for the relevant L2 chain.
- For staking, PoVW rewards, delegations -> use the **ZKC indexer** for the corresponding Ethereum deployment.
- If the user asks about both (e.g. "show me prover X's market stats and staking info"), you need to query both the market indexer and the corresponding ZKC indexer.

Use prod URLs by default unless the user explicitly asks for staging.

Store the selected base URL(s) in shell variables for reuse:

```bash
export MARKET_INDEXER_URL="https://d2mdvlnmyov1e1.cloudfront.net"
export ZKC_INDEXER_URL="https://d3ig5wxf8178te.cloudfront.net"
```

## Credentials

First, try to read `network_secrets.toml` from the repo root. If it exists, extract the indexer API key for the selected environment from `[environments.<env>.indexer] api_key`. Also read `network_address_labels.json` (same directory) for translating addresses to human-readable labels in output.

If `network_secrets.toml` is not present, recommend the user create it -- instructions and credentials are in the **Boundless runbook**. The API still works without a key, but they'll have lower rate limits.

If `network_address_labels.json` is not present, recommend the user create it -- the canonical address mapping is in the **Boundless runbook**. The file is plain JSON (`{"0xaddr": "label", ...}`).

When an API key is available:

```bash
export INDEXER_API_KEY="the-key"
```

Include it in requests via the `x-api-key` header. Without an API key, omit the header.

## Known Addresses

If `network_address_labels.json` exists at the repo root, use it to label addresses in results. The file is plain JSON (`{"0xaddr": "label", ...}`) so the user can paste directly from the canonical mapping. When displaying addresses from API responses, check if the address (case-insensitive) has a known label and show it alongside, e.g. `0xbdA9...5542 (BP1)`.

## Rate Limiting

The API is protected by AWS WAF rate limits:

- **Without API key**: 200 requests per IP per 5 minutes
- **With API key**: 1000 requests per IP per 5 minutes

Self-rate-limit when making multiple requests:

- Add `sleep 1` between sequential requests
- If a request returns HTTP 429 or 403, back off for 30 seconds before retrying

## Making Requests

All endpoints return JSON. Use `curl -s` and pipe through `jq` for readability.

Define a helper function that takes a full URL:

```bash
indexer_get() {
  local url="$1"
  if [ -n "$INDEXER_API_KEY" ]; then
    curl -s -H "x-api-key: $INDEXER_API_KEY" "$url"
  else
    curl -s "$url"
  fi
}
```

Usage:

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/requests?limit=10" | jq .
indexer_get "${ZKC_INDEXER_URL}/v1/staking/addresses" | jq .
```

## Pagination

Two pagination styles exist:

### Offset pagination (staking, PoVW, delegations -- ZKC indexer)

Uses `limit` (default 50, max 100) and `offset` (default 0). Response includes `pagination: { count, offset, limit }`.

```bash
indexer_get "${ZKC_INDEXER_URL}/v1/staking/addresses?limit=50&offset=0" | jq .
indexer_get "${ZKC_INDEXER_URL}/v1/staking/addresses?limit=50&offset=50" | jq .
```

### Cursor pagination (market endpoints -- market indexer)

Uses `limit` and `cursor` (Base64 opaque token). Response includes `has_more` and `next_cursor`.

```bash
RESP=$(indexer_get "${MARKET_INDEXER_URL}/v1/market/requests?limit=50")
echo "$RESP" | jq .
NEXT=$(echo "$RESP" | jq -r '.next_cursor // empty')
if [ -n "$NEXT" ]; then
  sleep 1
  indexer_get "${MARKET_INDEXER_URL}/v1/market/requests?limit=50&cursor=$NEXT" | jq .
fi
```

To fetch all pages in a loop (with rate limiting):

```bash
CURSOR=""
PAGE=0
BASE_URL="${MARKET_INDEXER_URL}/v1/market/requests"
while true; do
  PARAMS="limit=50"
  [ -n "$CURSOR" ] && PARAMS="${PARAMS}&cursor=${CURSOR}"
  RESP=$(indexer_get "${BASE_URL}?${PARAMS}")
  echo "$RESP" | jq '.data | length' 
  CURSOR=$(echo "$RESP" | jq -r '.next_cursor // empty')
  HAS_MORE=$(echo "$RESP" | jq -r '.has_more')
  PAGE=$((PAGE + 1))
  [ "$HAS_MORE" = "false" ] || [ -z "$CURSOR" ] && break
  sleep 1
done
echo "Fetched $PAGE pages"
```

## Filtering by Time Range

Not all endpoints support `before`/`after` query parameters. The aggregate and cumulative endpoints do, but request list endpoints (`/v1/market/requests`, `/v1/market/provers/:address/requests`, `/v1/market/requestors/:address/requests`) do **not** have server-side time filtering.

When you need requests from a specific time window on these endpoints, you must paginate and inspect timestamps client-side. Results are sorted by `created_at` descending (newest first) by default, so stop paginating once you see a `created_at` older than your cutoff:

```bash
CURSOR=""
CUTOFF="2026-03-25T00:00:00Z"
CUTOFF_UNIX=$(date -j -u -f "%Y-%m-%dT%H:%M:%SZ" "$CUTOFF" +%s 2>/dev/null || date -d "$CUTOFF" +%s)
BASE_URL="${MARKET_INDEXER_URL}/v1/market/requests"
while true; do
  PARAMS="limit=50"
  [ -n "$CURSOR" ] && PARAMS="${PARAMS}&cursor=${CURSOR}"
  RESP=$(indexer_get "${BASE_URL}?${PARAMS}")

  OLDEST=$(echo "$RESP" | jq '[.data[].created_at] | min')
  if [ "$OLDEST" != "null" ] && [ "$OLDEST" -lt "$CUTOFF_UNIX" ]; then
    echo "Reached cutoff, stopping."
    echo "$RESP" | jq --argjson cutoff "$CUTOFF_UNIX" '.data | map(select(.created_at >= $cutoff))'
    break
  fi

  echo "$RESP" | jq '.data | length'
  CURSOR=$(echo "$RESP" | jq -r '.next_cursor // empty')
  HAS_MORE=$(echo "$RESP" | jq -r '.has_more')
  [ "$HAS_MORE" = "false" ] || [ -z "$CURSOR" ] && break
  sleep 1
done
```

Endpoints that **do** support `before`/`after` (Unix timestamps):

- `/v1/market/aggregates`
- `/v1/market/cumulatives`
- `/v1/market/requestors/:address/aggregates`
- `/v1/market/requestors/:address/cumulatives`
- `/v1/market/provers/:address/aggregates`
- `/v1/market/provers/:address/cumulatives`
- `/v1/market/efficiency/aggregates`
- `/v1/market/efficiency/requests`

Endpoints that require **client-side** time filtering:

- `/v1/market/requests`
- `/v1/market/requestors/:address/requests`
- `/v1/market/provers/:address/requests`
- All staking, PoVW, and delegation endpoints

## Endpoint Reference

For the full list of endpoints, query parameters, and response schemas, read [references/endpoints.md](references/endpoints.md).

### Quick Reference

**Market Indexer endpoints** (L2 chains: Base, Taiko, Base Sepolia):

| Category              | Key Endpoints                                                    |
| --------------------- | ---------------------------------------------------------------- |
| Market status         | `GET /v1/market`                                                 |
| Requests              | `GET /v1/market/requests`, `GET /v1/market/requests/:request_id` |
| Market aggregates     | `GET /v1/market/aggregates?aggregation=daily`                    |
| Market cumulatives    | `GET /v1/market/cumulatives`                                     |
| Requestor leaderboard | `GET /v1/market/requestors?period=7d`                            |
| Prover leaderboard    | `GET /v1/market/provers?period=7d`                               |
| Requestor details     | `GET /v1/market/requestors/:address/requests`                    |
| Prover details        | `GET /v1/market/provers/:address/requests`                       |
| Efficiency            | `GET /v1/market/efficiency`                                      |

**ZKC Indexer endpoints** (Ethereum L1: Eth Mainnet, Eth Sepolia):

| Category            | Key Endpoints                                                                  |
| ------------------- | ------------------------------------------------------------------------------ |
| Staking summary     | `GET /v1/staking`                                                              |
| Staking leaderboard | `GET /v1/staking/addresses`                                                    |
| PoVW summary        | `GET /v1/povw`                                                                 |
| PoVW leaderboard    | `GET /v1/povw/addresses`                                                       |
| Delegations         | `GET /v1/delegations/votes/addresses`, `GET /v1/delegations/rewards/addresses` |

## Common Tasks

### Look up a specific request (market indexer)

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/requests/0xABC123" | jq .
```

### Get recent requests (market indexer)

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/requests?limit=20" | jq .
```

### Get requests for a specific requestor (market indexer)

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/requestors/0xADDRESS/requests?limit=20" | jq .
```

### Get daily market stats for the last week (market indexer)

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/aggregates?aggregation=daily&limit=7" | jq .
```

### Get prover leaderboard for last 24h (market indexer)

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/provers?period=1d" | jq .
```

### Get staking info for an address (ZKC indexer)

```bash
indexer_get "${ZKC_INDEXER_URL}/v1/staking/addresses/0xADDRESS" | jq .
```

### Get PoVW rewards for an address (ZKC indexer)

```bash
indexer_get "${ZKC_INDEXER_URL}/v1/povw/addresses/0xADDRESS" | jq .
```

### Check market efficiency (market indexer)

```bash
indexer_get "${MARKET_INDEXER_URL}/v1/market/efficiency?type=gas_adjusted" | jq .
```

## Error Handling

| HTTP Status | Meaning                                    |
| ----------- | ------------------------------------------ |
| 200         | Success                                    |
| 400         | Bad request (invalid params)               |
| 403         | WAF blocked (rate limited or bad key)      |
| 404         | Endpoint or resource not found             |
| 429         | Rate limited                               |
| 502/503     | Backend/database error (retry after delay) |

Error responses return JSON: `{"error": "...", "message": "..."}`.

## OpenAPI Spec

The full OpenAPI spec is available on any indexer at `/openapi.yaml` or interactively at `/docs` (Swagger UI). Fetch it if you need detailed field-level schema information:

```bash
indexer_get "${MARKET_INDEXER_URL}/openapi.yaml"
indexer_get "${ZKC_INDEXER_URL}/openapi.yaml"
```
