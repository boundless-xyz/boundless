// Copyright 2026 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::U256;
use anyhow::Result;
use boundless_market::contracts::pricing::price_at_time;

use crate::db::efficiency::{
    EfficiencyDbImpl, EfficiencyDbObj, MarketEfficiencyDaily, MarketEfficiencyHourly,
    MarketEfficiencyOrder, MoreProfitableSample, RequestForEfficiency,
};
use crate::market::time_boundaries::{get_day_start, get_hour_start};

const SECONDS_PER_DAY: u64 = 86400;

#[derive(Clone)]
pub struct MarketEfficiencyServiceConfig {
    pub interval: Duration,
    pub lookback_days: u64,
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
}

pub struct MarketEfficiencyService {
    db: EfficiencyDbObj,
    config: MarketEfficiencyServiceConfig,
}

impl MarketEfficiencyService {
    pub async fn new(db_conn: &str, config: MarketEfficiencyServiceConfig) -> Result<Self> {
        let db: EfficiencyDbObj =
            Arc::new(EfficiencyDbImpl::new(db_conn, Some(Duration::from_secs(30)), false).await?);
        Ok(Self { db, config })
    }

    pub async fn run(&mut self) -> Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();

        let lookback_seconds = self.config.lookback_days * SECONDS_PER_DAY;
        let from_timestamp = self.config.start_time.unwrap_or(now.saturating_sub(lookback_seconds));
        let to_timestamp = self.config.end_time.unwrap_or(now);

        tracing::info!(
            "Starting market efficiency analysis for time range {} to {}",
            from_timestamp,
            to_timestamp
        );

        // Step 1: Load all requests from the time range into memory
        let all_requests =
            self.db.get_requests_for_efficiency(from_timestamp, to_timestamp).await?;
        tracing::info!("Loaded {} requests from database", all_requests.len());

        // Step 2: Filter to fulfilled requests with known cycles
        let fulfilled_requests: Vec<&RequestForEfficiency> = all_requests
            .iter()
            .filter(|r| {
                r.fulfilled_at.is_some()
                    && r.program_cycles.is_some()
                    && r.locked_at.is_some()
                    && r.lock_price_per_cycle.is_some()
            })
            .collect();

        tracing::info!(
            "Found {} fulfilled requests with known cycles for analysis",
            fulfilled_requests.len()
        );

        if fulfilled_requests.is_empty() {
            tracing::info!("No requests to analyze, skipping");
            return Ok(());
        }

        // Step 3: Compute efficiency for each fulfilled request
        let efficiency_orders = self.compute_efficiency_orders(&fulfilled_requests, &all_requests);
        tracing::info!("Computed efficiency for {} orders", efficiency_orders.len());

        // Step 4: Store efficiency orders
        self.db.upsert_market_efficiency_orders(&efficiency_orders).await?;
        tracing::info!("Stored {} efficiency orders", efficiency_orders.len());

        // Step 5: Aggregate into hourly summaries
        let hourly_summaries = self.aggregate_hourly(&efficiency_orders);
        self.db.upsert_market_efficiency_hourly(&hourly_summaries).await?;
        tracing::info!("Stored {} hourly summaries", hourly_summaries.len());

        // Step 6: Aggregate into daily summaries
        let daily_summaries = self.aggregate_daily(&efficiency_orders);
        self.db.upsert_market_efficiency_daily(&daily_summaries).await?;
        tracing::info!("Stored {} daily summaries", daily_summaries.len());

        // Step 7: Update last processed timestamp
        if let Some(max_locked_at) = efficiency_orders.iter().map(|o| o.locked_at).max() {
            self.db.set_last_processed_locked_at(max_locked_at).await?;
        }

