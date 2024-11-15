// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy_primitives::{Address, Signature};
use alloy_sol_types::{Eip712Domain, SolStruct};
use anyhow::{bail, Result};
use boundless_market::contracts::{EIP721DomainSaltless, ProvingRequest};
use risc0_zkvm::{sha::Digest, ReceiptClaim};
use serde::{Deserialize, Serialize};

/// Fulfillment contains a signed request, including offer and requirements,
/// that the prover has completed, and the journal committed
/// into the Merkle tree of the aggregated set of proofs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Fulfillment {
    pub request: ProvingRequest,
    pub signature: Vec<u8>,
    pub journal: Vec<u8>,
    pub require_payment: bool,
}

impl Fulfillment {
    pub fn verify_signature(&self, domain: &Eip712Domain) -> Result<()> {
        let hash = self.request.eip712_signing_hash(domain);
        let signature = Signature::try_from(self.signature.as_slice())?;
        // NOTE: This could be optimized by accepting the public key as input, checking it against
        // the address, and using it to verify the signature instead of recovering the
        // public key. It would save ~1M cycles.
        let recovered = signature.recover_address_from_prehash(&hash)?;
        let client_addr = self.request.client_address();
        if recovered != self.request.client_address() {
            bail!("Invalid signature: mismatched addr {recovered} - {client_addr}");
        }
        Ok(())
    }
    pub fn evaluate_requirements(&self) -> Result<()> {
        if !self.request.requirements.predicate.eval(&self.journal) {
            bail!("Predicate evaluation failed");
        }
        Ok(())
    }
    pub fn receipt_claim(&self) -> ReceiptClaim {
        let image_id = Digest::from_bytes(self.request.requirements.imageId.0);
        ReceiptClaim::ok(image_id, self.journal.clone())
    }
}

/// Input of the Assessor guest.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AssessorInput {
    // List of fulfillments that the prover has completed.
    pub fills: Vec<Fulfillment>,
    // The smart contract address for the market that will be posted to.
    // This smart contract address is used solely to construct the EIP-712 Domain
    // and complete signature checks on the requests.
    pub domain: EIP721DomainSaltless,
    // The address of the prover.
    pub prover_address: Address,
}

impl AssessorInput {
    pub fn to_vec(&self) -> Vec<u8> {
        let bytes = postcard::to_allocvec(self).unwrap();
        let length = bytes.len() as u32;
        let mut result = Vec::with_capacity(4 + bytes.len());
        result.extend_from_slice(&length.to_le_bytes());
        result.extend_from_slice(&bytes);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aggregation_set::{GuestInput, GuestOutput, SET_BUILDER_GUEST_ELF, SET_BUILDER_GUEST_ID};
    use alloy::{
        primitives::{aliases::U96, Address, B256},
        signers::local::PrivateKeySigner,
        sol_types::SolValue,
    };
    use boundless_market::contracts::{
        eip712_domain, Input, InputType, Offer, Predicate, PredicateType, ProvingRequest,
        Requirements,
    };
    use guest_assessor::ASSESSOR_GUEST_ELF;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::{
        default_executor,
        sha::{Digest, Digestible},
        ExecutorEnv, ExitCode, FakeReceipt, InnerReceipt, MaybePruned, Receipt,
    };

    fn proving_request(
        id: u32,
        signer: Address,
        image_id: B256,
        prefix: Vec<u8>,
    ) -> ProvingRequest {
        ProvingRequest::new(
            id,
            &signer,
            Requirements {
                imageId: image_id,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: prefix.into(),
                },
            },
            &"test".to_string(),
            Input { inputType: InputType::Url, data: Default::default() },
            Offer {
                minPrice: U96::from(1),
                maxPrice: U96::from(10),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockinStake: U96::from(0),
            },
        )
    }

    fn to_b256(digest: Digest) -> B256 {
        <[u8; 32]>::from(digest).into()
    }

    #[test]
    #[test_log::test]
    fn test_claim() {
        let signer = PrivateKeySigner::random();
        let proving_request = proving_request(1, signer.address(), B256::ZERO, vec![1]);
        let signature = proving_request.sign_request(&signer, Address::ZERO, 1).unwrap();

        let claim = Fulfillment {
            request: proving_request,
            signature: signature.as_bytes().to_vec(),
            journal: vec![1, 2, 3],
            require_payment: true,
        };

        claim.verify_signature(&eip712_domain(Address::ZERO, 1).alloy_struct()).unwrap();
        claim.evaluate_requirements().unwrap();
    }

    #[test]
    #[test_log::test]
    fn test_domain_serde() {
        let domain = eip712_domain(Address::ZERO, 1);
        let bytes = postcard::to_allocvec(&domain).unwrap();
        let domain2: EIP721DomainSaltless = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(domain, domain2);
    }

