-- create_stream.lua
-- KEYS: none
-- ARGV[1]: worker_type, ARGV[2]: reserved, ARGV[3]: be_mult, ARGV[4]: user_id
-- Returns: stream_id (UUID)

-- Generate a simple UUID-like string using counter and timestamp
local stream_id = redis.call('INCR', 'stream_counter')
local timestamp = redis.call('TIME')[1]
local combined = tostring(stream_id) .. tostring(timestamp)
-- Pad to 32 characters with zeros
while string.len(combined) < 32 do
    combined = '0' .. combined
end
stream_id = string.sub(combined, 1, 32)

local worker_type = ARGV[1]
local reserved = tonumber(ARGV[2])
local be_mult = tonumber(ARGV[3])
local user_id = ARGV[4]

-- Validate inputs
if not worker_type or not user_id then
    return { err = 'MissingRequiredFields' }
end

if not reserved or reserved < 0 then
    return { err = 'InvalidReserved' }
end

if not be_mult or be_mult <= 0.0 then
    return { err = 'InvalidBeMult' }
end

-- Store stream metadata
redis.call('HSET', 'stream:' .. stream_id,
    'worker_type', worker_type,
    'reserved', reserved,
    'be_mult', be_mult,
    'user_id', user_id,
    'running', 0,
    'ready', 0
)

-- Index by worker_type for lookup
redis.call('SADD', 'streams:by_worker:' .. worker_type, stream_id)

-- Add to user's streams
redis.call('SADD', 'user:' .. user_id .. ':streams', stream_id)

return stream_id
