-- Add base_fee to blocks table for gas cost calculations
ALTER TABLE blocks ADD COLUMN IF NOT EXISTS base_fee TEXT;

-- Add fixed/variable cost breakdown to request_status
ALTER TABLE request_status ADD COLUMN IF NOT EXISTS fixed_cost TEXT;
ALTER TABLE request_status ADD COLUMN IF NOT EXISTS variable_cost_per_cycle TEXT;

-- Add cost breakdown to hourly requestor summary
ALTER TABLE hourly_requestor_summary ADD COLUMN IF NOT EXISTS total_fixed_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE hourly_requestor_summary ADD COLUMN IF NOT EXISTS total_variable_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';

-- Add cost breakdown to daily requestor summary
ALTER TABLE daily_requestor_summary ADD COLUMN IF NOT EXISTS total_fixed_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE daily_requestor_summary ADD COLUMN IF NOT EXISTS total_variable_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';

-- Add cost breakdown to weekly requestor summary
ALTER TABLE weekly_requestor_summary ADD COLUMN IF NOT EXISTS total_fixed_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE weekly_requestor_summary ADD COLUMN IF NOT EXISTS total_variable_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';

-- Add cost breakdown to monthly requestor summary
ALTER TABLE monthly_requestor_summary ADD COLUMN IF NOT EXISTS total_fixed_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE monthly_requestor_summary ADD COLUMN IF NOT EXISTS total_variable_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';

-- Add cost breakdown to epoch requestor summary
ALTER TABLE epoch_requestor_summary ADD COLUMN IF NOT EXISTS total_fixed_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE epoch_requestor_summary ADD COLUMN IF NOT EXISTS total_variable_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';

-- Add cost breakdown to all-time requestor summary
ALTER TABLE all_time_requestor_summary ADD COLUMN IF NOT EXISTS total_fixed_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';
ALTER TABLE all_time_requestor_summary ADD COLUMN IF NOT EXISTS total_variable_cost TEXT NOT NULL DEFAULT '000000000000000000000000000000000000000000000000000000000000000000000000000000';

-- Indexes for p50 fixed/variable cost queries (similar to migration 32 for lock price percentiles)
-- Optimizes: get_requestor_p50_fixed_costs
-- Query pattern: WHERE locked_at >= $1 AND locked_at < $2
--                AND client_address IN (...)
--                AND fixed_cost IS NOT NULL
--                GROUP BY client_address
--                ORDER BY fixed_cost (for PERCENTILE_CONT)
CREATE INDEX IF NOT EXISTS idx_request_status_client_fixed_cost
    ON request_status (client_address, locked_at, fixed_cost)
    WHERE fixed_cost IS NOT NULL AND locked_at IS NOT NULL;

-- Optimizes: get_requestor_p50_variable_costs
-- Query pattern: WHERE locked_at >= $1 AND locked_at < $2
--                AND client_address IN (...)
--                AND variable_cost_per_cycle IS NOT NULL
--                GROUP BY client_address
--                ORDER BY variable_cost_per_cycle (for PERCENTILE_CONT)
CREATE INDEX IF NOT EXISTS idx_request_status_client_variable_cost
    ON request_status (client_address, locked_at, variable_cost_per_cycle)
    WHERE variable_cost_per_cycle IS NOT NULL AND locked_at IS NOT NULL;
