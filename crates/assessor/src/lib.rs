// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Assessor is a guest that verifies the fulfillment of a request.

#![deny(missing_docs)]

use alloy_primitives::{Address, Keccak256, Signature, SignatureError};
use alloy_sol_types::{Eip712Domain, SolStruct};
use boundless_market::contracts::{
    EIP712DomainSaltless, FulfillmentData, FulfillmentDataError, Predicate, PredicateError,
    ProofRequest, RequestError,
};
use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};

/// Errors that may occur in the assessor.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// Deserialization error originating from [postcard].
    #[error("postcard deserialize error: {0}")]
    PostcardDeserializeError(#[from] postcard::Error),

    /// Signature parsing or verification error.
    #[error("signature error: {0}")]
    AlloySignatureError(#[from] SignatureError),

    /// Malformed proof request error.
    #[error("proof request error: {0}")]
    RequestError(#[from] RequestError),

    /// Signature verification error.
    #[error("invalid signature: mismatched addr {recovered_addr} - {expected_addr}")]
    SignatureVerificationError {
        /// Address recovered when trying to verify the ECDSA signature.
        recovered_addr: Address,
        /// Address expected from decoding the [ProofRequest].
        expected_addr: Address,
    },

    /// Signature is non-canonical.
    #[error("invalid signature: not normalized s-value")]
    SignatureNonCanonicalError,

    /// Predicate evaluation failure from [ProofRequest] [Requirements]
    #[error("fulfillment requirements evaluation failed")]
    RequirementsEvaluationError,

    /// Fulfillment data malformed or invalid
    #[error("fulfillment data error")]
    FulfillmentDataError(#[from] FulfillmentDataError),

    /// Predicate error due to malformed data
    #[error("predicate error: {0}")]
    PredicateError(#[from] PredicateError),

    /// Fulfillment data type is not recognized.
    #[error("fulfillment data type is not recognized {0:?}")]
    UnknownFulfillmentDataType(FulfillmentData),

    /// Predicate type is not recognized.
    #[error("predicate type is not recognized {0:?}")]
    UnknownPredicateType(Predicate),

    /// Invalid combination of fulfillment type and predicate type
    #[error(
        "invalid combination of fulfillment type and predicate type, cant compute claim digest"
    )]
    InvalidFulfillmentPredicateCombination,
}

/// Fulfillment contains a signed request, including offer and requirements,
/// that the prover has completed, and the journal committed
/// into the Merkle tree of the aggregated set of proofs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Fulfillment {
    /// The request that was fulfilled.
    pub request: ProofRequest,
    /// The EIP-712 signature over the request.
    pub signature: Vec<u8>,
    /// The fulfillment data of the request.
    pub fulfillment_data: FulfillmentData,
}

impl Fulfillment {
    // TODO: Change this to use a thiserror error type.
    /// Verifies the signature of the request.
    pub fn verify_signature(&self, domain: &Eip712Domain) -> Result<[u8; 32], Error> {
        let hash = self.request.eip712_signing_hash(domain);
        let signature = Signature::try_from(self.signature.as_slice())?;
        // Check if the signature is non-canonical.
        if signature.normalize_s().is_some() {
            return Err(Error::SignatureNonCanonicalError);
        }
        // NOTE: This could be optimized by accepting the public key as input, checking it against
        // the address, and using it to verify the signature instead of recovering the
        // public key. It would save ~1M cycles.
        let recovered = signature.recover_address_from_prehash(&hash)?;
        let client_addr = self.request.client_address();
        if recovered != client_addr {
            return Err(Error::SignatureVerificationError {
                recovered_addr: recovered,
                expected_addr: client_addr,
            });
        }
        Ok(hash.into())
    }

    /// Evaluates the requirements of the request and returns the claim digest.
    pub fn evaluate_requirements(&self) -> Result<Digest, Error> {
        let predicate = Predicate::try_from(self.request.requirements.predicate.clone())?;
        predicate.eval(&self.fulfillment_data).ok_or(Error::RequirementsEvaluationError)
    }
}

/// Input of the Assessor guest.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AssessorInput {
    /// List of fulfillments that the prover has completed.
    pub fills: Vec<Fulfillment>,
    /// EIP-712 domain checking the signature of the request.
    ///
    /// The EIP-712 domain contains the chain ID and smart contract address.
    /// This smart contract address is used solely to construct the EIP-712 Domain
    /// and complete signature checks on the requests.
    pub domain: EIP712DomainSaltless,
    /// The address of the prover.
    pub prover_address: Address,
}