        tracing::info!("Market efficiency analysis complete");
        Ok(())
    }

    fn compute_efficiency_orders(
        &self,
        fulfilled_requests: &[&RequestForEfficiency],
        all_requests: &[RequestForEfficiency],
    ) -> Vec<MarketEfficiencyOrder> {
        let mut efficiency_orders = Vec::with_capacity(fulfilled_requests.len());

        for r in fulfilled_requests {
            let lock_time = match r.locked_at {
                Some(t) => t,
                None => continue,
            };

            let r_price_per_cycle = match &r.lock_price_per_cycle {
                Some(ppc) => *ppc,
                None => continue,
            };

            let r_lock_price = match &r.lock_price {
                Some(lp) => *lp,
                None => continue,
            };

            let r_program_cycles = match &r.program_cycles {
                Some(pc) => *pc,
                None => continue,
            };

            // Find all orders that were available at lock_time
            let mut more_profitable: Vec<(&RequestForEfficiency, U256)> = Vec::new();
            let mut less_profitable_count = 0u64;
            let mut available_unfulfilled_count = 0u64;

            for o in all_requests.iter() {
                if o.request_digest == r.request_digest {
                    continue;
                }

                // Check if O was available at lock_time
                let was_submitted = o.created_at <= lock_time;
                let was_not_locked = o.locked_at.is_none() || o.locked_at.unwrap() > lock_time;
                let was_not_expired = o.lock_end > lock_time;

                if !was_submitted || !was_not_locked || !was_not_expired {
                    continue;
                }

                // Check if O was eventually fulfilled (so we know cycles)
                if o.fulfilled_at.is_none() || o.program_cycles.is_none() {
                    available_unfulfilled_count += 1;
                    continue;
                }

                let o_program_cycles = o.program_cycles.unwrap();
                if o_program_cycles == U256::ZERO {
                    continue;
                }

                // Compute O's hypothetical lock price at lock_time
                let lock_timeout = o.lock_end.saturating_sub(o.ramp_up_start) as u32;
                let o_lock_price_at_time = price_at_time(
                    o.min_price,
                    o.max_price,
                    o.ramp_up_start,
                    o.ramp_up_period as u32,
                    lock_timeout,
                    lock_time,
                );

                // Skip if price would be zero (past lock deadline)
                if o_lock_price_at_time == U256::ZERO {
                    continue;
                }

                let o_price_per_cycle_at_time = o_lock_price_at_time / o_program_cycles;

                if o_price_per_cycle_at_time > r_price_per_cycle {
                    more_profitable.push((o, o_price_per_cycle_at_time));
                } else {
                    less_profitable_count += 1;
                }
            }

            // Sort by profitability descending
            more_profitable.sort_by(|a, b| b.1.cmp(&a.1));

            // Take top 5 for sample
            let sample: Option<Vec<MoreProfitableSample>> = if more_profitable.is_empty() {
                None
            } else {
                Some(
                    more_profitable
                        .iter()
                        .take(5)
                        .map(|(o, ppc)| {
                            let lock_timeout = o.lock_end.saturating_sub(o.ramp_up_start) as u32;
                            let lock_price_at_time = price_at_time(
                                o.min_price,
                                o.max_price,
                                o.ramp_up_start,
                                o.ramp_up_period as u32,
                                lock_timeout,
                                lock_time,
                            );
                            MoreProfitableSample {
                                request_digest: format!("{:x}", o.request_digest),
                                request_id: o.request_id.to_string(),
                                lock_price_at_time: lock_price_at_time.to_string(),
                                program_cycles: o.program_cycles.unwrap_or(U256::ZERO).to_string(),
                                price_per_cycle_at_time: ppc.to_string(),
                            }
                        })
                        .collect(),
                )
            };

            let is_most_profitable = more_profitable.is_empty();

            efficiency_orders.push(MarketEfficiencyOrder {
                request_digest: r.request_digest,
                request_id: r.request_id,
                locked_at: lock_time,
                lock_price: r_lock_price,
                program_cycles: r_program_cycles,
                lock_price_per_cycle: r_price_per_cycle,
                num_orders_more_profitable: more_profitable.len() as u64,
                num_orders_less_profitable: less_profitable_count,
                num_orders_available_unfulfilled: available_unfulfilled_count,
                is_most_profitable,
                more_profitable_sample: sample,
            });
        }

        efficiency_orders
    }

    fn aggregate_hourly(&self, orders: &[MarketEfficiencyOrder]) -> Vec<MarketEfficiencyHourly> {
        let mut hourly_map: HashMap<u64, (u64, u64)> = HashMap::new();

        for order in orders {
            let hour_start = get_hour_start(order.locked_at);
            let entry = hourly_map.entry(hour_start).or_insert((0, 0));
            if order.is_most_profitable {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }

        hourly_map
            .into_iter()
            .map(|(period_timestamp, (most, not_most))| {
                let total = most + not_most;
                let efficiency_rate = if total > 0 { most as f64 / total as f64 } else { 0.0 };
                MarketEfficiencyHourly {
                    period_timestamp,
                    num_most_profitable_locked: most,
                    num_not_most_profitable_locked: not_most,
                    efficiency_rate,
                }
            })
            .collect()
    }

    fn aggregate_daily(&self, orders: &[MarketEfficiencyOrder]) -> Vec<MarketEfficiencyDaily> {
        let mut daily_map: HashMap<u64, (u64, u64)> = HashMap::new();

        for order in orders {
            let day_start = get_day_start(order.locked_at);
            let entry = daily_map.entry(day_start).or_insert((0, 0));
            if order.is_most_profitable {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }

        daily_map
            .into_iter()
            .map(|(period_timestamp, (most, not_most))| {
                let total = most + not_most;
                let efficiency_rate = if total > 0 { most as f64 / total as f64 } else { 0.0 };
                MarketEfficiencyDaily {
                    period_timestamp,
                    num_most_profitable_locked: most,
                    num_not_most_profitable_locked: not_most,
                    efficiency_rate,
                }
            })
            .collect()
    }
}
