-- update_task_retry.lua
-- KEYS: none
-- ARGV[1]: job_id, ARGV[2]: task_id

local job_id = ARGV[1]
local task_id = ARGV[2]

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

if state ~= 'running' then
    return 0
end

local now = redis.call('TIME')
local timestamp = now[1] .. '.' .. now[2]

local retries = tonumber(redis.call('HGET', 'task:' .. task_key, 'retries')) or 0
local max_retries = tonumber(redis.call('HGET', 'task:' .. task_key, 'max_retries')) or 0

retries = retries + 1

if retries > max_retries then
    -- Max retries hit, fail the task
    local error_msg = 'retry max hit'

    -- Update task to failed state
    redis.call('HSET', 'task:' .. task_key,
        'state', 'failed',
        'error', error_msg,
        'progress', 1.0,
        'updated_at', timestamp
    )

    -- Update stream counters
    local stream_id = redis.call('HGET', 'task:' .. task_key, 'stream_id')
    redis.call('HINCRBY', 'stream:' .. stream_id, 'running', -1)

    -- Fail the job
    local job_state = redis.call('HGET', 'job:' .. job_id, 'state')
    if job_state ~= 'failed' then
        redis.call('HSET', 'job:' .. job_id, 'state', 'failed', 'error', error_msg)
        local user_id = redis.call('HGET', 'job:' .. job_id, 'user_id')
        redis.call('SREM', 'user:' .. user_id .. ':jobs:running', job_id)
        redis.call('SADD', 'user:' .. user_id .. ':jobs:failed', job_id)
    end

    return 1
end

-- Reset and requeue
redis.call('HSET', 'task:' .. task_key,
    'state', 'ready',
    'retries', retries,
    'progress', 0.0,
    'error', '',
    'updated_at', timestamp
)

-- Update stream counters and queues
local stream_id = redis.call('HGET', 'task:' .. task_key, 'stream_id')
redis.call('HINCRBY', 'stream:' .. stream_id, 'running', -1)
redis.call('HINCRBY', 'stream:' .. stream_id, 'ready', 1)

local worker_type = redis.call('HGET', 'stream:' .. stream_id, 'worker_type')
local created_at = redis.call('HGET', 'task:' .. task_key, 'created_at')
redis.call('ZADD', 'ready:' .. worker_type .. ':' .. stream_id, created_at, task_key)

return 1
