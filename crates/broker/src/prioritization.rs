// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    config::{OrderCommitmentPriority, OrderPricingPriority},
    order_monitor::OrderMonitor,
    order_picker::OrderPicker,
    FulfillmentType, OrderRequest,
};
use rand::seq::SliceRandom;
use rand::Rng;
use std::{collections::VecDeque, sync::Arc};

impl<P> OrderPicker<P> {
    /// Select the next order for pricing based on the configured order pricing priority. This
    /// method chooses which of the orders that have been observed to preflight to be ready to be
    /// committed to prove.
    ///
    /// This method can be modified to implement custom order selection strategies. It has access
    /// to [`OrderPicker`] state, allowing it to use OrderPicker state such as current prover
    /// capacity, market conditions, or other contextual information for order selection.
    pub(crate) fn select_next_pricing_order(
        &self,
        orders: &mut VecDeque<Box<OrderRequest>>,
        priority_mode: OrderPricingPriority,
    ) -> Option<Box<OrderRequest>> {
        if orders.is_empty() {
            return None;
        }

        match priority_mode {
            OrderPricingPriority::Random => {
                let mut rng = rand::rng();
                let index = rng.random_range(0..orders.len());
                orders.remove(index)
            }
            OrderPricingPriority::ObservationTime => orders.pop_front(),
            OrderPricingPriority::ShortestExpiry => {
                let (shortest_index, _) = orders.iter().enumerate().min_by_key(|(_, order)| {
                    if order.fulfillment_type == FulfillmentType::FulfillAfterLockExpire {
                        order.request.offer.biddingStart + order.request.offer.timeout as u64
                    } else {
                        order.request.offer.biddingStart + order.request.offer.lockTimeout as u64
                    }
                })?;
                orders.remove(shortest_index)
            }
        }
    }
}

impl<P> OrderMonitor<P> {
    /// Default implementation of order prioritization logic for choosing which order to commit to
    /// prove.
    pub(crate) fn prioritize_orders(
        &self,
        mut orders: Vec<Arc<OrderRequest>>,
        priority_mode: OrderCommitmentPriority,
    ) -> Vec<Arc<OrderRequest>> {
        match priority_mode {
            OrderCommitmentPriority::ShortestExpiry => {
                orders.sort_by_key(|order| {
                    if order.fulfillment_type == FulfillmentType::LockAndFulfill {
                        order.request.lock_expires_at()
                    } else {
                        order.request.expires_at()
                    }
                });
            }
            OrderCommitmentPriority::Random => {
                orders.shuffle(&mut rand::rng());
            }
        }

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
    use crate::now_timestamp;
    use crate::order_monitor::tests::setup_om_test_context;
    use crate::order_picker::tests::{OrderParams, PickerTestCtxBuilder};
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_order_pricing_priority_observation_time() {
        let ctx = PickerTestCtxBuilder::default().build().await;

        let mut orders = VecDeque::new();
        for i in 0..5 {
            let order = ctx
                .generate_next_order(OrderParams {
                    order_index: i,
                    bidding_start: now_timestamp() + (i as u64 * 10), // Different start times
                    ..Default::default()
                })
                .await;
            orders.push_back(order);
        }

        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            if let Some(order) = ctx
                .picker
                .select_next_pricing_order(&mut orders, OrderPricingPriority::ObservationTime)
            {
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
        let mut orders = VecDeque::new();
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
            orders.push_back(order);
        }

        // Test that shortest_expiry mode returns orders by earliest expiry
        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            if let Some(order) = ctx
                .picker
                .select_next_pricing_order(&mut orders, OrderPricingPriority::ShortestExpiry)
            {
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
        let mut orders = VecDeque::new();

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
        orders.push_back(order1);

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
        orders.push_back(order2);

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
        orders.push_back(order3);

        // Test selection order
        let mut selected_order_indices = Vec::new();
        while !orders.is_empty() {
            if let Some(order) = ctx
                .picker
                .select_next_pricing_order(&mut orders, OrderPricingPriority::ShortestExpiry)
            {
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
            let mut orders = VecDeque::new();
            for i in 0..5 {
                let order = ctx
                    .generate_next_order(OrderParams { order_index: i, ..Default::default() })
                    .await;
                orders.push_back(order);
            }

            let mut selected_order_indices = Vec::new();
            while !orders.is_empty() {
                if let Some(order) =
                    ctx.picker.select_next_pricing_order(&mut orders, OrderPricingPriority::Random)
                {
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
        let orders = ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::ShortestExpiry);

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
                ctx.monitor.prioritize_orders(test_orders, OrderCommitmentPriority::Random);

            // Extract the ordering of all orders
            let order_ids: Vec<_> = test_orders.iter().map(|order| order.request.id).collect();
            all_orderings.insert(order_ids);
        }

        // Should see different orderings due to randomness
        assert!(all_orderings.len() > 1, "Random mode should produce different orderings");

        // Test that random mode produces different orderings
        let prioritized = ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::Random);

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
            ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::ShortestExpiry);

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
        let _prioritized_random =
            ctx.monitor.prioritize_orders(_prioritized_random, OrderCommitmentPriority::Random);

        // Test shortest expiry mode
        let prioritized_shortest =
            ctx.monitor.prioritize_orders(orders, OrderCommitmentPriority::ShortestExpiry);

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
}