impl AssessorInput {
    /// Serialize the [AssessorInput] to a bytes vector.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(&self).unwrap()
    }

    /// Deserialize the [AssessorInput] from a slice of bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

/// Processes a vector of leaves to compute the Merkle root.
pub fn process_tree(values: Vec<Digest>) -> Digest {
    let n = values.len();
    if n == 0 {
        panic!("process_tree: empty input");
    }
    if n == 1 {
        return values[0];
    }
    let mut n = values.len();
    let mut leaves = values.clone();
    while n > 1 {
        let next_level_length = n.div_ceil(2);
        for i in 0..(n / 2) {
            leaves[i] = commutative_keccak256(&leaves[2 * i], &leaves[2 * i + 1]);
        }
        if n % 2 == 1 {
            leaves[next_level_length - 1] = leaves[n - 1];
        }
        n = next_level_length;
    }
    leaves[0]
}

/// Computes the hash of a sorted pair of [Digest].
fn commutative_keccak256(a: &Digest, b: &Digest) -> Digest {
    let mut hasher = Keccak256::new();
    if a.as_bytes() < b.as_bytes() {
        hasher.update(a.as_bytes());
        hasher.update(b.as_bytes());
    } else {
        hasher.update(b.as_bytes());
        hasher.update(a.as_bytes());
    }
    hasher.finalize().0.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{Address, B256, U256},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        eip712_domain, Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType,
        Requirements,
    };
    use guest_assessor::ASSESSOR_GUEST_ELF;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::{
        default_executor,
        sha::{Digest, Digestible},
        ExecutorEnv, ExitCode, FakeReceipt, InnerReceipt, MaybePruned, Receipt, ReceiptClaim,
    };

    fn proving_request(id: u32, signer: Address, image_id: B256, prefix: Vec<u8>) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(signer, id),
            Requirements::new(Predicate::prefix_match(Digest::from_bytes(image_id.0), prefix)),
            "http://test.null",
            RequestInput { inputType: RequestInputType::Url, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                rampUpStart: 1741386831,
                timeout: 1000,
                rampUpPeriod: 1,
                lockTimeout: 1000,
                lockCollateral: U256::from(0),
            },
        )
    }

    fn claim_digest_request(id: u32, signer: Address, claim_digest: Digest) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(signer, id),
            Requirements::new(Predicate::claim_digest_match(claim_digest)),
            "http://test.null",
            RequestInput { inputType: RequestInputType::Url, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                rampUpStart: 1741386831,
                timeout: 1000,
                rampUpPeriod: 1,
                lockTimeout: 1000,
                lockCollateral: U256::from(0),
            },
        )
    }

    fn to_b256(digest: Digest) -> B256 {
        <[u8; 32]>::from(digest).into()
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_claim() {
        let signer = PrivateKeySigner::random();
        let proving_request = proving_request(1, signer.address(), B256::ZERO, vec![1]);
        let signature = proving_request.sign_request(&signer, Address::ZERO, 1).await.unwrap();

        let claim = Fulfillment {
            request: proving_request,
            signature: signature.as_bytes().to_vec(),
            fulfillment_data: FulfillmentData::from_image_id_and_journal(
                Digest::from_bytes(B256::ZERO.0),
                vec![1u8],
            ),
        };

        claim.verify_signature(&eip712_domain(Address::ZERO, 1).alloy_struct()).unwrap();
        claim.evaluate_requirements().unwrap();
    }

    #[test]
    #[test_log::test]
    fn test_domain_serde() {
        let domain = eip712_domain(Address::ZERO, 1);
        let bytes = postcard::to_allocvec(&domain).unwrap();
        let domain2: EIP712DomainSaltless = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(domain, domain2);
    }

    async fn setup_proving_request_and_signature(
        signer: &PrivateKeySigner,
    ) -> (ProofRequest, B256, Vec<u8>) {
        let image_id = to_b256(ECHO_ID.into());
        let request = proving_request(1, signer.address(), image_id, "test".as_bytes().to_vec());
        let signature =
            request.sign_request(signer, Address::ZERO, 1).await.unwrap().as_bytes().to_vec();
        (request, image_id, signature)
    }

    fn echo(input: &str) -> Receipt {
        let env = ExecutorEnv::builder().write_slice(input.as_bytes()).build().unwrap();

        // TODO: Change this to use SessionInfo::claim or another method.
        // See https://github.com/risc0/risc0/issues/2267.
        let session = default_executor().execute(env, ECHO_ELF).unwrap();
        Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(ReceiptClaim::ok(
                ECHO_ID,
                MaybePruned::Pruned(session.journal.digest()),
            ))),
            session.journal.bytes,
        )
    }

    fn assessor(claims: Vec<Fulfillment>) {
        let assessor_input = AssessorInput {
            domain: eip712_domain(Address::ZERO, 1),
            fills: claims,
            prover_address: Address::ZERO,
        };
        let mut env_builder = ExecutorEnv::builder();
        env_builder.write_frame(&assessor_input.encode());

        let env = env_builder.build().unwrap();
        let session = default_executor().execute(env, ASSESSOR_GUEST_ELF).unwrap();
        assert_eq!(session.exit_code, ExitCode::Halted(0));
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_assessor_e2e_singleton() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a request
        let (request, image_id, signature) = setup_proving_request_and_signature(&signer).await;

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();

        // 3. Prove the Assessor
        let claims = vec![Fulfillment {
            request,
            signature,
            fulfillment_data: FulfillmentData::from_image_id_and_journal(*image_id, journal),
        }];
        assessor(claims);
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_assessor_e2e_two_leaves() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a request
        let (request, image_id, signature) = setup_proving_request_and_signature(&signer).await;

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();
        let claim = Fulfillment {
            request,
            signature,
            fulfillment_data: FulfillmentData::from_image_id_and_journal(*image_id, journal),
        };

        // 3. Prove the Assessor reusing the same leaf twice
        let claims = vec![claim.clone(), claim];
        assessor(claims);
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_assessor_claim_digest_match() {
        let signer = PrivateKeySigner::random();
        let image_id = to_b256(ECHO_ID.into());
        let input = "test";
        let claim_digest = ReceiptClaim::ok(*image_id, input.as_bytes().to_vec()).digest();

        // 1. Mock and sign a request
        let request = claim_digest_request(1, signer.address(), claim_digest);
        let signature =
            request.sign_request(&signer, Address::ZERO, 1).await.unwrap().as_bytes().to_vec();
        // 2. Prove the request via the application guest
        let application_receipt = echo(input);
        let journal = application_receipt.journal.bytes.clone();

        // 3. Prove the Assessor. For ClaimDigestMatch predicate, we can fulfill the request either
        // with the journal or the claim digest.
        let claims = vec![
            Fulfillment {
                request: request.clone(),
                signature: signature.clone(),
                fulfillment_data: FulfillmentData::from_image_id_and_journal(*image_id, journal),
            },
            Fulfillment { request, signature, fulfillment_data: FulfillmentData::None },
        ];
        assessor(claims);
    }
}
