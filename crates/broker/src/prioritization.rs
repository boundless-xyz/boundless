// Copyright 2025 RISC Zero, Inc.
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
    OrderRequest,
};

use alloy::primitives::U256;
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::sync::Arc;

/// Calculate the profitability (net price per cycle) for an order
/// Returns the net price per cycle after accounting for gas costs, or 0 if cycles are not available
fn calculate_profitability(order: &OrderRequest, gas_cost: Option<U256>) -> U256 {
    let Some(total_cycles) = order.total_cycles else {
        tracing::debug!("Order {} has no total_cycles, profitability = 0", order.id());
        return U256::ZERO;
    };

    if total_cycles == 0 {
        tracing::debug!("Order {} has 0 total_cycles, profitability = 0", order.id());
        return U256::ZERO;
    }

    // Use the current price at the time of prioritization, not max price
    let current_price = order.request.offer.price_at(crate::now_timestamp()).unwrap_or(U256::ZERO);

    // Subtract gas costs if provided
    let net_price = if let Some(gas_cost) = gas_cost {
        current_price.saturating_sub(gas_cost)
    } else {
        current_price
    };

    // Ensure we don't have negative profitability
    if net_price == U256::ZERO {
        tracing::debug!(
            "Order {} has zero or negative net price after gas costs, profitability = 0",
            order.id()
        );
        return U256::ZERO;
    }

    let profitability = net_price / U256::from(total_cycles);

    tracing::debug!(
        "Order {} profitability calculation: current_price={}, gas_cost={:?}, net_price={}, total_cycles={}, profitability={}",
        order.id(),
        current_price,
        gas_cost,
        net_price,
        total_cycles,
        profitability
    );

    profitability
}

/// Unified priority mode for both pricing and commitment
#[derive(Debug, Clone, Copy)]
enum UnifiedPriorityMode {
    Random,
    TimeOrdered,
    ShortestExpiry,
    HighestProfitability,
}

impl From<OrderPricingPriority> for UnifiedPriorityMode {
    fn from(mode: OrderPricingPriority) -> Self {
        match mode {
            OrderPricingPriority::Random => UnifiedPriorityMode::Random,
            OrderPricingPriority::ObservationTime => UnifiedPriorityMode::TimeOrdered,
            OrderPricingPriority::ShortestExpiry => UnifiedPriorityMode::ShortestExpiry,
            OrderPricingPriority::HighestProfitability => UnifiedPriorityMode::HighestProfitability,
        }
    }
}

