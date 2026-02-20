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

use crate::{
    config::{OrderCommitmentPriority, OrderPricingPriority},
    order_monitor::OrderMonitor,
    order_picker::OrderPicker,
    FulfillmentType, OrderRequest,
};

use alloy::primitives::U256;
use rand::seq::SliceRandom;
use rand::Rng;
use std::sync::Arc;

/// Unified priority mode for both pricing and commitment
#[derive(Debug, Clone, Copy)]
enum UnifiedPriorityMode {
    Random,
    TimeOrdered,
    ShortestExpiry,
    Price,
    CyclePrice,
}

impl From<OrderPricingPriority> for UnifiedPriorityMode {
    fn from(mode: OrderPricingPriority) -> Self {
        match mode {
            OrderPricingPriority::Random => UnifiedPriorityMode::Random,
            OrderPricingPriority::ObservationTime => UnifiedPriorityMode::TimeOrdered,
            OrderPricingPriority::ShortestExpiry => UnifiedPriorityMode::ShortestExpiry,
        }
    }
}

impl From<OrderCommitmentPriority> for UnifiedPriorityMode {
    fn from(mode: OrderCommitmentPriority) -> Self {
        match mode {
            OrderCommitmentPriority::Random => UnifiedPriorityMode::Random,
            OrderCommitmentPriority::ShortestExpiry => UnifiedPriorityMode::ShortestExpiry,
            OrderCommitmentPriority::Price => UnifiedPriorityMode::Price,
            OrderCommitmentPriority::CyclePrice => UnifiedPriorityMode::CyclePrice,
        }
    }
}

fn sort_orders_by_priority_and_mode<T>(
    orders: &mut Vec<T>,
    priority_addresses: Option<&[alloy::primitives::Address]>,
    mode: UnifiedPriorityMode,
) where
    T: AsRef<OrderRequest>,
{
    let Some(addresses) = priority_addresses else {
        sort_by_mode(orders, mode);
        return;
    };

    let (mut priority_orders, mut regular_orders): (Vec<T>, Vec<T>) = orders
        .drain(..)
        .partition(|order| addresses.contains(&order.as_ref().request.client_address()));

    sort_by_mode(&mut priority_orders, mode);
    sort_by_mode(&mut regular_orders, mode);

    orders.extend(priority_orders);
    orders.extend(regular_orders);
}

fn sort_by_mode<T>(orders: &mut [T], mode: UnifiedPriorityMode)
where
    T: AsRef<OrderRequest>,
{
    let now = crate::now_timestamp();
    let mut rng = rand::rng();

    match mode {
        UnifiedPriorityMode::Random => orders.shuffle(&mut rng),
        UnifiedPriorityMode::TimeOrdered => {
            // Already in observation time order, no sorting needed
        }
        UnifiedPriorityMode::ShortestExpiry => {
            orders.sort_by_key(|order| order.as_ref().expiry());
        }
        UnifiedPriorityMode::Price => {
            orders
                .sort_by_key(|o| std::cmp::Reverse(total_reward_amount(o.as_ref(), now, &mut rng)));
        }
        UnifiedPriorityMode::CyclePrice => {
            orders.sort_by_key(|o| {
                let amount = total_reward_amount(o.as_ref(), now, &mut rng);
                std::cmp::Reverse(
                    o.as_ref()
                        .total_cycles
                        .and_then(|cycles| amount.checked_div(U256::from(cycles)))
                        .unwrap_or_default(),
                )
            });
        }
    }
}

fn total_reward_amount<R>(order: &OrderRequest, now: u64, rng: &mut R) -> U256
where
    R: Rng,
{
    if matches!(order.fulfillment_type, FulfillmentType::FulfillAfterLockExpire) {
        // Secondary orders: use pre-computed ETH value: collateral_reward_if_locked_and_not_fulfilled() * expected_probability_win_secondary_fulfillment
        let expected_reward_eth = order.expected_reward_eth.unwrap_or_default();

        // Secondary orders are proof races — multiple provers compete to fulfill them.
        // If all provers rank orders identically by expected_reward_eth, they all pick the
        // same order, wasting network capacity. Applying a random factor spreads provers
        // across different orders.
        // Multiply by a random factor between 0.2 and 1.0 (scaled by 1000 for integer math).
        let factor = rng.random_range(200u64..=1000u64);
        expected_reward_eth * U256::from(factor) / U256::from(1000u64)
    } else {
        // Primary orders: use the current auction price (already in ETH)
        order.request.offer.price_at(now).unwrap_or_default()
    }
}

