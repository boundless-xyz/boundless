CREATE TABLE orders (
    id TEXT PRIMARY KEY,
    data JSONB
);

CREATE TABLE batches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    data JSONB
);

CREATE TABLE last_block (
    id INTEGER PRIMARY KEY,
    block TEXT
)