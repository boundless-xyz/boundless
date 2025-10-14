CREATE TABLE IF NOT EXISTS order_stream_state (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE,
    last_processed_timestamp TIMESTAMP WITH TIME ZONE
);

INSERT INTO order_stream_state (id) VALUES (TRUE)
  ON CONFLICT (id) DO NOTHING;
