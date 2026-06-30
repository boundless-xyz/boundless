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

//! Helpers for constructing the batched fulfillment payloads submitted to the
//! BoundlessMarket contract: [SlimRequest] and the router `assessorSeal`.

use alloy::primitives::{keccak256, Bytes, FixedBytes};
use alloy_sol_types::SolStruct;

use super::{ProofRequest, SlimRequest};

impl SlimRequest {
    /// Derives the [SlimRequest] bound to a full [ProofRequest].
    ///
    /// The predicate, callback and selector are carried verbatim (the market reads them at
    /// fulfill time); `imageUrl`, `input` and `offer` are reduced to the same EIP-712 digests
    /// the market uses to reconstruct the request digest and check it against the value stored
    /// at lock time.
    pub fn from_request(request: &ProofRequest) -> Self {
        SlimRequest {
            id: request.id,
            predicate: request.requirements.predicate.clone(),
            callback: request.requirements.callback.clone(),
            selector: request.requirements.selector,
            imageUrlHash: keccak256(request.imageUrl.as_bytes()),
            inputDigest: request.input.eip712_hash_struct(),
            offerDigest: request.offer.eip712_hash_struct(),
        }
    }
}

/// Assembles a router `assessorSeal`: the 4-byte router assessor selector followed by the
/// inner per-class seal (the assessor set-inclusion proof).
pub fn assessor_seal(selector: FixedBytes<4>, inner_seal: impl AsRef<[u8]>) -> Bytes {
    let inner = inner_seal.as_ref();
    let mut bytes = Vec::with_capacity(4 + inner.len());
    bytes.extend_from_slice(selector.as_slice());
    bytes.extend_from_slice(inner);
    Bytes::from(bytes)
}
