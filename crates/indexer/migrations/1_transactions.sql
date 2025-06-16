CREATE TABLE IF NOT EXISTS last_block (
    id INTEGER PRIMARY KEY,
    block TEXT
);

CREATE TABLE IF NOT EXISTS transactions (
  tx_hash         TEXT      PRIMARY KEY,
  block_number    BIGINT    NOT NULL,
  from_address    TEXT      NOT NULL,
  block_timestamp BIGINT    NOT NULL
);
