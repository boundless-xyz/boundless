-- Add image_id and image_url columns to proof_requests table to match request_status schema
-- Note: image_id doesn't exist on ProofRequest in Solidity (only imageUrl exists)
-- It only exists on FulfillmentData. Setting as empty string for now.
ALTER TABLE proof_requests ADD COLUMN image_id TEXT NOT NULL DEFAULT '';
ALTER TABLE proof_requests ADD COLUMN image_url TEXT;
