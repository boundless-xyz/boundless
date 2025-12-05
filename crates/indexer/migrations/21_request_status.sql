CREATE TABLE IF NOT EXISTS request_status (
    -- Identifiers
    request_digest TEXT PRIMARY KEY,
    request_id TEXT NOT NULL,

    -- Status tracking (mutually exclusive)
    request_status TEXT NOT NULL,  -- 'submitted', 'locked', 'fulfilled', 'expired'
    slashed_status TEXT NOT NULL DEFAULT 'N/A',  -- 'N/A', 'Pending', 'Slashed'
    source TEXT NOT NULL,  -- 'onchain', 'offchain', 'unknown'

    -- Parties involved
    client_address TEXT NOT NULL,
    lock_prover_address TEXT,  -- NULL until locked
    fulfill_prover_address TEXT,  -- NULL until fulfilled

    -- Timestamps (for tracking state transitions)
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    locked_at BIGINT,
    fulfilled_at BIGINT,
    slashed_at BIGINT,
    lock_prover_delivered_proof_at BIGINT,  -- timestamp of first ProofDelivered event where prover = lock_prover_address

    -- Block numbers (for blockchain reference)
    submit_block BIGINT,               -- block where request was submitted (NULL for offchain)
    lock_block BIGINT,                 -- block where request was locked
    fulfill_block BIGINT,              -- block where request was fulfilled
    slashed_block BIGINT,              -- block where prover was slashed

    -- Pricing & timing from offer
    min_price TEXT NOT NULL,
    max_price TEXT NOT NULL,
    lock_collateral TEXT NOT NULL,
    ramp_up_start BIGINT NOT NULL,
    ramp_up_period BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    lock_end BIGINT NOT NULL,

    -- Slashing details (NULL unless slashed)
    slash_recipient TEXT,              -- who received the transferred collateral
    slash_transferred_amount TEXT,     -- amount transferred to recipient
    slash_burned_amount TEXT,          -- amount burned

    -- Optional metrics (from fulfillment, NULL until fulfilled)
    program_cycles TEXT,
    total_cycles TEXT,
    peak_prove_mhz BIGINT,
    effective_prove_mhz BIGINT,

    -- Lock pricing (computed when locked)
    lock_price TEXT,
    lock_price_per_cycle TEXT,

    -- Blockchain references (tx hashes)
    submit_tx_hash TEXT,
    lock_tx_hash TEXT,
    fulfill_tx_hash TEXT,
    slash_tx_hash TEXT,

    -- Request specification (for detailed view)
    image_id TEXT NOT NULL,
    image_url TEXT,
    selector TEXT NOT NULL,
    predicate_type TEXT NOT NULL,
    predicate_data TEXT NOT NULL,
    input_type TEXT NOT NULL,
    input_data TEXT NOT NULL,

    -- Fulfillment proof data (NULL until fulfilled)
    fulfill_journal TEXT,
    fulfill_seal TEXT
);

-- Query 1a: List all requests sorted by last updated
CREATE INDEX idx_request_status_updated
    ON request_status (updated_at DESC);

-- Query 1b: List all requests sorted by created_at
CREATE INDEX idx_request_status_created
    ON request_status (created_at DESC);

-- Query 2a: List requests by specific requestor sorted by updated_at
CREATE INDEX idx_request_status_client_updated
    ON request_status (client_address, updated_at DESC);

-- Query 2b: List requests by specific requestor sorted by created_at
CREATE INDEX idx_request_status_client_created
    ON request_status (client_address, created_at DESC);

-- Query 3a: List requests locked by specific prover sorted by updated_at
CREATE INDEX idx_request_status_lock_prover_updated
    ON request_status (lock_prover_address, updated_at DESC)
    WHERE lock_prover_address IS NOT NULL;

-- Query 3b: List requests locked by specific prover sorted by created_at
CREATE INDEX idx_request_status_lock_prover_created
    ON request_status (lock_prover_address, created_at DESC)
    WHERE lock_prover_address IS NOT NULL;

-- Query 3c: List requests fulfilled by specific prover sorted by updated_at
CREATE INDEX idx_request_status_fulfill_prover_updated
    ON request_status (fulfill_prover_address, updated_at DESC)
    WHERE fulfill_prover_address IS NOT NULL;

-- Query 3d: List requests fulfilled by specific prover sorted by created_at
CREATE INDEX idx_request_status_fulfill_prover_created
    ON request_status (fulfill_prover_address, created_at DESC)
    WHERE fulfill_prover_address IS NOT NULL;

-- Additional useful indexes
CREATE INDEX idx_request_status_request_id
    ON request_status (request_id);

CREATE INDEX idx_request_status_status_updated
    ON request_status (request_status, updated_at DESC);

CREATE INDEX idx_request_status_source
    ON request_status (source);

CREATE INDEX idx_request_status_expires
    ON request_status (expires_at)
    WHERE request_status IN ('submitted', 'locked');
