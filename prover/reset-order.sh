#!/bin/bash

# This script performs a full reset of a failed order using the ORDER ID. It will:
# 1. Find the corresponding job_id (proof_id) from the SQLite database (broker.db).
# 2. Delete the job and all related taskdb keys from Redis.
# 3. Reset the order's status to 'PendingProving' in the SQLite database.
#
# This script is designed for the Ansible-deployed broker setup.

set -euo pipefail

ORDER_ID_FRAGMENT="${1:-}"
if [ -z "$ORDER_ID_FRAGMENT" ]; then
  echo "Usage: $0 <order_id_fragment>"
  echo "You can provide a partial or full order ID."
  exit 1
fi

BROKER_DB="${BROKER_DB:-/opt/boundless/broker.db}"
REDIS_URL="${TASKDB_REDIS_URL:-${REDIS_URL:-redis://127.0.0.1:6379}}"
TASKDB_NAMESPACE="${TASKDB_NAMESPACE:-taskdb}"

if ! command -v sqlite3 > /dev/null 2>&1; then
  echo "Error: sqlite3 command not found. Please install sqlite3."
  exit 1
fi

if ! command -v redis-cli > /dev/null 2>&1; then
  echo "Error: redis-cli command not found. Please install redis-tools."
  exit 1
fi

if [ ! -f "$BROKER_DB" ]; then
  echo "Error: Broker database not found at: $BROKER_DB"
  echo "Please ensure the broker has been deployed via Ansible, or set BROKER_DB."
  exit 1
fi

echo "--- [Step 1/3] Finding Job ID (proof_id) for Order fragment: ${ORDER_ID_FRAGMENT} ---"

SQLITE_FIND_SQL="SELECT json_extract(data, '\$.proof_id') FROM orders WHERE id LIKE '%${ORDER_ID_FRAGMENT}%';"

if [ -r "$BROKER_DB" ]; then
  JOB_ID=$(sqlite3 "$BROKER_DB" "${SQLITE_FIND_SQL}")
else
  JOB_ID=$(sudo sqlite3 "$BROKER_DB" "${SQLITE_FIND_SQL}")
fi

if [ -z "$JOB_ID" ] || [ "$JOB_ID" == "null" ]; then
  echo "Error: No order found with an ID fragment matching '${ORDER_ID_FRAGMENT}' or the order does not have a proof_id."
  exit 1
fi

echo "Found Job ID: ${JOB_ID}"
echo ""

echo "--- [Step 2/3] Resetting Redis taskdb data for Job ID: ${JOB_ID} ---"

REDIS_LUA=$(cat <<'LUA'
local p = ARGV[1]
local job_id = ARGV[2]

local job_key = p .. ':job:' .. job_id .. ':meta'
local user_id = redis.call('HGET', job_key, 'user_id')
if user_id then
  redis.call('SREM', p .. ':jobs:running:' .. user_id, job_id)
end

local all_tasks = redis.call('SMEMBERS', p .. ':tasks:by_job:' .. job_id)
for _, tid in ipairs(all_tasks) do
  local task_key = p .. ':task:' .. job_id .. ':' .. tid
  local state = redis.call('HGET', task_key, 'state')
  local worker_type = redis.call('HGET', task_key, 'worker_type')
  local priority = redis.call('HGET', task_key, 'priority')
  local compound = job_id .. '|' .. tid

  if state == 'ready' and worker_type and priority then
    redis.call('ZREM', p .. ':ready:' .. worker_type .. ':' .. priority, compound)
  elseif state == 'running' then
    redis.call('ZREM', p .. ':tasks:running', compound)
  end

  local prereqs_json = redis.call('HGET', task_key, 'prerequisites') or '[]'
  local prereqs = cjson.decode(prereqs_json)
  for _, pre in ipairs(prereqs) do
    redis.call('SREM', p .. ':deps:' .. job_id .. ':' .. pre, tid)
  end

  redis.call('DEL', p .. ':deps:' .. job_id .. ':' .. tid)
  redis.call('DEL', task_key)
end

redis.call('DEL', p .. ':tasks:by_job:' .. job_id)
redis.call('DEL', job_key)

return #all_tasks
LUA
)

REDIS_RESULT=$(redis-cli -u "$REDIS_URL" --eval /dev/stdin , "$TASKDB_NAMESPACE" "$JOB_ID" <<<"$REDIS_LUA" 2>&1) || {
  echo "Error: Redis cleanup failed:"
  echo "$REDIS_RESULT"
  exit 1
}

echo "Redis cleanup complete. Removed task entries: ${REDIS_RESULT}"
echo ""

echo "--- [Step 3/3] Resetting SQLite order status to PendingProving ---"

SQLITE_RESET_SQL="
UPDATE orders
SET data = json_set(
               json_set(data, '\$.status', 'PendingProving'),
               '\$.proof_id', NULL
           )
WHERE id LIKE '%${ORDER_ID_FRAGMENT}%';
SELECT 'SQLite: Order status reset for ' || changes() || ' order(s).';
"

if [ -w "$BROKER_DB" ]; then
  SQLITE_RESULT=$(sqlite3 "$BROKER_DB" "${SQLITE_RESET_SQL}")
else
  SQLITE_RESULT=$(sudo sqlite3 "$BROKER_DB" "${SQLITE_RESET_SQL}")
fi

echo "${SQLITE_RESULT}"

echo ""
echo "--- Reset complete. The broker should now pick up the order for proving. ---"
