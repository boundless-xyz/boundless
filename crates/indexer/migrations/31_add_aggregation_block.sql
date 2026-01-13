-- Migration to add aggregation block tracking
-- Uses id=1 to track last processed aggregation block (id=0 is for main processing)
INSERT INTO last_block (id, block) 
VALUES (1, '0')
ON CONFLICT (id) DO NOTHING;
