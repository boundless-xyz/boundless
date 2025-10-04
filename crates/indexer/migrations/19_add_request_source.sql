ALTER TABLE proof_requests ADD COLUMN source TEXT NOT NULL DEFAULT 'unknown';

CREATE INDEX IF NOT EXISTS idx_proof_requests_source ON proof_requests(source);
