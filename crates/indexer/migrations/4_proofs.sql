CREATE TABLE IF NOT EXISTS assessor_receipts (
  tx_hash           TEXT      PRIMARY KEY REFERENCES transactions(tx_hash),
  prover_address    TEXT      NOT NULL,
  seal              TEXT      NOT NULL,
  block_number      BIGINT    NOT NULL,
  block_timestamp   BIGINT    NOT NULL
);

CREATE TABLE IF NOT EXISTS proofs (
  request_digest         TEXT      NOT NULL,
  request_id             TEXT      NOT NULL,
  prover_address         TEXT      NOT NULL,
  claim_digest           TEXT      NOT NULL,
  fulfillment_data_type  TEXT      NOT NULL,
  fulfillment_data       TEXT,

  seal                TEXT      NOT NULL,
  tx_hash             TEXT      NOT NULL REFERENCES transactions(tx_hash),
  transaction_index   BIGINT,
  block_number        BIGINT    NOT NULL,
  block_timestamp     BIGINT    NOT NULL,
  PRIMARY KEY (request_digest, tx_hash)
);

CREATE INDEX IF NOT EXISTS idx_proofs_request_id ON proofs(request_id);
CREATE INDEX IF NOT EXISTS idx_proofs_prover_address ON proofs(prover_address);
