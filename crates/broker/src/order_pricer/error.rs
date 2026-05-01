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

//! Error type alias and skip-code constants for the order pricer service.

use crate::errors::CodedError;
use crate::OrderPricingError;

// Broker-specific skip codes for checks that live in OrderPicker (not in pricing logic).
pub(crate) const SKIP_NOT_IN_ALLOWLIST: &str = "[S-OP-001]";
pub(crate) const SKIP_IN_DENYLIST: &str = "[S-OP-002]";
pub(crate) const SKIP_UNSUPPORTED_SELECTOR_BROKER: &str = "[S-OP-003]";
pub(crate) const SKIP_ALREADY_LOCKED: &str = "[S-OP-004]";
pub(crate) const SKIP_ALREADY_FULFILLED: &str = "[S-OP-005]";
pub(crate) const SKIP_GAS_EXCEEDS_MAX_PRICE: &str = "[S-OP-006]";
pub(crate) const SKIP_GAS_EXCEEDS_BALANCE: &str = "[S-OP-007]";
pub(crate) const SKIP_INSUFFICIENT_COLLATERAL: &str = "[S-OP-008]";
pub(crate) const SKIP_FETCH_ERROR: &str = "[S-OP-009]";
pub(crate) const SKIP_PRICING_ERROR: &str = "[S-OP-010]";

pub(crate) type OrderPricerErr = OrderPricingError;

// `coded_error_impl!` doesn't fit here: OrderPricingError is a foreign type
// (defined in boundless_market::prover_utils) and is `#[non_exhaustive]`, so
// the match needs a wildcard arm. Keep the manual impl.
impl CodedError for OrderPricingError {
    fn code(&self) -> &str {
        match self {
            OrderPricingError::FetchInputErr(_) => "[B-OP-001]",
            OrderPricingError::FetchImageErr(_) => "[B-OP-002]",
            OrderPricingError::RequestError(_) => "[B-OP-004]",
            OrderPricingError::RpcErr(_) => "[B-OP-005]",
            OrderPricingError::UnexpectedErr(_) => "[B-OP-500]",
            _ => "[B-OP-500]",
        }
    }
}
