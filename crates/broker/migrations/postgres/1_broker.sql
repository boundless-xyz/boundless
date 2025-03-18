-- Postgres requires us to create the enum before using it as a column type
-- as it does not store enums as strings internally as sqlite does
CREATE TYPE order_status AS ENUM (
    'New',
    'Pricing',
    'Locking',
    'Locked',
    'Proving',
    'PendingAgg',
    'Aggregating',
    'PendingSubmission',
    'Done',
    'Failed',
    'Skipped'
);

CREATE TYPE batch_status AS ENUM (
    'Aggregating',
    'PendingCompression',
    'Complete',
    'PendingSubmission',
    'Submitted',
    'Failed'
);

CREATE TABLE orders (
    id TEXT PRIMARY KEY,
    data JSONB
);

CREATE TABLE batches (
    id BIGSERIAL PRIMARY KEY,
    data JSONB
);

CREATE TABLE last_block (
    id INTEGER PRIMARY KEY,
    block TEXT
)
