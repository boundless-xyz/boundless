CREATE INDEX idx_orders_status ON orders(data->>'status');