    fn setup_proving_request_and_signature(signer: &PrivateKeySigner) -> (ProvingRequest, Vec<u8>) {
        let request = proving_request(
            1,
            signer.address(),
            to_b256(ECHO_ID.into()),
            "test".as_bytes().to_vec(),
        );
        let signature =
            request.sign_request(&signer, Address::ZERO, 1).unwrap().as_bytes().to_vec();
        (request, signature)
    }

    fn echo(input: &str) -> Receipt {
        let env = ExecutorEnv::builder().write(&input.as_bytes()).unwrap().build().unwrap();

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

    fn singleton(assumption: Receipt) -> Receipt {
        let claim = assumption.claim().unwrap().value().unwrap();
        let guest_input = GuestInput::Singleton {
            self_image_id: Digest::from(SET_BUILDER_GUEST_ID),
            claim: claim.clone(),
        };
        let env = ExecutorEnv::builder()
            .write(&guest_input)
            .unwrap()
            .add_assumption(assumption)
            .build()
            .unwrap();
        let session = default_executor().execute(env, SET_BUILDER_GUEST_ELF).unwrap();
        assert_eq!(session.exit_code, ExitCode::Halted(0));
        let journal = &session.journal.bytes;
        let guest_output = GuestOutput::abi_decode(journal, true).unwrap();
        assert_eq!(guest_output.image_id(), Digest::from(SET_BUILDER_GUEST_ID));
        assert_eq!(guest_output.root(), claim.digest());
        Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(ReceiptClaim::ok(
                SET_BUILDER_GUEST_ID,
                MaybePruned::Pruned(journal.digest()),
            ))),
            journal.clone(),
        )
    }

    fn join(left: Receipt, right: Receipt) -> Receipt {
        let journal_left = left.clone().journal.bytes;
        let guest_output_left = GuestOutput::abi_decode(&journal_left, true).unwrap();
        let journal_right = right.clone().journal.bytes;
        let guest_output_right = GuestOutput::abi_decode(&journal_right, true).unwrap();

        let guest_input = GuestInput::Join {
            self_image_id: Digest::from(SET_BUILDER_GUEST_ID),
            left_set_root: guest_output_left.root(),
            right_set_root: guest_output_right.root(),
        };
        let env = ExecutorEnv::builder()
            .write(&guest_input)
            .unwrap()
            .add_assumption(left)
            .add_assumption(right)
            .build()
            .unwrap();
        let session = default_executor().execute(env, SET_BUILDER_GUEST_ELF).unwrap();
        assert_eq!(session.exit_code, ExitCode::Halted(0));
        let journal = &session.journal.bytes;

        let guest_output = GuestOutput::abi_decode(journal, true).unwrap();
        assert_eq!(guest_output.image_id(), Digest::from(SET_BUILDER_GUEST_ID));
        Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(ReceiptClaim::ok(
                SET_BUILDER_GUEST_ID,
                MaybePruned::Pruned(journal.digest()),
            ))),
            journal.clone(),
        )
    }

    fn assessor(claims: Vec<Fulfillment>, assumption_receipt: Receipt) {
        let assessor_input = AssessorInput {
            domain: eip712_domain(Address::ZERO, 1),
            fills: claims,
            prover_address: Address::ZERO,
        };
        let env = ExecutorEnv::builder()
            .write_slice(&assessor_input.to_vec())
            .add_assumption(assumption_receipt)
            .build()
            .unwrap();
        let session = default_executor().execute(env, ASSESSOR_GUEST_ELF).unwrap();
        assert_eq!(session.exit_code, ExitCode::Halted(0));
    }

    #[test]
    #[test_log::test]
    fn test_assessor_e2e_singleton() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a proving request
        let (request, signature) = setup_proving_request_and_signature(&signer);

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();

        // 3. Prove a singleton via the aggregator set
        let singleton_receipt = singleton(application_receipt);

        // 4. Prove the Assessor
        let claims = vec![Fulfillment { request, signature, journal, require_payment: true }];
        assessor(claims, singleton_receipt);
    }

    #[test]
    #[test_log::test]
    fn test_assessor_e2e_two_leaves() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a proving request
        let (request, signature) = setup_proving_request_and_signature(&signer);

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();
        let claim = Fulfillment { request, signature, journal, require_payment: true };

        // 3. Prove a singleton via the aggregator set
        let singleton_receipt = singleton(application_receipt);

        // 4. Prove the join of two leaves via the aggregator set, reusing the same singleton twice
        let join_receipt = join(singleton_receipt.clone(), singleton_receipt);

        // 4. Prove the Assessor reusing the same leaf twice
        let claims = vec![claim.clone(), claim];
        assessor(claims, join_receipt);
    }
}
