-- Add DOUBLE PRECISION columns for prove_mhz fields. Previous columns were mistakenly BIGINT.

-- Request status table
ALTER TABLE request_status ADD COLUMN effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE request_status ADD COLUMN peak_prove_mhz_v2 DOUBLE PRECISION;

-- Market summary tables
ALTER TABLE hourly_market_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE hourly_market_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE daily_market_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE daily_market_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE weekly_market_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE weekly_market_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE monthly_market_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE monthly_market_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE all_time_market_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE all_time_market_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

-- Requestor summary tables
ALTER TABLE hourly_requestor_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE hourly_requestor_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE daily_requestor_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE daily_requestor_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE weekly_requestor_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE weekly_requestor_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE monthly_requestor_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE monthly_requestor_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

ALTER TABLE all_time_requestor_summary ADD COLUMN best_effective_prove_mhz_v2 DOUBLE PRECISION;
ALTER TABLE all_time_requestor_summary ADD COLUMN best_peak_prove_mhz_v2 DOUBLE PRECISION;

