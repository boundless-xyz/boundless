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
