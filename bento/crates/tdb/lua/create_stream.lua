-- create_stream.lua
-- KEYS: none
-- ARGV[1]: worker_type
-- Returns: stream_id (simple identifier)

local worker_type = ARGV[1]

-- Validate inputs
if not worker_type then
    return { err = 'MissingRequiredFields' }
end

-- Use worker_type as the stream_id for simplicity
local stream_id = worker_type

-- Check if stream already exists
local exists = redis.call('EXISTS', 'stream:' .. stream_id)
if exists == 1 then
    return stream_id
end

-- Create stream metadata
redis.call('HSET', 'stream:' .. stream_id,
    'worker_type', worker_type,
    'running', 0,
    'ready', 0
)

-- Index by worker_type for lookup
redis.call('SADD', 'streams:by_worker:' .. worker_type, stream_id)

return stream_id
