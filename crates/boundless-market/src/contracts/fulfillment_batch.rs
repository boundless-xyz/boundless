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

use alloy::primitives::{keccak256, Address, Bytes, FixedBytes, B256};
use alloy::signers::Signer;
use alloy_sol_types::{eip712_domain, Eip712Domain, SolStruct};

use super::{Fulfillment, ProofRequest, SlimRequest};

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
/// inner per-class seal (e.g. the assessor set-inclusion proof, or the OnChainAssessor
/// prover signature).
pub fn assessor_seal(selector: FixedBytes<4>, inner_seal: impl AsRef<[u8]>) -> Bytes {
    let inner = inner_seal.as_ref();
    let mut bytes = Vec::with_capacity(4 + inner.len());
    bytes.extend_from_slice(selector.as_slice());
    bytes.extend_from_slice(inner);
    Bytes::from(bytes)
}

alloy::sol! {
    /// EIP-712 authorization a prover signs to bind a fulfillment batch to itself,
    /// verified on-chain by the `OnChainAssessor` adapter.
    ///
    /// The type string MUST match `OnChainAssessor.FULFILLMENT_BATCH_AUTH_TYPE`:
    /// `FulfillmentBatchAuth(address prover,bytes32[] requestDigests,bytes32[] claimDigests)`.
    struct FulfillmentBatchAuth {
        address prover;
        bytes32[] requestDigests;
        bytes32[] claimDigests;
    }
}

/// The EIP-712 domain of the `OnChainAssessor` adapter deployed at `address` on `chain_id`.
///
/// Mirrors the `DOMAIN_SEPARATOR` the adapter pins in its constructor
/// (`name = "OnChainAssessor"`, `version = "1"`, chain id, verifying contract).
pub fn onchain_assessor_eip712_domain(address: Address, chain_id: u64) -> Eip712Domain {
    eip712_domain! {
        name: "OnChainAssessor",
        version: "1",
        chain_id: chain_id,
        verifying_contract: address,
    }
}

/// Computes the EIP-712 signing hash a prover must sign to authorize a fulfillment batch
/// for the `OnChainAssessor` adapter.
///
/// `request_digests` and `claim_digests` must be in the same order as the batch's fills.
pub fn fulfillment_batch_auth_signing_hash(
    domain: &Eip712Domain,
    prover: Address,
    request_digests: &[B256],
    claim_digests: &[B256],
) -> B256 {
    FulfillmentBatchAuth {
        prover,
        requestDigests: request_digests.to_vec(),
        claimDigests: claim_digests.to_vec(),
    }
    .eip712_signing_hash(domain)
}

/// Signs the [FulfillmentBatchAuth] for the `OnChainAssessor` adapter and returns the
/// 65-byte ECDSA signature — the inner seal that [assessor_seal] prepends the assessor
/// selector to.
pub async fn sign_fulfillment_batch_auth(
    signer: &impl Signer,
    domain: &Eip712Domain,
    prover: Address,
    request_digests: &[B256],
    claim_digests: &[B256],
) -> Result<Bytes, alloy::signers::Error> {
    let hash = fulfillment_batch_auth_signing_hash(domain, prover, request_digests, claim_digests);
    Ok(Bytes::from(signer.sign_hash(&hash).await?.as_bytes().to_vec()))
}

/// Builds the complete `OnChainAssessor` assessor seal for one fulfillment batch: derives the
/// per-fill request digests (from `market_domain`) and claim digests (from `fulfillments`), signs
/// the EIP-712 [FulfillmentBatchAuth], and frames the 65-byte signature behind the router
/// `selector`. This is the single entry point a backend calls to produce an on-chain assessor seal.
///
/// `requests` and `fulfillments` must be in the same (fill) order. `assessor_address` is the
/// deployed `OnChainAssessor` adapter (its EIP-712 `verifyingContract`); `chain_id` must be the
/// deployment chain.
#[allow(clippy::too_many_arguments)]
pub async fn build_onchain_assessor_seal(
    signer: &impl Signer,
    selector: FixedBytes<4>,
    assessor_address: Address,
    chain_id: u64,
    market_domain: &Eip712Domain,
    prover: Address,
    requests: &[ProofRequest],
    fulfillments: &[Fulfillment],
) -> Result<Bytes, alloy::signers::Error> {
    let request_digests: Vec<B256> =
        requests.iter().map(|request| request.eip712_signing_hash(market_domain)).collect();
    let claim_digests: Vec<B256> = fulfillments.iter().map(|fill| fill.claimDigest).collect();
    let domain = onchain_assessor_eip712_domain(assessor_address, chain_id);
    let signature =
        sign_fulfillment_batch_auth(signer, &domain, prover, &request_digests, &claim_digests)
            .await?;
    Ok(assessor_seal(selector, &signature))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, Signature};
    use alloy::signers::local::PrivateKeySigner;

    /// Pins the EIP-712 type string against `OnChainAssessor.FULFILLMENT_BATCH_AUTH_TYPE`
    /// so a silent rename of either side trips this test.
    #[test]
    fn fulfillment_batch_auth_type_matches_contract() {
        assert_eq!(
            FulfillmentBatchAuth::eip712_encode_type(),
            "FulfillmentBatchAuth(address prover,bytes32[] requestDigests,bytes32[] claimDigests)"
        );
    }

    /// The signed assessor seal is `selector ‖ 65-byte sig` and recovers to the prover over
    /// the same signing hash the contract reconstructs.
    #[tokio::test]
    async fn fulfillment_batch_auth_seal_roundtrips() {
        let signer: PrivateKeySigner =
            "6f142508b4eea641e33cb2a0161221105086a84584c74245ca463a49effea30b".parse().unwrap();
        let prover = signer.address();
        let assessor_addr = address!("00000000000000000000000000000000000000aa");
        let chain_id = 31337u64;
        let domain = onchain_assessor_eip712_domain(assessor_addr, chain_id);

        let request_digests = vec![B256::repeat_byte(0x11), B256::repeat_byte(0x22)];
        let claim_digests = vec![B256::repeat_byte(0x33), B256::repeat_byte(0x44)];
        let selector = FixedBytes::<4>::from([0x00, 0x00, 0x00, 0x22]);

        let sig =
            sign_fulfillment_batch_auth(&signer, &domain, prover, &request_digests, &claim_digests)
                .await
                .unwrap();

        let seal = assessor_seal(selector, &sig);
        assert_eq!(seal.len(), 4 + 65, "seal is selector(4) + ECDSA sig(65)");
        assert_eq!(&seal[..4], selector.as_slice());

        let hash =
            fulfillment_batch_auth_signing_hash(&domain, prover, &request_digests, &claim_digests);
        let recovered =
            Signature::try_from(sig.as_ref()).unwrap().recover_address_from_prehash(&hash).unwrap();
        assert_eq!(recovered, prover);
    }
}
