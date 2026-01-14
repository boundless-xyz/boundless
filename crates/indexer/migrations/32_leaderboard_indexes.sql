-- Migration 32: Leaderboard Query Indexes
-- Optimizes the median lock price and last activity queries for the requestor/prover leaderboard endpoints

-- For requestor median lock price computation
-- Optimizes: get_requestor_median_lock_prices
-- Query pattern: WHERE locked_at >= $1 AND locked_at < $2 
--                AND client_address IN (...) 
--                AND lock_price_per_cycle IS NOT NULL
--                GROUP BY client_address
--                ORDER BY lock_price_per_cycle (for PERCENTILE_CONT)
-- Partial index excludes rows without lock_price_per_cycle
CREATE INDEX IF NOT EXISTS idx_request_status_client_locked_pricing
    ON request_status (client_address, locked_at, lock_price_per_cycle)
    WHERE lock_price_per_cycle IS NOT NULL AND locked_at IS NOT NULL;

-- For prover median lock price computation  
-- Optimizes: get_prover_median_lock_prices
-- Query pattern: WHERE locked_at >= $1 AND locked_at < $2
--                AND lock_prover_address IN (...)
--                AND lock_price_per_cycle IS NOT NULL
--                GROUP BY lock_prover_address
--                ORDER BY lock_price_per_cycle (for PERCENTILE_CONT)
CREATE INDEX IF NOT EXISTS idx_request_status_prover_locked_pricing
    ON request_status (lock_prover_address, locked_at, lock_price_per_cycle)
    WHERE lock_price_per_cycle IS NOT NULL AND lock_prover_address IS NOT NULL;
