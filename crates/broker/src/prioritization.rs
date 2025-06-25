// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    config::OrderCommitmentPriority, order_monitor::OrderMonitor, FulfillmentType, OrderRequest,
};
use rand::seq::SliceRandom;
use std::sync::Arc;

/// Default implementation of order prioritization logic.
///
/// This function provides the core prioritization strategies that can be used
/// by the OrderMonitor method. It implements the basic strategies without
/// requiring access to monitor state.
impl<P> OrderMonitor<P> {
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
