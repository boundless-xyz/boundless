-- Migration 49: Fix expired index to use md5(input_data) like fulfilled index (migration 48)
DROP INDEX IF EXISTS idx_request_status_client_expired_input_image;

CREATE INDEX idx_request_status_client_expired_input_image
    ON request_status (client_address, expires_at, md5(input_data), image_url)
    WHERE request_status = 'expired'
      AND locked_at IS NOT NULL;
