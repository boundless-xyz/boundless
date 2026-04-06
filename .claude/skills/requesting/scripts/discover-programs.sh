#!/usr/bin/env bash
# Discover recently fulfilled proof requests from the Boundless indexer API.
# Finds the smallest programs with accessible IPFS URLs and their original inputs.
#
# Usage: bash scripts/discover-programs.sh
# Output: JSON array of up to 5 verified requests, sorted by cycle count (smallest first)
#
# Dependencies: curl, python3
set -euo pipefail

INDEXER_URL="https://d2mdvlnmyov1e1.cloudfront.net/v1/market/requests?limit=500"
MAX_AGE_HOURS=12

RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
NC='\033[0m'

# Check dependencies
for cmd in curl python3; do
    if ! command -v "$cmd" &>/dev/null; then
        printf "${RED}Error:${NC} %s is required but not found.\n" "$cmd" >&2
        exit 1
    fi
done

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

printf "${BOLD}Querying Boundless indexer for recently fulfilled requests...${NC}\n" >&2

# Fetch requests from the indexer
curl -s --fail --max-time 15 "$INDEXER_URL" > "$tmpdir/response.json" || {
    printf "${RED}Error:${NC} Failed to fetch from indexer API.\n" >&2
    exit 1
}

# Filter and sort using Python
python3 -c '
import json, sys, time
from datetime import datetime, timezone, timedelta

with open(sys.argv[1]) as f:
    data = json.load(f)

requests = data if isinstance(data, list) else data.get("data", data.get("requests", []))
cutoff_ts = time.time() - (int(sys.argv[3]) * 3600)
results = []

for r in requests:
    if r.get("request_status") != "fulfilled":
        continue
    url = r.get("image_url", "") or ""
    if "/ipfs/" not in url:
        continue
    cycles = r.get("program_cycles")
    if cycles is None:
        continue
    fulfilled_at = r.get("fulfilled_at")
    if not fulfilled_at:
        continue
    # fulfilled_at is a Unix timestamp (integer)
    if isinstance(fulfilled_at, (int, float)) and fulfilled_at < cutoff_ts:
        continue
    input_data = r.get("input_data")
    if not input_data:
        continue
    # Use the ISO string for display if available
    fulfilled_iso = r.get("fulfilled_at_iso", "")
    results.append({
        "cycles": int(cycles),
        "price": r.get("lock_price_formatted", "unknown"),
        "image_id": r.get("image_id", ""),
        "image_url": url,
        "input_data": input_data,
        "input_type": r.get("input_type", ""),
        "fulfilled_at": fulfilled_iso or str(fulfilled_at),
        "fulfilled_ts": int(fulfilled_at) if isinstance(fulfilled_at, (int, float)) else 0,
    })

results.sort(key=lambda x: x["cycles"])
with open(sys.argv[2], "w") as f:
    json.dump(results[:20], f)
' "$tmpdir/response.json" "$tmpdir/candidates.json" "$MAX_AGE_HOURS"

candidate_count=$(python3 -c "import json,sys; data=json.load(open(sys.argv[1])); print(len(data))" "$tmpdir/candidates.json")

if [[ "$candidate_count" -eq 0 ]]; then
    printf "${RED}No recently fulfilled requests found with IPFS URLs.${NC}\n" >&2
    printf "This can happen if no proofs were fulfilled in the last %d hours,\n" "$MAX_AGE_HOURS" >&2
    printf "or if all recent programs used non-IPFS storage.\n" >&2
    exit 1
fi

printf "Found ${BOLD}%s${NC} candidates. Verifying IPFS accessibility...\n" "$candidate_count" >&2

# Verify IPFS URLs are accessible and collect verified results
python3 -c '
import json, sys, subprocess

with open(sys.argv[1]) as f:
    candidates = json.load(f)

verified = []
for c in candidates:
    if len(verified) >= 5:
        break
    url = c["image_url"]
    try:
        result = subprocess.run(
            ["curl", "-s", "-o", "/dev/null", "-w", "%{http_code}", "--head",
             "--max-time", "5", url],
            capture_output=True, text=True, timeout=10
        )
        status = result.stdout.strip()
        if status == "200":
            verified.append(c)
            print(f"  ✓ {url[:80]}...", file=sys.stderr)
        else:
            print(f"  ✗ {url[:80]}... (HTTP {status})", file=sys.stderr)
    except Exception as e:
        print(f"  ✗ {url[:80]}... ({e})", file=sys.stderr)

with open(sys.argv[2], "w") as f:
    json.dump(verified, f)
' "$tmpdir/candidates.json" "$tmpdir/verified.json"

verified_count=$(python3 -c "import json,sys; data=json.load(open(sys.argv[1])); print(len(data))" "$tmpdir/verified.json")

if [[ "$verified_count" -eq 0 ]]; then
    printf "\n${RED}No verified IPFS URLs found.${NC} All candidate URLs were unreachable.\n" >&2
    exit 1
fi

printf "\n${GREEN}${BOLD}%s verified request(s) ready:${NC}\n\n" "$verified_count" >&2

# Pretty-print summary to stderr
python3 -c '
import json, sys, time

with open(sys.argv[1]) as f:
    results = json.load(f)

for i, r in enumerate(results, 1):
    fulfilled_ts = r.get("fulfilled_ts", 0)
    if fulfilled_ts:
        ago_secs = time.time() - fulfilled_ts
        hours = int(ago_secs / 3600)
        mins = int((ago_secs % 3600) / 60)
        time_str = "%dh %dm ago" % (hours, mins)
    else:
        time_str = r.get("fulfilled_at", "unknown")
    price = r["price"]
    if price.endswith(" ETH"):
        price = price[:-4]
    cycles = r["cycles"]
    image_id = r["image_id"][:16]
    url = r["image_url"]
    print("  [%d] %s cycles | %s ETH | fulfilled %s" % (i, format(cycles, ","), price, time_str), file=sys.stderr)
    print("      Image ID: %s..." % image_id, file=sys.stderr)
    print("      URL: %s" % url, file=sys.stderr)
    print(file=sys.stderr)
' "$tmpdir/verified.json"

# Output the verified results as JSON to stdout
cat "$tmpdir/verified.json"
