CREATE TABLE orders (
    id TEXT PRIMARY KEY,
    data JSONB
);

CREATE TABLE fulfilled_requests (
    id TEXT PRIMARY KEY,
    block_timestamp INTEGER,
    block_number INTEGER
);

CREATE TABLE locked_requests (
    id TEXT PRIMARY KEY,
    locker TEXT,
    block_number INTEGER,
    block_timestamp INTEGER
);

CREATE TABLE batches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    data JSONB
);

CREATE TABLE last_block (
    id INTEGER PRIMARY KEY,
    block TEXT
)