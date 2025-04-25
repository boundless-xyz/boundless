CREATE TABLE IF NOT EXISTS proof_requests (
    request_digest      TEXT      PRIMARY KEY,
    request_id          TEXT      NOT NULL,
    client_address      TEXT      NOT NULL,  -- Ethereum address
    
    -- Requirements fields
    image_id            TEXT      NOT NULL, -- 32-byte image ID (hex encoded)
    predicate_type      TEXT      NOT NULL, -- Type of predicate (e.g., 'DigestMatch', 'PrefixMatch')
    predicate_data      TEXT      NOT NULL, -- Predicate data (hex encoded)
    callback_address    TEXT,               -- Optional callback contract address
    callback_gas_limit  TEXT,               -- Optional gas limit for callback
    selector            TEXT      NOT NULL, -- Selector (hex encoded)
    
    -- Input fields
    input_type          TEXT      NOT NULL, -- Type of input (e.g., 'Inline', 'Url')
    input_data          TEXT      NOT NULL, -- Input data (hex encoded)
    
    -- Offer fields
    min_price           TEXT      NOT NULL, -- Minimum price in wei
    max_price           TEXT      NOT NULL, -- Maximum price in wei
    lock_stake          TEXT      NOT NULL, -- Lock stake amount in wei
    bidding_start       BIGINT    NOT NULL, -- Unix timestamp when bidding starts
    expires_at          BIGINT    NOT NULL, -- Unix timestamp when request expires
    lock_end            BIGINT    NOT NULL, -- Unix timestamp when lock ends
    ramp_up_period      BIGINT    NOT NULL  -- Ramp up period in seconds  
);

-- Add an index on image_id for faster lookups
CREATE INDEX IF NOT EXISTS idx_proof_requests_image_id ON proof_requests(image_id);

-- Add an index on bidding_end for time-based queries
CREATE INDEX IF NOT EXISTS idx_proof_requests_expires_at ON proof_requests(expires_at);
