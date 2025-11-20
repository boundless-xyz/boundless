CREATE TABLE IF NOT EXISTS proof_requests (
    request_digest      TEXT      PRIMARY KEY,
    request_id          TEXT      NOT NULL,
    client_address      TEXT      NOT NULL,  -- Ethereum address
    
    -- Requirements fields
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
    lock_collateral          TEXT      NOT NULL, -- Lock collateral amount in wei
    bidding_start       BIGINT    NOT NULL, -- Unix timestamp when bidding starts
    expires_at          BIGINT    NOT NULL, -- Unix timestamp when request expires
    lock_end            BIGINT    NOT NULL, -- Unix timestamp when lock ends
    ramp_up_period      BIGINT    NOT NULL,  -- Ramp up period in seconds

    -- Image fields
    image_id            TEXT      NOT NULL DEFAULT '',
    image_url           TEXT      NOT NULL DEFAULT '',

    -- Tx metadata which led to us adding this proof request to the table.
    -- Could be a request submitted or request locked (if submitted offchain)
    tx_hash             TEXT      NOT NULL,
    block_number        BIGINT    NOT NULL, -- Block number
    block_timestamp     BIGINT    NOT NULL, -- Block timestamp
    
    submission_timestamp BIGINT   NOT NULL  -- Timestamp when request was submitted (block_timestamp for onchain, created_at for offchain)
);

-- Add an index on client_address for faster lookups
CREATE INDEX IF NOT EXISTS idx_proof_requests_client_address ON proof_requests(client_address);

-- Add an index on bidding_end for time-based queries
CREATE INDEX IF NOT EXISTS idx_proof_requests_expires_at ON proof_requests(expires_at);

-- Add an index on submission_timestamp for aggregation queries
CREATE INDEX IF NOT EXISTS idx_proof_requests_submission_timestamp ON proof_requests(submission_timestamp);

-- Composite index for active requestor lookup in aggregation queries
-- Optimizes: get_active_requestor_addresses_in_period
-- Query pattern: WHERE submission_timestamp >= $1 AND submission_timestamp < $2
CREATE INDEX IF NOT EXISTS idx_proof_requests_submission_timestamp_client
    ON proof_requests (submission_timestamp, client_address);
