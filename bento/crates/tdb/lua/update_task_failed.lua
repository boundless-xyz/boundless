-- update_task_failed.lua
-- KEYS: none
-- ARGV[1]: job_id, ARGV[2]: task_id, ARGV[3]: error

-- Declare Redis globals for linter
---@diagnostic disable-next-line: undefined-global
ARGV = ARGV or {}
---@diagnostic disable-next-line: undefined-global
redis = redis or {}

local job_id = ARGV[1]
local task_id = ARGV[2]
local error_msg = ARGV[3]

-- Validate inputs
if not job_id or not task_id then
    return 0
end

local task_key = job_id .. ':' .. task_id
local state = redis.call('HGET', 'task:' .. task_key, 'state')

-- Check if task exists and belongs to the job
if not state then
    return 0
end

if state ~= 'ready' and state ~= 'running' and state ~= 'pending' then
    return 0
end

local now = redis.call('TIME')
local timestamp = now[1] .. '.' .. now[2]

-- Update task
redis.call('HSET', 'task:' .. task_key,
    'state', 'failed',
    'error', error_msg,
    'progress', 1.0,
    'updated_at', timestamp
)

-- Update stream counters
local stream_id = redis.call('HGET', 'task:' .. task_key, 'stream_id')
if state == 'running' then
    redis.call('HINCRBY', 'stream:' .. stream_id, 'running', -1)
elseif state == 'ready' then
    redis.call('HINCRBY', 'stream:' .. stream_id, 'ready', -1)
    local worker_type = redis.call('HGET', 'stream:' .. stream_id, 'worker_type')
    redis.call('ZREM', 'ready:' .. worker_type .. ':' .. stream_id, task_key)
end

-- Fail the job
local job_state = redis.call('HGET', 'job:' .. job_id, 'state')
if job_state ~= 'failed' then
    redis.call('HSET', 'job:' .. job_id, 'state', 'failed', 'error', error_msg)
    local user_id = redis.call('HGET', 'job:' .. job_id, 'user_id')
    redis.call('SREM', 'user:' .. user_id .. ':jobs:running', job_id)
    redis.call('SADD', 'user:' .. user_id .. ':jobs:failed', job_id)
end

return 1
