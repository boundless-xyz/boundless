-- Migration 48: Optimize indexing params too big
-- Hash the input_data to reduce the size of the index preventing index bloat
DROP INDEX IF EXISTS idx_request_status_client_fulfilled_input_image;

CREATE INDEX idx_request_status_client_fulfilled_input_image
    ON request_status (client_address, fulfilled_at, md5(input_data), image_url)
    WHERE fulfilled_at IS NOT NULL
      AND locked_at IS NOT NULL;