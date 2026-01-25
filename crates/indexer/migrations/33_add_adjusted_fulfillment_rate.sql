ALTER TABLE hourly_requestor_summary
ADD COLUMN IF NOT EXISTS locked_orders_fulfillment_rate_adjusted DOUBLE PRECISION NOT NULL DEFAULT 0.0;

ALTER TABLE daily_requestor_summary
ADD COLUMN IF NOT EXISTS locked_orders_fulfillment_rate_adjusted DOUBLE PRECISION NOT NULL DEFAULT 0.0;

ALTER TABLE weekly_requestor_summary
ADD COLUMN IF NOT EXISTS locked_orders_fulfillment_rate_adjusted DOUBLE PRECISION NOT NULL DEFAULT 0.0;

ALTER TABLE monthly_requestor_summary
ADD COLUMN IF NOT EXISTS locked_orders_fulfillment_rate_adjusted DOUBLE PRECISION NOT NULL DEFAULT 0.0;

ALTER TABLE all_time_requestor_summary
ADD COLUMN IF NOT EXISTS locked_orders_fulfillment_rate_adjusted DOUBLE PRECISION NOT NULL DEFAULT 0.0;