impl<P> OrderPicker<P> {
    #[allow(clippy::vec_box)]
    pub(crate) fn select_pricing_orders(
        &self,
        orders: &mut Vec<Box<OrderRequest>>,
        priority_mode: OrderPricingPriority,
        priority_addresses: Option<&[alloy::primitives::Address]>,
        capacity: usize,
    ) -> Vec<Box<OrderRequest>> {
        if orders.is_empty() || capacity == 0 {
            return Vec::new();
        }

        sort_orders_by_priority_and_mode(orders, priority_addresses, priority_mode.into());

        let take_count = std::cmp::min(capacity, orders.len());
        orders.drain(..take_count).collect()
    }
}

impl<P> OrderMonitor<P> {
    /// Default implementation of order prioritization logic for choosing which order to commit to
    /// prove.
    pub(crate) fn prioritize_orders(
        &self,
        mut orders: Vec<Arc<OrderRequest>>,
        priority_mode: OrderCommitmentPriority,
        priority_addresses: Option<&[alloy::primitives::Address]>,
    ) -> Vec<Arc<OrderRequest>> {
        // Sort orders with priority addresses first, then by mode
        sort_orders_by_priority_and_mode(&mut orders, priority_addresses, priority_mode.into());

        tracing::debug!(
            "Orders ready for proving, prioritized. Before applying capacity limits: {}",
            orders.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        );

        orders
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::{
        now_timestamp,
        order_monitor::tests::setup_om_test_context,
        order_picker::tests::{OrderParams, PickerTestCtxBuilder},
        FulfillmentType,
    };
    use alloy::primitives::U256;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_order_pricing_priority_observation_time() {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let mut orders = Vec::new();
        for i in 0..5 {
            let order = ctx
                .generate_next_order(OrderParams {
                    order_index: i,
                    bidding_start: now_timestamp() + (i as u64 * 10), // Different start times
                    ..Default::default()
                })
                .await;
            orders.push(order);
        }

        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            let selected_orders = ctx.picker.select_pricing_orders(
                &mut orders,
                OrderPricingPriority::ObservationTime,
                None,
                1,
            );
            if let Some(order) = selected_orders.into_iter().next() {
                let order_index =
                    boundless_market::contracts::RequestId::try_from(order.request.id)
                        .unwrap()
                        .index;
                selected_order_indices.push(order_index);
            }
        }

        assert_eq!(selected_order_indices, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_order_pricing_priority_shortest_expiry() {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let base_time = now_timestamp();

        // Create orders with different expiry times (lock timeouts)
        let mut orders = Vec::new();
        let expiry_times = [300, 100, 500, 200, 400]; // Different lock timeouts

        for (i, &timeout) in expiry_times.iter().enumerate() {
            let order = ctx
                .generate_next_order(OrderParams {
                    order_index: i as u32,
                    bidding_start: base_time,
                    lock_timeout: timeout,
                    ..Default::default()
                })
                .await;
            orders.push(order);
        }

        // Test that shortest_expiry mode returns orders by earliest expiry
        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            let selected_orders = ctx.picker.select_pricing_orders(
                &mut orders,
                OrderPricingPriority::ShortestExpiry,
                None,
                1,
            );
            if let Some(order) = selected_orders.into_iter().next() {
                let order_index =
                    boundless_market::contracts::RequestId::try_from(order.request.id)
                        .unwrap()
                        .index;
                selected_order_indices.push(order_index);
            }
        }

        assert_eq!(selected_order_indices, vec![1, 3, 0, 4, 2]);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_order_pricing_priority_shortest_expiry_with_lock_expired() {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let base_time = now_timestamp();

        // Create a mix of regular orders and lock-expired orders
        let mut orders = Vec::new();

        // Regular order with lock timeout 300
        let order1 = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                bidding_start: base_time,
                lock_timeout: 300,
                timeout: 600,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        orders.push(order1);

        // Lock-expired order with timeout 400 (uses timeout for expiry, not lock_timeout)
        let order2 = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                bidding_start: base_time,
                lock_timeout: 200, // This is ignored for lock-expired orders
                timeout: 400,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        orders.push(order2);

        // Regular order with lock timeout 250
        let order3 = ctx
            .generate_next_order(OrderParams {
                order_index: 3,
                bidding_start: base_time,
                lock_timeout: 250,
                timeout: 500,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        orders.push(order3);

        // Test selection order
        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            let selected_orders = ctx.picker.select_pricing_orders(
                &mut orders,
                OrderPricingPriority::ShortestExpiry,
                None,
                1,
            );
            if let Some(order) = selected_orders.into_iter().next() {
                let order_index =
                    boundless_market::contracts::RequestId::try_from(order.request.id)
                        .unwrap()
                        .index;
                selected_order_indices.push(order_index);
            }
        }

        // Should be: 3 (250), 1 (300), 2 (400)
        // Order 3: lock_timeout 250 -> expiry = base_time + 250
        // Order 1: lock_timeout 300 -> expiry = base_time + 300
        // Order 2: timeout 400 (lock-expired) -> expiry = base_time + 400
        assert_eq!(selected_order_indices, vec![3, 1, 2]);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_order_pricing_priority_random() {
        let ctx = PickerTestCtxBuilder::default().build().await;

        // Run the test multiple times to verify randomness
        let mut all_orderings = HashSet::new();

        for _ in 0..20 {
            // Run 20 times to get different random orderings
            let mut orders = Vec::new();
            for i in 0..5 {
                let order = ctx
                    .generate_next_order(OrderParams { order_index: i, ..Default::default() })
                    .await;
                orders.push(order);
            }

            let mut selected_order_indices = Vec::new();
            while !orders.is_empty() {
                let selected_orders = ctx.picker.select_pricing_orders(
                    &mut orders,
                    OrderPricingPriority::Random,
                    None,
                    1,
                );
                if let Some(order) = selected_orders.into_iter().next() {
                    let order_index =
                        boundless_market::contracts::RequestId::try_from(order.request.id)
                            .unwrap()
                            .index;
                    selected_order_indices.push(order_index);
                }
            }

            all_orderings.insert(selected_order_indices);
        }

        assert!(all_orderings.len() > 1, "Random selection should produce different orderings");

        // Verify all orderings contain the same elements (all 5 orders)
        for ordering in &all_orderings {
            let mut sorted_ordering = ordering.clone();
            sorted_ordering.sort();
            assert_eq!(sorted_ordering, vec![0, 1, 2, 3, 4]);
        }
    }

    #[tokio::test]
    async fn test_prioritize_orders() {
        let mut ctx = setup_om_test_context().await;
        let current_timestamp = now_timestamp();

        // Create orders with different expiration times
        // Must lock and fulfill within 50 seconds
        let order1 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 50, 200)
            .await;
        let order_1_id = order1.id();

        // Must lock and fulfill within 100 seconds.
        let order2 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        let order_2_id = order2.id();

        // Must fulfill after lock expires within 51 seconds.
        let order3 = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 1, 51)
            .await;
        let order_3_id = order3.id();

        // Must fulfill after lock expires within 53 seconds.
        let order4 = ctx
            .create_test_order(FulfillmentType::FulfillAfterLockExpire, current_timestamp, 1, 53)
            .await;
        let order_4_id = order4.id();

        let orders =
            vec![Arc::from(order1), Arc::from(order2), Arc::from(order3), Arc::from(order4)];
        let orders =
            ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::ShortestExpiry, None);

        assert!(orders[0].id() == order_1_id);
        assert!(orders[1].id() == order_3_id);
        assert!(orders[2].id() == order_4_id);
        assert!(orders[3].id() == order_2_id);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_expired_order_fulfillment_priority_random() {
        let mut ctx = setup_om_test_context().await;
        let current_timestamp = now_timestamp();

        // Create mixed orders: some lock-and-fulfill, some expired
        let mut orders = Vec::new();

        // Add lock-and-fulfill orders
        for i in 1..=3 {
            let order = ctx
                .create_test_order(
                    FulfillmentType::LockAndFulfill,
                    current_timestamp,
                    100 + (i * 10) as u64,
                    200,
                )
                .await;
            orders.push(Arc::from(order));
        }

        // Add expired orders
        for i in 4..=6 {
            let order = ctx
                .create_test_order(
                    FulfillmentType::FulfillAfterLockExpire,
                    current_timestamp,
                    10,
                    100 + (i * 10) as u64,
                )
                .await;
            orders.push(Arc::from(order));
        }

        // Run multiple times to test randomness of all orders
        let mut all_orderings = HashSet::new();

        for _ in 0..10 {
            let test_orders = orders.clone();
            let test_orders =
                ctx.monitor.prioritize_orders(test_orders, OrderCommitmentPriority::Random, None);

            // Extract the ordering of all orders
            let order_ids: Vec<_> = test_orders.iter().map(|order| order.request.id).collect();
            all_orderings.insert(order_ids);
        }

        // Should see different orderings due to randomness
        assert!(all_orderings.len() > 1, "Random mode should produce different orderings");

        // Test that random mode produces different orderings
        let prioritized =
            ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::Random, None);

        // We should have 3 LockAndFulfill and 3 FulfillAfterLockExpire orders in total
        let lock_and_fulfill_count = prioritized
            .iter()
            .filter(|order| order.fulfillment_type == FulfillmentType::LockAndFulfill)
            .count();
        let fulfill_after_expire_count = prioritized
            .iter()
            .filter(|order| order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire)
            .count();

        assert_eq!(lock_and_fulfill_count, 3);
        assert_eq!(fulfill_after_expire_count, 3);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_high_value_secondary_order_ranks_above_low_value_primary_price_mode() {
        let ctx = PickerTestCtxBuilder::default().build().await;
        let base_time = now_timestamp().saturating_sub(1_000); // ensure price_at(now) == maxPrice

        // Low-value primary order: price_at(now) == maxPrice == 200
        let low_value_primary = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(200u64),
                lock_timeout: 300,
                timeout: 600,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        let low_value_primary = std::sync::Arc::new(*low_value_primary);

        // High-value secondary order: expected_reward_eth * min_factor (0.2) = 400 > primary maxPrice 200
        let mut high_value_secondary = *ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                bidding_start: base_time,
                lock_timeout: 200,
                timeout: 400,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        high_value_secondary.expected_reward_eth = Some(U256::from(2000u64));
        let high_value_secondary = std::sync::Arc::new(high_value_secondary);

        let mut orders = vec![low_value_primary.clone(), high_value_secondary.clone()];
        sort_orders_by_priority_and_mode(&mut orders, None, OrderCommitmentPriority::Price.into());

        // Secondary order should be first: effective reward [400, 2000] > primary price 200
        assert_eq!(
            orders[0].request.id, high_value_secondary.request.id,
            "High-value secondary order should rank first"
        );
        assert_eq!(
            orders[1].request.id, low_value_primary.request.id,
            "Low-value primary order should rank second"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_high_value_secondary_order_ranks_above_low_value_primary_cycle_price_mode() {
        let ctx = PickerTestCtxBuilder::default().build().await;
        let base_time = now_timestamp().saturating_sub(1_000); // ensure price_at(now) == maxPrice

        // Low-value primary: price_at(now) == maxPrice == 200, cycles 100 -> per-cycle = 2
        let mut low_value_primary = *ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(200u64),
                lock_timeout: 300,
                timeout: 600,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        low_value_primary.total_cycles = Some(100);
        let low_value_primary = std::sync::Arc::new(low_value_primary);

        // High-value secondary: expected_reward_eth 2000, cycles 10
        // per-cycle at min factor (0.2x): 400/10 = 40, which is >> primary per-cycle (2)
        let mut high_value_secondary = *ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                bidding_start: base_time,
                lock_timeout: 200,
                timeout: 400,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        high_value_secondary.total_cycles = Some(10);
        high_value_secondary.expected_reward_eth = Some(U256::from(2000u64));
        let high_value_secondary = std::sync::Arc::new(high_value_secondary);

        let mut orders = vec![low_value_primary.clone(), high_value_secondary.clone()];
        sort_orders_by_priority_and_mode(
            &mut orders,
            None,
            OrderCommitmentPriority::CyclePrice.into(),
        );

        // Secondary order should be first: per-cycle reward [40, 200] >> primary per-cycle 2
        assert_eq!(
            orders[0].request.id, high_value_secondary.request.id,
            "High-value secondary order should rank first"
        );
        assert_eq!(
            orders[1].request.id, low_value_primary.request.id,
            "Low-value primary order should rank second"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_expired_order_fulfillment_priority_shortest_expiry() {
        let mut ctx = setup_om_test_context().await;
        let current_timestamp = now_timestamp();

        // Create mixed orders with different expiry times
        let mut orders = Vec::new();

        // Lock-and-fulfill orders with different lock timeouts
        let lock_timeouts = [150, 100, 200]; // Will be sorted: 100, 150, 200
        for &timeout in lock_timeouts.iter() {
            let order = ctx
                .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, timeout, 300)
                .await;
            orders.push(Arc::from(order));
        }

        // Expired orders with different total timeouts
        let total_timeouts = [250, 150, 300]; // Will be sorted: 150, 250, 300
        for &timeout in total_timeouts.iter() {
            let order = ctx
                .create_test_order(
                    FulfillmentType::FulfillAfterLockExpire,
                    current_timestamp,
                    10,
                    timeout,
                )
                .await;
            orders.push(Arc::from(order));
        }

        let prioritized =
            ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::ShortestExpiry, None);

        // Orders should be sorted by their relevant expiry times, regardless of type
        // Expected order: LockAndFulfill(100), LockAndFulfill(150), FulfillAfterLockExpire(150), LockAndFulfill(200), FulfillAfterLockExpire(250), FulfillAfterLockExpire(300)

        // Position 0: LockAndFulfill with lock_expires=100
        assert_eq!(prioritized[0].fulfillment_type, FulfillmentType::LockAndFulfill);
        assert_eq!(prioritized[0].request.lock_expires_at(), current_timestamp + 100);

        // Position 1: LockAndFulfill with lock_expires=150
        assert_eq!(prioritized[1].fulfillment_type, FulfillmentType::LockAndFulfill);
        assert_eq!(prioritized[1].request.lock_expires_at(), current_timestamp + 150);

        // Position 2: FulfillAfterLockExpire with expires=150
        assert_eq!(prioritized[2].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);
        assert_eq!(prioritized[2].request.expires_at(), current_timestamp + 150);

        // Position 3: LockAndFulfill with lock_expires=200
        assert_eq!(prioritized[3].fulfillment_type, FulfillmentType::LockAndFulfill);
        assert_eq!(prioritized[3].request.lock_expires_at(), current_timestamp + 200);

        // Position 4: FulfillAfterLockExpire with expires=250
        assert_eq!(prioritized[4].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);
        assert_eq!(prioritized[4].request.expires_at(), current_timestamp + 250);

        // Position 5: FulfillAfterLockExpire with expires=300
        assert_eq!(prioritized[5].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);
        assert_eq!(prioritized[5].request.expires_at(), current_timestamp + 300);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_expired_order_fulfillment_priority_configuration_change() {
        let mut ctx = setup_om_test_context().await;
        let current_timestamp = now_timestamp();

        // Start with random mode
        ctx.config.load_write().unwrap().market.order_commitment_priority =
            OrderCommitmentPriority::Random;

        // Create only expired orders for this test
        let mut orders = Vec::new();
        for i in 1..=4 {
            let order = ctx
                .create_test_order(
                    FulfillmentType::FulfillAfterLockExpire,
                    current_timestamp,
                    10,
                    100 + (i * 20) as u64, // Different expiry times: 120, 140, 160, 180
                )
                .await;
            orders.push(Arc::from(order));
        }

        // Test random mode (no need to capture result since it's random)
        let _prioritized_random = orders.clone();
        let _prioritized_random = ctx.monitor.prioritize_orders(
            _prioritized_random,
            OrderCommitmentPriority::Random,
            None,
        );

        // Test shortest expiry mode
        let prioritized_shortest =
            ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::ShortestExpiry, None);

        // In shortest expiry mode, orders should be sorted by expiry time
        for i in 0..3 {
            assert!(
                prioritized_shortest[i].request.expires_at()
                    <= prioritized_shortest[i + 1].request.expires_at()
            );
        }

        // Verify the exact order for shortest expiry
        assert_eq!(prioritized_shortest[0].request.expires_at(), current_timestamp + 120);
        assert_eq!(prioritized_shortest[1].request.expires_at(), current_timestamp + 140);
        assert_eq!(prioritized_shortest[2].request.expires_at(), current_timestamp + 160);
        assert_eq!(prioritized_shortest[3].request.expires_at(), current_timestamp + 180);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_priority_requestor_addresses_pricing() {
        let ctx = PickerTestCtxBuilder::default().build().await;
        let base_time = now_timestamp();

        let regular_addr = alloy::primitives::Address::from([0x42; 20]);
        let priority_addr = alloy::primitives::Address::from([0x99; 20]);
        let priority_addresses = vec![priority_addr];

        // Test shortest expiry mode without priority addresses
        let mut regular_order_1 = ctx
            .generate_next_order(OrderParams {
                order_index: 0,
                bidding_start: base_time,
                lock_timeout: 100,
                ..Default::default()
            })
            .await;
        regular_order_1.request.id =
            boundless_market::contracts::RequestId::new(regular_addr, 0).into();

        let mut priority_order_1 = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                bidding_start: base_time,
                lock_timeout: 500,
                ..Default::default()
            })
            .await;
        priority_order_1.request.id =
            boundless_market::contracts::RequestId::new(priority_addr, 1).into();

        let mut test_orders = vec![regular_order_1, priority_order_1];
        let selected_orders = ctx.picker.select_pricing_orders(
            &mut test_orders,
            OrderPricingPriority::ShortestExpiry,
            None,
            1,
        );
        let selected_order = selected_orders.into_iter().next().unwrap();
        assert_eq!(selected_order.request.client_address(), regular_addr); // Regular order selected due to shorter expiry

        // Test shortest expiry mode with priority addresses
        let mut regular_order_2 = ctx
            .generate_next_order(OrderParams {
                order_index: 0,
                bidding_start: base_time,
                lock_timeout: 100,
                ..Default::default()
            })
            .await;
        regular_order_2.request.id =
            boundless_market::contracts::RequestId::new(regular_addr, 0).into();

        let mut priority_order_2 = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                bidding_start: base_time,
                lock_timeout: 500,
                ..Default::default()
            })
            .await;
        priority_order_2.request.id =
            boundless_market::contracts::RequestId::new(priority_addr, 1).into();

        let mut test_orders = vec![regular_order_2, priority_order_2];
        let selected_orders = ctx.picker.select_pricing_orders(
            &mut test_orders,
            OrderPricingPriority::ShortestExpiry,
            Some(&priority_addresses),
            1,
        );
        let selected_order = selected_orders.into_iter().next().unwrap();
        assert_eq!(selected_order.request.client_address(), priority_addr); // Priority order selected first despite longer expiry
    }

    #[tokio::test]
    #[traced_test]
    async fn test_priority_requestor_addresses_commitment() {
        let mut ctx = setup_om_test_context().await;
        let current_timestamp = now_timestamp();

        // Create orders with different priorities and timeouts
        let mut orders = Vec::new();

        // Regular order with short expiry (should be selected first without priority)
        let regular_order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        orders.push(Arc::from(regular_order));

        // Switch the signer address to a new one.
        ctx.signer = crate::PrivateKeySigner::random();
        let priority_addr = ctx.signer.address();
        let priority_addresses = vec![priority_addr];

        // Priority order with long expiry (should be selected first with priority)
        // Note: The order is created with the default signer address (ctx.signer.address())
        // so it will be treated as a priority order
        let priority_order = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 500, 600)
            .await;
        orders.push(Arc::from(priority_order));

        // Test shortest expiry mode without priority addresses
        let test_orders = orders.clone();
        let prioritized_orders = ctx.monitor.prioritize_orders(
            test_orders,
            OrderCommitmentPriority::ShortestExpiry,
            None,
        );
        assert_eq!(prioritized_orders[0].request.lock_expires_at(), current_timestamp + 100); // Regular order first

        // Test shortest expiry mode with priority addresses
        let test_orders = orders.clone();
        let prioritized_orders = ctx.monitor.prioritize_orders(
            test_orders,
            OrderCommitmentPriority::ShortestExpiry,
            Some(&priority_addresses),
        );

        // Priority order should be first despite longer expiry, regular order second
        assert_eq!(prioritized_orders[0].request.lock_expires_at(), current_timestamp + 500);
        assert_eq!(prioritized_orders[0].request.client_address(), priority_addr);
        assert_eq!(prioritized_orders[1].request.lock_expires_at(), current_timestamp + 100);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_commitment_priority_lock_price_ordering_and_expired_shuffled() {
        let ctx = PickerTestCtxBuilder::default().build().await;
        let base_time = now_timestamp().saturating_sub(1_000); // ensure price_at(now) == maxPrice

        // Lock-capable orders with max prices 30_000, 10_000, 20_000 (descending should be 30_000, 20_000, 10_000).
        // Prices are kept well above the secondary orders' maximum effective reward (500) so
        // primary orders always rank first for this test.
        let lock_30 = ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(30_000u64),
                lock_timeout: 10_000,
                timeout: 20_000,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        let lock_30 = std::sync::Arc::new(*lock_30);

        let lock_10 = ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(10_000u64),
                lock_timeout: 10_000,
                timeout: 20_000,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        let lock_10 = std::sync::Arc::new(*lock_10);

        let lock_20 = ctx
            .generate_next_order(OrderParams {
                order_index: 3,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(20_000u64),
                lock_timeout: 10_000,
                timeout: 20_000,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        let lock_20 = std::sync::Arc::new(*lock_20);

        // Expired fulfillment orders
        let mut exp_a = *ctx
            .generate_next_order(OrderParams {
                order_index: 4,
                bidding_start: base_time,
                lock_collateral: U256::from(50u64),
                lock_timeout: 100,
                timeout: 500,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        exp_a.expected_reward_eth = Some(U256::from(500u64)); // effective range [100, 500] with random factor
        let exp_a = std::sync::Arc::new(exp_a);

        let mut exp_b = *ctx
            .generate_next_order(OrderParams {
                order_index: 5,
                bidding_start: base_time,
                lock_collateral: U256::from(30u64),
                lock_timeout: 100,
                timeout: 600,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        exp_b.expected_reward_eth = Some(U256::from(400u64)); // effective range [80, 400] with random factor
        let exp_b = std::sync::Arc::new(exp_b);

        // Verify ordering for LockPrice
        let mut orders =
            vec![lock_30.clone(), lock_10.clone(), lock_20.clone(), exp_a.clone(), exp_b.clone()];

        sort_orders_by_priority_and_mode(&mut orders, None, OrderCommitmentPriority::Price.into());

        // First 3 must be lock-capable, ordered by price desc (30_000, 20_000, 10_000)
        assert_eq!(orders[0].request.offer.maxPrice, U256::from(30_000u64));
        assert_eq!(orders[1].request.offer.maxPrice, U256::from(20_000u64));
        assert_eq!(orders[2].request.offer.maxPrice, U256::from(10_000u64));
        assert!(matches!(
            orders[0].fulfillment_type,
            FulfillmentType::LockAndFulfill | FulfillmentType::FulfillWithoutLocking
        ));
        assert!(matches!(
            orders[1].fulfillment_type,
            FulfillmentType::LockAndFulfill | FulfillmentType::FulfillWithoutLocking
        ));
        assert!(matches!(
            orders[2].fulfillment_type,
            FulfillmentType::LockAndFulfill | FulfillmentType::FulfillWithoutLocking
        ));

        // Last 2 must be expired type
        assert_eq!(orders[3].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);
        assert_eq!(orders[4].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);

        // The expired tail should be randomized across runs, but higher expected_reward_eth should appear first more often
        let mut tails = HashSet::new();
        let mut exp_a_first_count = 0;
        let iterations = 100;
        for _ in 0..iterations {
            let mut test_orders = vec![
                lock_30.clone(),
                lock_10.clone(),
                lock_20.clone(),
                exp_a.clone(),
                exp_b.clone(),
            ];
            sort_orders_by_priority_and_mode(
                &mut test_orders,
                None,
                OrderCommitmentPriority::Price.into(),
            );
            tails.insert((test_orders[3].request.id, test_orders[4].request.id));
            if test_orders[3].request.id == exp_a.request.id {
                exp_a_first_count += 1;
            }
        }
        assert!(tails.len() > 1, "Expired orders should be shuffled in LockPrice mode");
        // exp_a has higher expected_reward_eth (500 vs 400), so should appear first more than half the time
        assert!(
            exp_a_first_count > iterations / 2,
            "Higher expected_reward_eth order should appear first more often: {} out of {}",
            exp_a_first_count,
            iterations
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_commitment_priority_lock_cycle_price_ordering_and_expired_shuffled() {
        // Build orders with same absolute price behavior but different cycles; sort by per-cycle desc.
        let ctx = PickerTestCtxBuilder::default().build().await;
        let base_time = now_timestamp().saturating_sub(1_000);

        // B: max 900, cycles 30 -> per-cycle 30 (first)
        let mut lock_b = *ctx
            .generate_next_order(OrderParams {
                order_index: 10,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(900u64),
                lock_timeout: 10_000,
                timeout: 20_000,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        lock_b.total_cycles = Some(30);
        let lock_b = std::sync::Arc::new(lock_b);

        // A: max 1000, cycles 100 -> per-cycle 10 (second)
        let mut lock_a = *ctx
            .generate_next_order(OrderParams {
                order_index: 11,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(1000u64),
                lock_timeout: 10_000,
                timeout: 20_000,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        lock_a.total_cycles = Some(100);
        let lock_a = std::sync::Arc::new(lock_a);

        // C: max 1200, cycles 200 -> per-cycle 6 (third)
        let mut lock_c = *ctx
            .generate_next_order(OrderParams {
                order_index: 12,
                bidding_start: base_time,
                min_price: U256::from(1u64),
                max_price: U256::from(1200u64),
                lock_timeout: 10_000,
                timeout: 20_000,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                ..Default::default()
            })
            .await;
        lock_c.total_cycles = Some(200);
        let lock_c = std::sync::Arc::new(lock_c);

        // Expired fulfillment orders
        let mut exp_a = *ctx
            .generate_next_order(OrderParams {
                order_index: 13,
                bidding_start: base_time,
                lock_collateral: U256::from(100u64),
                lock_timeout: 100,
                timeout: 500,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        exp_a.total_cycles = Some(10);
        // Per-cycle key at max factor (1.0): 50/10 = 5 < min primary per-cycle (6), so primary always ranks first.
        // Per-cycle effective range [1, 5] with random factor.
        exp_a.expected_reward_eth = Some(U256::from(50u64));
        let exp_a = std::sync::Arc::new(exp_a);

        let mut exp_b = *ctx
            .generate_next_order(OrderParams {
                order_index: 14,
                bidding_start: base_time,
                lock_collateral: U256::from(40u64),
                lock_timeout: 100,
                timeout: 600,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        exp_b.total_cycles = Some(10);
        // Per-cycle key at max factor (1.0): 40/10 = 4 < min primary per-cycle (6).
        // Per-cycle effective range [0, 4] with random factor.
        exp_b.expected_reward_eth = Some(U256::from(40u64));
        let exp_b = std::sync::Arc::new(exp_b);

        // Verify ordering for CyclePrice: B (30), A (10), C (6)
        let mut orders =
            vec![lock_a.clone(), lock_b.clone(), lock_c.clone(), exp_a.clone(), exp_b.clone()];

        sort_orders_by_priority_and_mode(
            &mut orders,
            None,
            OrderCommitmentPriority::CyclePrice.into(),
        );

        assert_eq!(orders[0].request.id, lock_b.request.id);
        assert_eq!(orders[1].request.id, lock_a.request.id);
        assert_eq!(orders[2].request.id, lock_c.request.id);
        assert!(matches!(
            orders[0].fulfillment_type,
            FulfillmentType::LockAndFulfill | FulfillmentType::FulfillWithoutLocking
        ));
        assert!(matches!(
            orders[1].fulfillment_type,
            FulfillmentType::LockAndFulfill | FulfillmentType::FulfillWithoutLocking
        ));
        assert!(matches!(
            orders[2].fulfillment_type,
            FulfillmentType::LockAndFulfill | FulfillmentType::FulfillWithoutLocking
        ));

        // Last 2 must be expired type
        assert_eq!(orders[3].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);
        assert_eq!(orders[4].fulfillment_type, FulfillmentType::FulfillAfterLockExpire);

        // The expired tail should be randomized across runs, but higher expected_reward_eth should appear first more often
        let mut tails = HashSet::new();
        let mut exp_a_first_count = 0;
        let iterations = 50;
        for _ in 0..iterations {
            let mut test_orders =
                vec![lock_a.clone(), lock_b.clone(), lock_c.clone(), exp_a.clone(), exp_b.clone()];
            sort_orders_by_priority_and_mode(
                &mut test_orders,
                None,
                OrderCommitmentPriority::CyclePrice.into(),
            );
            tails.insert((test_orders[3].request.id, test_orders[4].request.id));
            if test_orders[3].request.id == exp_a.request.id {
                exp_a_first_count += 1;
            }
        }
        assert!(tails.len() > 1, "Expired orders should be shuffled in CyclePrice mode");
        // exp_a has higher expected_reward_eth (50 vs 40), so should appear first more than half the time
        assert!(
            exp_a_first_count > iterations / 2,
            "Higher expected_reward_eth order should appear first more often: {} out of {}",
            exp_a_first_count,
            iterations
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_secondary_orders_randomized_by_price_mode() {
        // Verify that secondary orders with identical expected_reward_eth produce different
        // orderings across runs, testing the random factor spreads provers across orders.
        let ctx = PickerTestCtxBuilder::default().build().await;
        let base_time = now_timestamp().saturating_sub(1_000);

        let mut sec_1 = *ctx
            .generate_next_order(OrderParams {
                order_index: 1,
                bidding_start: base_time,
                lock_timeout: 100,
                timeout: 500,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        sec_1.expected_reward_eth = Some(U256::from(1000u64));
        let sec_1 = std::sync::Arc::new(sec_1);

        let mut sec_2 = *ctx
            .generate_next_order(OrderParams {
                order_index: 2,
                bidding_start: base_time,
                lock_timeout: 100,
                timeout: 500,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        sec_2.expected_reward_eth = Some(U256::from(1000u64));
        let sec_2 = std::sync::Arc::new(sec_2);

        let mut sec_3 = *ctx
            .generate_next_order(OrderParams {
                order_index: 3,
                bidding_start: base_time,
                lock_timeout: 100,
                timeout: 500,
                fulfillment_type: FulfillmentType::FulfillAfterLockExpire,
                ..Default::default()
            })
            .await;
        sec_3.expected_reward_eth = Some(U256::from(1000u64));
        let sec_3 = std::sync::Arc::new(sec_3);

        let mut all_orderings = HashSet::new();
        for _ in 0..20 {
            let mut orders = vec![sec_1.clone(), sec_2.clone(), sec_3.clone()];
            sort_orders_by_priority_and_mode(
                &mut orders,
                None,
                OrderCommitmentPriority::Price.into(),
            );
            let ids: Vec<_> = orders.iter().map(|o| o.request.id).collect();
            all_orderings.insert(ids);
        }

        assert!(
            all_orderings.len() > 1,
            "Secondary orders with identical expected_reward_eth should produce different orderings due to random factor"
        );
    }
}
