CREATE TABLE orders (
    id BIGSERIAL NOT NULL PRIMARY KEY,
    request_id TEXT NOT NULL,
    request_digest TEXT NOT NULL UNIQUE,
    order_data JSONB NOT NULL
);

ALTER TABLE orders
ADD CONSTRAINT unique_request_digest UNIQUE (request_digest);

CREATE TABLE brokers (
    addr BYTEA NOT NULL PRIMARY KEY,
    nonce TEXT NOT NULL,
    connections INTEGER NOT NULL DEFAULT 0 CHECK (connections >= 0)
)