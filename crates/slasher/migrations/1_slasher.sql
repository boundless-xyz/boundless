CREATE TABLE orders (
    id TEXT PRIMARY KEY,
    expires_at BIGINT NOT NULL
);

CREATE TABLE last_block (
    id INTEGER PRIMARY KEY,
    block TEXT
)