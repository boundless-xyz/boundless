CREATE TABLE IF NOT EXISTS assessor_receipts (
  tx_hash           TEXT      PRIMARY KEY REFERENCES transactions(tx_hash),
  prover_address    TEXT      NOT NULL,
  seal              TEXT      NOT NULL,
  block_number      BIGINT    NOT NULL,
  block_timestamp   BIGINT    NOT NULL
);

CREATE TABLE IF NOT EXISTS fulfillments (
  request_digest      TEXT      REFERENCES proof_delivered_events(request_digest),
  request_id          TEXT      NOT NULL,
  prover_address      TEXT      NOT NULL,
  image_id            TEXT      NOT NULL,
  journal             TEXT,
  seal                TEXT      NOT NULL,
  tx_hash             TEXT      NOT NULL REFERENCES transactions(tx_hash),
  block_number        BIGINT    NOT NULL,
  block_timestamp     BIGINT    NOT NULL,
  PRIMARY KEY (request_digest, tx_hash)
);

CREATE INDEX IF NOT EXISTS idx_fulfillments_request_id ON fulfillments(request_id);
CREATE INDEX IF NOT EXISTS idx_fulfillments_prover_address ON fulfillments(prover_address);
CREATE INDEX IF NOT EXISTS idx_fulfillments_image_id ON fulfillments(image_id);
