-- Index for filtering requests by status sorted by created_at
-- Complements idx_request_status_status_updated which sorts by updated_at
CREATE INDEX IF NOT EXISTS idx_request_status_status_created
    ON request_status (request_status, created_at DESC);
