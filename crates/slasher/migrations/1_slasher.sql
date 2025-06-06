CREATE TABLE IF NOT EXISTS orders (
    id TEXT PRIMARY KEY,
    expires_at BIGINT NOT NULL,
    lock_expires_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_orders_expires_at ON orders(expires_at);
CREATE INDEX IF NOT EXISTS idx_orders_lock_expires_at ON orders(lock_expires_at);

CREATE TABLE IF NOT EXISTS last_block (
    id INTEGER PRIMARY KEY,
    block TEXT
)
