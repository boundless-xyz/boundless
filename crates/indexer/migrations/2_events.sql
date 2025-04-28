CREATE TABLE IF NOT EXISTS request_submitted_events (
  request_digest    TEXT        PRIMARY KEY,
  request_id        TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL
);

CREATE INDEX IF NOT EXISTS request_submitted_events_request_id_idx
  ON request_submitted_events (request_id);

CREATE TABLE request_locked_events (
  request_digest    TEXT        PRIMARY KEY,
  request_id        TEXT        NOT NULL,
  prover_address    TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL
);

CREATE INDEX IF NOT EXISTS request_locked_events_request_id_idx
  ON request_locked_events (request_id);

CREATE TABLE IF NOT EXISTS request_fulfilled_events (
  request_digest    TEXT        PRIMARY KEY,
  request_id        TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL
);

CREATE INDEX IF NOT EXISTS request_fulfilled_events_request_id_idx
  ON request_fulfilled_events (request_id);

CREATE TABLE IF NOT EXISTS proof_delivered_events (
  request_digest    TEXT        PRIMARY KEY,
  request_id        TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL
);

CREATE INDEX IF NOT EXISTS proof_delivered_events_request_id_idx
  ON proof_delivered_events (request_id);

CREATE TABLE IF NOT EXISTS callback_failed_events (
  request_id        TEXT        NOT NULL,
  callback_address  TEXT        NOT NULL,
  error_data        BYTEA       NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL,
  PRIMARY KEY (request_id, tx_hash)
);

CREATE TABLE IF NOT EXISTS prover_slashed_events (
  request_id        TEXT        PRIMARY KEY,
  prover_address    TEXT        NOT NULL,
  burn_value        TEXT        NOT NULL,
  transfer_value    TEXT        NOT NULL,
  stake_recipient   TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL
);

CREATE INDEX IF NOT EXISTS prover_slashed_events_prover_address_idx
  ON prover_slashed_events (prover_address);

CREATE TABLE IF NOT EXISTS deposit_events (
  account           TEXT        NOT NULL,
  value             TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL,
  PRIMARY KEY (account, tx_hash)
);

CREATE TABLE IF NOT EXISTS withdrawal_events (
  account           TEXT        NOT NULL,
  value             TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL,
  PRIMARY KEY (account, tx_hash)
);

CREATE TABLE IF NOT EXISTS stake_deposit_events (
  account           TEXT        NOT NULL,
  value             TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL,
  PRIMARY KEY (account, tx_hash)
);

CREATE TABLE IF NOT EXISTS stake_withdrawal_events (
  account           TEXT        NOT NULL,
  value             TEXT        NOT NULL,
  tx_hash           TEXT        NOT NULL REFERENCES transactions(tx_hash),
  block_number      BIGINT      NOT NULL,
  block_timestamp   BIGINT      NOT NULL,
  PRIMARY KEY (account, tx_hash)
);
