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
