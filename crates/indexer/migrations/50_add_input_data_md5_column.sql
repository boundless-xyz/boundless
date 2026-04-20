-- Migration 50: Add stored generated column for md5(input_data)
--
-- Fixes sustained CPU load on Aurora reader caused by computing md5(input_data)
-- at query time on every row. The stored column computes the hash once at write
-- time; queries and indexes reference the pre-computed value directly.

-- request_status: used by per-requestor adjusted count queries
ALTER TABLE request_status
    ADD COLUMN input_data_md5 TEXT GENERATED ALWAYS AS (md5(input_data)) STORED;

-- proof_requests: used by batched leaderboard queries that JOIN with request_status
ALTER TABLE proof_requests
    ADD COLUMN input_data_md5 TEXT GENERATED ALWAYS AS (md5(input_data)) STORED;

-- Rebuild indexes to use the stored column instead of the md5() expression.
-- The stored column is a plain TEXT value, so these are regular B-tree indexes.
DROP INDEX IF EXISTS idx_request_status_client_fulfilled_input_image;
CREATE INDEX idx_request_status_client_fulfilled_input_image
    ON request_status (client_address, fulfilled_at, input_data_md5, image_url)
    WHERE fulfilled_at IS NOT NULL
      AND locked_at IS NOT NULL;

DROP INDEX IF EXISTS idx_request_status_client_expired_input_image;
CREATE INDEX idx_request_status_client_expired_input_image
    ON request_status (client_address, expires_at, input_data_md5, image_url)
    WHERE request_status = 'expired'
      AND locked_at IS NOT NULL;
