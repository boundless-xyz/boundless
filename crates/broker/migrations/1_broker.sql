CREATE TABLE orders (
    id TEXT PRIMARY KEY,
    data JSONB
);

CREATE TABLE fulfilled_orders (
    id TEXT PRIMARY KEY,
    block_timestamp INTEGER,
    block_number INTEGER
);

CREATE TABLE batches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    data JSONB
);

CREATE TABLE last_block (
    id INTEGER PRIMARY KEY,
    block TEXT
)