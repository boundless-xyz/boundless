-- Drop and recreate the table with the correct column type
-- This is safe because the table was just created in migration 18
DROP TABLE IF EXISTS order_stream_state;

CREATE TABLE IF NOT EXISTS order_stream_state (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE,
    last_processed_timestamp TEXT
);

INSERT INTO order_stream_state (id) VALUES (TRUE)
  ON CONFLICT (id) DO NOTHING;
