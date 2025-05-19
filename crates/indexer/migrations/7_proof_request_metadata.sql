 -- Tx metadata which led to us adding this proof request to the table. 
 -- Could be a request submitted or request locked (if submitted offchain)
ALTER TABLE proof_requests ADD COLUMN tx_hash TEXT NOT NULL;
ALTER TABLE proof_requests ADD COLUMN block_number BIGINT NOT NULL;
ALTER TABLE proof_requests ADD COLUMN block_timestamp BIGINT NOT NULL;
