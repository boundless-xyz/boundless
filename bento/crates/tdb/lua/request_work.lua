-- request_work.lua
-- KEYS: none
-- ARGV[1]: worker_type
-- Returns: {job_id, task_id, task_def, prerequisites, max_retries} or nil
-- Declare Redis globals for linter
---@diagnostic disable-next-line: undefined-global
ARGV = ARGV or {}
---@diagnostic disable-next-line: undefined-global
redis = redis or {}

local worker_type = ARGV[1]

-- Get all streams for this worker type
local stream_ids = redis.call('SMEMBERS', 'streams:by_worker:' .. worker_type)
if #stream_ids == 0 then
    return nil
end

-- Find stream with available work (simple round-robin approach)
local task_key = nil
local timestamp = nil
local selected_stream = nil

for i = 1, #stream_ids do
    local queue_key = 'ready:' .. worker_type .. ':' .. stream_ids[i]
    local task_keys = redis.call('ZRANGE', queue_key, 0, 0, 'WITHSCORES')

    if #task_keys > 0 then
        task_key = task_keys[1]
        timestamp = task_keys[2]
        selected_stream = stream_ids[i]
        break
    end
end

if not task_key then
    return nil
end

-- Remove from ready queue
local queue_key = 'ready:' .. worker_type .. ':' .. selected_stream
redis.call('ZREM', queue_key, task_key)

-- Update task state
local now = redis.call('TIME')
local now_timestamp = now[1] .. '.' .. now[2]

redis.call('HSET', 'task:' .. task_key,
    'state', 'running',
    'started_at', timestamp,
    'updated_at', now_timestamp
)

-- Update stream counters
redis.call('HINCRBY', 'stream:' .. selected_stream, 'ready', -1)
redis.call('HINCRBY', 'stream:' .. selected_stream, 'running', 1)

-- Return task details
local task = redis.call('HGETALL', 'task:' .. task_key)
local task_map = {}
for i = 1, #task, 2 do
    task_map[task[i]] = task[i + 1]
end

return {
    task_map['job_id'],
    task_map['task_id'],
    task_map['task_def'],
    task_map['prerequisites'],
    task_map['max_retries']
}