impl From<OrderCommitmentPriority> for UnifiedPriorityMode {
    fn from(mode: OrderCommitmentPriority) -> Self {
        match mode {
            OrderCommitmentPriority::Random => UnifiedPriorityMode::Random,
            OrderCommitmentPriority::ShortestExpiry => UnifiedPriorityMode::ShortestExpiry,
            OrderCommitmentPriority::HighestProfitability => {
                UnifiedPriorityMode::HighestProfitability
            }
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
    sort_orders_by_priority_and_mode_with_gas(orders, priority_addresses, mode, None);
}

fn sort_orders_by_priority_and_mode_with_gas<T>(
    orders: &mut Vec<T>,
    priority_addresses: Option<&[alloy::primitives::Address]>,
    mode: UnifiedPriorityMode,
    gas_costs: Option<&HashMap<String, U256>>,
) where
    T: AsRef<OrderRequest>,
{
    let Some(addresses) = priority_addresses else {
        tracing::debug!("No priority addresses specified, sorting all orders by mode: {:?}", mode);
        sort_by_mode_with_gas(orders, mode, gas_costs);
        return;
    };

    tracing::debug!(
        "Priority addresses specified: {}, sorting orders with priority first",
        addresses.len()
    );

    let (mut priority_orders, mut regular_orders): (Vec<T>, Vec<T>) = orders
        .drain(..)
        .partition(|order| addresses.contains(&order.as_ref().request.client_address()));

    tracing::debug!(
        "Partitioned orders: {} priority orders, {} regular orders",
        priority_orders.len(),
        regular_orders.len()
    );

    sort_by_mode_with_gas(&mut priority_orders, mode, gas_costs);
    sort_by_mode_with_gas(&mut regular_orders, mode, gas_costs);

    orders.extend(priority_orders);
    orders.extend(regular_orders);

    tracing::debug!("Final order after priority sorting: {} total orders", orders.len());
}

fn sort_by_mode<T>(orders: &mut [T], mode: UnifiedPriorityMode)
where
    T: AsRef<OrderRequest>,
{
    sort_by_mode_with_gas(orders, mode, None);
}

fn sort_by_mode_with_gas<T>(
    orders: &mut [T],
    mode: UnifiedPriorityMode,
    gas_costs: Option<&HashMap<String, U256>>,
) where
    T: AsRef<OrderRequest>,
{
    match mode {
        UnifiedPriorityMode::Random => orders.shuffle(&mut rand::rng()),
        UnifiedPriorityMode::TimeOrdered => {
            // Already in observation time order, no sorting needed
        }
        UnifiedPriorityMode::ShortestExpiry => {
            orders.sort_by_key(|order| order.as_ref().expiry());
        }
        UnifiedPriorityMode::HighestProfitability => {
            tracing::debug!(
                "Using HIGHEST PROFITABILITY mode: Sorting {} orders by profitability",
                orders.len()
            );

            orders.sort_by(|a, b| {
                let order_a = a.as_ref();
                let order_b = b.as_ref();

                // Get gas cost for this order if available
                let gas_cost_a = gas_costs.and_then(|costs| costs.get(&order_a.id())).copied();
                let gas_cost_b = gas_costs.and_then(|costs| costs.get(&order_b.id())).copied();

                let profitability_a = calculate_profitability(order_a, gas_cost_a);
                let profitability_b = calculate_profitability(order_b, gas_cost_b);

                profitability_b.cmp(&profitability_a) // Descending order (highest first)
            });

            tracing::debug!(
                "Orders sorted by profitability (highest first): {}",
                orders
                    .iter()
                    .take(5) // Only show first 5 orders
                    .enumerate()
                    .map(|(i, order)| {
                        let gas_cost =
                            gas_costs.and_then(|costs| costs.get(&order.as_ref().id())).copied();
                        let profitability = calculate_profitability(order.as_ref(), gas_cost);
                        format!("[{}] {} (profitability={})", i, order.as_ref().id(), profitability)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
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
            tracing::debug!("No orders to select or capacity is 0");
            return Vec::new();
        }

        tracing::debug!(
            "Selecting pricing orders: {} orders available, capacity={}, priority_mode={:?}",
            orders.len(),
            capacity,
            priority_mode
        );

        sort_orders_by_priority_and_mode(orders, priority_addresses, priority_mode.into());

        let take_count = std::cmp::min(capacity, orders.len());
        let selected_orders: Vec<Box<OrderRequest>> = orders.drain(..take_count).collect();

        tracing::debug!("Selected {} orders for pricing", selected_orders.len());

        selected_orders
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
        tracing::debug!(
            "Prioritizing {} orders for commitment: priority_mode={:?}",
            orders.len(),
            priority_mode
        );

        // Sort orders with priority addresses first, then by mode
        sort_orders_by_priority_and_mode(&mut orders, priority_addresses, priority_mode.into());

        tracing::debug!("Orders ready for proving, prioritized: {} orders", orders.len());

        orders
    }

    /// Prioritize orders with gas cost consideration for more accurate profitability calculation
    pub(crate) fn prioritize_orders_with_gas_costs(
        &self,
        mut orders: Vec<Arc<OrderRequest>>,
        priority_mode: OrderCommitmentPriority,
        priority_addresses: Option<&[alloy::primitives::Address]>,
        gas_costs: Option<&HashMap<String, U256>>,
    ) -> Vec<Arc<OrderRequest>> {
        tracing::debug!(
            "Prioritizing {} orders for commitment with gas costs: priority_mode={:?}",
            orders.len(),
            priority_mode
        );

        // Sort orders with priority addresses first, then by mode with gas costs
        sort_orders_by_priority_and_mode_with_gas(
            &mut orders,
            priority_addresses,
            priority_mode.into(),
            gas_costs,
        );

        tracing::debug!(
            "Orders ready for proving, prioritized with gas costs: {} orders",
            orders.len()
        );

        orders
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::now_timestamp;
    use crate::order_monitor::tests::setup_om_test_context;
    use crate::order_picker::tests::{OrderParams, PickerTestCtxBuilder};
    use crate::FulfillmentType;
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
    async fn test_order_pricing_priority_highest_profitability_with_logging() {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let base_time = now_timestamp();

        // Create orders with different profitability (current price per cycle)
        // Since we're using current price (which starts at min_price), we need to set min_price
        let mut orders = Vec::new();
        let test_cases = [
            (100, 1000, 1000), // 1000 wei for 100 cycles = 10 wei/cycle
            (200, 2000, 2000), // 2000 wei for 200 cycles = 10 wei/cycle (same as above)
            (300, 1500, 1500), // 1500 wei for 300 cycles = 5 wei/cycle
            (400, 4000, 4000), // 4000 wei for 400 cycles = 10 wei/cycle (same as first two)
            (500, 1000, 1000), // 1000 wei for 500 cycles = 2 wei/cycle
        ];

        for (i, (cycles, min_price, max_price)) in test_cases.iter().enumerate() {
            let mut order = ctx
                .generate_next_order(OrderParams {
                    order_index: i as u32,
                    bidding_start: base_time,
                    lock_timeout: 300,
                    min_price: U256::from(*min_price),
                    max_price: U256::from(*max_price),
                    ..Default::default()
                })
                .await;
            // Manually set cycles for profitability calculation
            order.total_cycles = Some(*cycles);
            orders.push(order);
        }

        // Test that highest profitability mode returns orders by profitability (descending)
        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            let selected_orders = ctx.picker.select_pricing_orders(
                &mut orders,
                OrderPricingPriority::HighestProfitability,
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

        // Expected order: 0, 1, 3 (10 wei/cycle), then 2 (5 wei/cycle), then 4 (2 wei/cycle)
        // Orders with same profitability maintain their relative order
        assert_eq!(selected_order_indices, vec![0, 1, 3, 2, 4]);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_order_pricing_priority_highest_profitability() {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let base_time = now_timestamp();

        // Create orders with different profitability (current price per cycle)
        let mut orders = Vec::new();
        let test_cases = [
            (100, 1000, 1000), // 1000 wei for 100 cycles = 10 wei/cycle
            (200, 2000, 2000), // 2000 wei for 200 cycles = 10 wei/cycle (same as above)
            (300, 1500, 1500), // 1500 wei for 300 cycles = 5 wei/cycle
            (400, 4000, 4000), // 4000 wei for 400 cycles = 10 wei/cycle (same as first two)
            (500, 1000, 1000), // 1000 wei for 500 cycles = 2 wei/cycle
        ];

        for (i, (cycles, min_price, max_price)) in test_cases.iter().enumerate() {
            let mut order = ctx
                .generate_next_order(OrderParams {
                    order_index: i as u32,
                    bidding_start: base_time,
                    lock_timeout: 300,
                    min_price: U256::from(*min_price),
                    max_price: U256::from(*max_price),
                    ..Default::default()
                })
                .await;
            // Manually set cycles for profitability calculation
            order.total_cycles = Some(*cycles);
            orders.push(order);
        }

        // Test that highest profitability mode returns orders by profitability (descending)
        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            let selected_orders = ctx.picker.select_pricing_orders(
                &mut orders,
                OrderPricingPriority::HighestProfitability,
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

        // Expected order: 0, 1, 3 (10 wei/cycle), then 2 (5 wei/cycle), then 4 (2 wei/cycle)
        // Orders with same profitability maintain their relative order
        assert_eq!(selected_order_indices, vec![0, 1, 3, 2, 4]);
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
    #[traced_test]
    async fn test_prioritize_orders_highest_profitability() {
        let mut ctx = setup_om_test_context().await;
        let current_timestamp = now_timestamp();

        // Create orders with different profitability
        let mut orders = Vec::new();

        // High profitability order (10 wei per cycle)
        let mut order1 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        // Manually set cycles and price for profitability calculation
        order1.total_cycles = Some(100);
        order1.request.offer.maxPrice = U256::from(1000); // 1000 wei for 100 cycles = 10 wei/cycle
        let order_1_id = order1.id();

        // Medium profitability order (5 wei per cycle)
        let mut order2 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 200, 300)
            .await;
        order2.total_cycles = Some(200);
        order2.request.offer.maxPrice = U256::from(1000); // 1000 wei for 200 cycles = 5 wei/cycle
        let order_2_id = order2.id();

        // Low profitability order (2 wei per cycle)
        let mut order3 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 500, 600)
            .await;
        order3.total_cycles = Some(500);
        order3.request.offer.maxPrice = U256::from(1000); // 1000 wei for 500 cycles = 2 wei/cycle
        let order_3_id = order3.id();

        let orders = vec![Arc::from(order1), Arc::from(order2), Arc::from(order3)];
        let prioritized_orders = ctx.monitor.prioritize_orders(
            orders,
            OrderCommitmentPriority::HighestProfitability,
            None,
        );

        // Should be ordered by profitability: order1 (10 wei/cycle), order2 (5 wei/cycle), order3 (2 wei/cycle)
        assert_eq!(prioritized_orders[0].id(), order_1_id);
        assert_eq!(prioritized_orders[1].id(), order_2_id);
        assert_eq!(prioritized_orders[2].id(), order_3_id);
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
    async fn test_profitability_with_gas_costs() {
        let mut ctx = setup_om_test_context().await;
        let current_timestamp = now_timestamp();

        // Create orders with different profitability and gas costs
        let mut orders = Vec::new();

        // Order 1: High price, high gas cost (net: 800 wei for 100 cycles = 8 wei/cycle)
        let mut order1 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 100, 200)
            .await;
        order1.total_cycles = Some(100);
        order1.request.offer.minPrice = U256::from(1000);
        order1.request.offer.maxPrice = U256::from(1000);
        let order1_id = order1.id();

        // Order 2: Medium price, low gas cost (net: 800 wei for 200 cycles = 4 wei/cycle)
        let mut order2 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 200, 300)
            .await;
        order2.total_cycles = Some(200);
        order2.request.offer.minPrice = U256::from(1000);
        order2.request.offer.maxPrice = U256::from(1000);
        let order2_id = order2.id();

        // Order 3: Low price, low gas cost (net: 400 wei for 100 cycles = 4 wei/cycle)
        let mut order3 = ctx
            .create_test_order(FulfillmentType::LockAndFulfill, current_timestamp, 300, 400)
            .await;
        order3.total_cycles = Some(100);
        order3.request.offer.minPrice = U256::from(600);
        order3.request.offer.maxPrice = U256::from(600);
        let order3_id = order3.id();

        let orders = vec![Arc::from(order1), Arc::from(order2), Arc::from(order3)];

        // Create gas costs map
        let mut gas_costs = HashMap::new();
        gas_costs.insert(order1_id.clone(), U256::from(200)); // 200 wei gas cost
        gas_costs.insert(order2_id.clone(), U256::from(200)); // 200 wei gas cost
        gas_costs.insert(order3_id.clone(), U256::from(200)); // 200 wei gas cost

        // Test prioritization with gas costs
        let prioritized_orders = ctx.monitor.prioritize_orders_with_gas_costs(
            orders,
            OrderCommitmentPriority::HighestProfitability,
            None,
            Some(&gas_costs),
        );

        // Should be ordered by net profitability: order1 (8 wei/cycle), order2 (4 wei/cycle), order3 (4 wei/cycle)
        assert_eq!(prioritized_orders[0].id(), order1_id);
        assert_eq!(prioritized_orders[1].id(), order2_id);
        assert_eq!(prioritized_orders[2].id(), order3_id);
    }
}
