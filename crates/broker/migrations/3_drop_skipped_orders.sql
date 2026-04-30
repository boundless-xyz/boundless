-- Skipped orders are no longer persisted: the broker stopped reading them
-- ages ago, and accumulated rows have caused multi-hundred-thousand-row
-- bloat that slows json_set UPDATE statements on busy provers.
DELETE FROM orders WHERE data->>'status' = 'Skipped';
