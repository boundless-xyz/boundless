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
    /// Default implementation of order prioritization logic.
    ///
    /// This function provides the core prioritization strategies that can be used
    /// by the OrderMonitor method. It implements the basic strategies without
    /// requiring access to monitor state.
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
}
