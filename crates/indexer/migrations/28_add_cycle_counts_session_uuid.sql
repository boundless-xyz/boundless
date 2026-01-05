-- Add a session UUID column to the cycle counts table to track currently executing requests
-- (cycle_status = 'EXECUTING')

ALTER TABLE cycle_counts
    ADD COLUMN session_uuid TEXT;
