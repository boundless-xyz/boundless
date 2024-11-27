CREATE TABLE orders (
    id BIGSERIAL NOT NULL PRIMARY KEY,
    order_data JSONB NOT NULL
);

CREATE TABLE brokers (
    addr BYTEA NOT NULL PRIMARY KEY,
    nonce TEXT NOT NULL,
    connections INTEGER NOT NULL DEFAULT 0 CHECK (connections >= 0)
)