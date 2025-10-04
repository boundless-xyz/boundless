-- create_task.lua
-- KEYS: none
-- ARGV[1]: job_id, ARGV[2]: task_id, ARGV[3]: stream_id, ARGV[4]: task_def,
-- ARGV[5]: prerequisites (JSON array), ARGV[6]: max_retries, ARGV[7]: timeout_secs
-- Declare Redis globals for linter
---@diagnostic disable-next-line: undefined-global
ARGV = ARGV or {}
---@diagnostic disable-next-line: undefined-global
redis = redis or {}

local job_id = ARGV[1]
local task_id = ARGV[2]
local stream_id = ARGV[3]
local task_def = ARGV[4]
local prerequisites_str = ARGV[5]
local max_retries = tonumber(ARGV[6])
local timeout_secs = tonumber(ARGV[7])

-- Validate inputs
if not job_id or not task_id or not stream_id or not task_def then
    return { err = 'MissingRequiredFields' }
end

if not max_retries or max_retries < 0 then
    return { err = 'InvalidMaxRetries' }
end

if not timeout_secs or timeout_secs <= 0 then
    return { err = 'InvalidTimeoutSecs' }
end

-- Parse prerequisites with error handling
local prerequisites
local success, result = pcall(cjson.decode, prerequisites_str)
if not success then
    return { err = 'InvalidPrerequisites' }
end
prerequisites = result

local now = redis.call('TIME')
local timestamp = now[1] .. '.' .. now[2]

local task_key = job_id .. ':' .. task_id

-- Count how many prerequisites are not done
local waiting_on = 0
for _, prereq_id in ipairs(prerequisites) do
    local prereq_key = job_id .. ':' .. prereq_id
    local prereq_state = redis.call('HGET', 'task:' .. prereq_key, 'state')
    if prereq_state ~= 'done' then
        waiting_on = waiting_on + 1
        -- Store dependency
        redis.call('SADD', 'task:' .. prereq_key .. ':dependents', task_key)
    end
end

local state = waiting_on == 0 and 'ready' or 'pending'

-- Create task
redis.call('HSET', 'task:' .. task_key,
    'job_id', job_id,
    'task_id', task_id,
    'stream_id', stream_id,
    'task_def', task_def,
    'prerequisites', ARGV[5],
    'state', state,
    'waiting_on', waiting_on,
    'progress', 0.0,
    'retries', 0,
    'max_retries', max_retries,
    'timeout_secs', timeout_secs,
    'created_at', timestamp
)

-- If ready, add to queue
if state == 'ready' then
    local worker_type = redis.call('HGET', 'stream:' .. stream_id, 'worker_type')
    redis.call('ZADD', 'ready:' .. worker_type .. ':' .. stream_id, timestamp, task_key)
    redis.call('HINCRBY', 'stream:' .. stream_id, 'ready', 1)
end

-- Track task in job
redis.call('SADD', 'job:' .. job_id .. ':tasks', task_key)
redis.call('HINCRBY', 'job:' .. job_id, 'unresolved', 1)

return 'OK'
