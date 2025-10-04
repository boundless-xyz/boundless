-- create_job.lua
-- KEYS: none
-- ARGV[1]: stream_id, ARGV[2]: task_def (JSON), ARGV[3]: max_retries,
-- ARGV[4]: timeout_secs, ARGV[5]: user_id
-- Returns: job_id

local stream_id = ARGV[1]
local task_def = ARGV[2]
local max_retries = tonumber(ARGV[3])
local timeout_secs = tonumber(ARGV[4])
local user_id = ARGV[5]

-- Validate inputs
if not stream_id or not user_id then
    return { err = 'MissingRequiredFields' }
end

if not max_retries or max_retries < 0 then
    return { err = 'InvalidMaxRetries' }
end

if not timeout_secs or timeout_secs <= 0 then
    return { err = 'InvalidTimeoutSecs' }
end

-- Verify stream exists
if redis.call('EXISTS', 'stream:' .. stream_id) == 0 then
    return { err = 'StreamNotFound' }
end

-- Generate job_id
local job_id = redis.call('INCR', 'uuid_counter')
job_id = string.format('job:%020d', job_id)

local now = redis.call('TIME')
local timestamp = now[1] .. '.' .. now[2]

-- Create job
redis.call('HSET', 'job:' .. job_id,
    'state', 'running',
    'user_id', user_id,
    'stream_id', stream_id,
    'created_at', timestamp,
    'unresolved', 1
)

-- Create init task
local task_key = job_id .. ':init'
redis.call('HSET', 'task:' .. task_key,
    'job_id', job_id,
    'task_id', 'init',
    'stream_id', stream_id,
    'task_def', task_def,
    'prerequisites', '[]',
    'state', 'ready',
    'waiting_on', 0,
    'progress', 0.0,
    'retries', 0,
    'max_retries', max_retries,
    'timeout_secs', timeout_secs,
    'created_at', timestamp
)

-- Add to ready queue with priority (timestamp for FIFO)
local worker_type = redis.call('HGET', 'stream:' .. stream_id, 'worker_type')
redis.call('ZADD', 'ready:' .. worker_type .. ':' .. stream_id, timestamp, task_key)

-- Update stream counters
redis.call('HINCRBY', 'stream:' .. stream_id, 'ready', 1)

-- Track job
redis.call('SADD', 'user:' .. user_id .. ':jobs:running', job_id)

return job_id
