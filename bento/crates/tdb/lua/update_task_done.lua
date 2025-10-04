-- update_task_done.lua
-- KEYS: none
-- ARGV[1]: job_id, ARGV[2]: task_id, ARGV[3]: output (JSON)
-- Returns: boolean (success)

-- Declare Redis globals for linter
---@diagnostic disable-next-line: undefined-global
ARGV = ARGV or {}
---@diagnostic disable-next-line: undefined-global
redis = redis or {}

local job_id = ARGV[1]
local task_id = ARGV[2]
local output = ARGV[3]

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

if state ~= 'ready' and state ~= 'running' then
    return 0
end

local now = redis.call('TIME')
local timestamp = now[1] .. '.' .. now[2]

-- Update task
redis.call('HSET', 'task:' .. task_key,
    'state', 'done',
    'output', output,
    'updated_at', timestamp,
    'progress', 1.0
)

local stream_id = redis.call('HGET', 'task:' .. task_key, 'stream_id')

-- Update stream counters
if state == 'running' then
    redis.call('HINCRBY', 'stream:' .. stream_id, 'running', -1)
elseif state == 'ready' then
    redis.call('HINCRBY', 'stream:' .. stream_id, 'ready', -1)
    -- Remove from ready queue if still there
    local worker_type = redis.call('HGET', 'stream:' .. stream_id, 'worker_type')
    redis.call('ZREM', 'ready:' .. worker_type .. ':' .. stream_id, task_key)
end

-- Process dependents
local dependents = redis.call('SMEMBERS', 'task:' .. task_key .. ':dependents')
for _, dep_key in ipairs(dependents) do
    local waiting = redis.call('HINCRBY', 'task:' .. dep_key, 'waiting_on', -1)

    if waiting == 0 then
        -- Move to ready
        redis.call('HSET', 'task:' .. dep_key, 'state', 'ready')
        local dep_stream = redis.call('HGET', 'task:' .. dep_key, 'stream_id')
        local dep_created = redis.call('HGET', 'task:' .. dep_key, 'created_at')
        local dep_worker = redis.call('HGET', 'stream:' .. dep_stream, 'worker_type')
        redis.call('ZADD', 'ready:' .. dep_worker .. ':' .. dep_stream, dep_created, dep_key)
        redis.call('HINCRBY', 'stream:' .. dep_stream, 'ready', 1)
    end
end

-- Check if job is complete
redis.call('HINCRBY', 'job:' .. job_id, 'unresolved', -1)
local unresolved = tonumber(redis.call('HGET', 'job:' .. job_id, 'unresolved'))

if unresolved == 0 then
    redis.call('HSET', 'job:' .. job_id, 'state', 'done')
    local user_id = redis.call('HGET', 'job:' .. job_id, 'user_id')
    redis.call('SREM', 'user:' .. user_id .. ':jobs:running', job_id)
    redis.call('SADD', 'user:' .. user_id .. ':jobs:done', job_id)
end

return 1
