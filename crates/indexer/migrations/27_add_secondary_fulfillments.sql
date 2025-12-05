-- Add total_secondary_fulfillments column to all market and requestor summary tables
-- A secondary fulfillment occurs when fulfilled_at > lock_end AND fulfilled_at < expires_at AND request_status = 'fulfilled'

ALTER TABLE hourly_market_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE daily_market_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE weekly_market_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE monthly_market_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE all_time_market_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;

ALTER TABLE hourly_requestor_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE daily_requestor_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE weekly_requestor_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE monthly_requestor_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;
ALTER TABLE all_time_requestor_summary ADD COLUMN total_secondary_fulfillments BIGINT NOT NULL DEFAULT 0;

