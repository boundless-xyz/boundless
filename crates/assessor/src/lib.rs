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

//! Assessor is a guest that verifies the fulfillment of a request.

#![deny(missing_docs)]

use alloy_primitives::{Address, PrimitiveSignature, B256};
use alloy_sol_macro::sol;
use alloy_sol_types::{Eip712Domain, SolStruct};
use anyhow::{bail, ensure, Result};
use boundless_market::contracts::{EIP721DomainSaltless, ProofRequest};
use risc0_steel::{
    ethereum::EthBlockHeader, Commitment, Contract, EvmBlockHeader, EvmEnv, EvmInput, StateDb,
    SteelVerifier,
};
use risc0_zkvm::{sha::Digest, ReceiptClaim};
use serde::{Deserialize, Serialize};

// Re-export steel to make version consistency easier.
pub use risc0_steel as steel;

/// Authorization for the request, which may either be an ECDSA signature or an ERC-1271 smart
/// contract signature. In both cases, EIP-712 is used for hashing the request struct.
#[derive(Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub enum Authorization<H> {
    /// An ECDSA signature authorizing the request from an externally owned account (EOA).
    ECDSA(Vec<u8>),
    /// An ERC-1271 smart contract signature, along with associated data required for verification
    /// in the zkVM using [Steel][risc0_steel].
    ERC1271 {
        /// Data used as a `signature` argument in calling `ERC1271.isValidSignature`.
        signature: Vec<u8>,
        /// [EvmInput] containing state inclusion proofs needed to verify the signature within the
        /// guest using [Steel][risc0_steel].
        steel_input: EvmInput<H>,
    },
}

/// Fulfillment contains a signed request, including offer and requirements,
/// that the prover has completed, and the journal committed
/// into the Merkle tree of the aggregated set of proofs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Fulfillment {
    /// The request that was fulfilled.
    pub request: ProofRequest,
    /// The journal of the request.
    pub journal: Vec<u8>,
    /// Whether the fulfillment requires payment.
    ///
    /// When set to true, the fulfill transaction will revert if the payment conditions are not met (e.g. the request is locked to a different prover address)
    pub require_payment: bool,
}

sol! {
    interface IERC1271 {
        function isValidSignature(
            bytes32 hash,
            bytes memory signature)
            public
            view
            returns (bytes4 magicValue);
    }
}

/// The magic value returned by ERC-1271 contracts to indicate a valid signature.
const ERC1271_MAGICVALUE: [u8; 4] = [0x16, 0x26, 0xba, 0x7e];

impl Fulfillment {
    // TODO: Change this to use a thiserror error type.
    /// Verifies the authorization on the request, handling ECDSA and ERC-1271 smart contract signatures.
    ///
    /// If ERC-1271 smart contract signatures are used, the given [SteelVerifier] is used to check
    /// that the block used for the signature check is included in the committed chain.
    ///
    /// Returns the EIP-712 hash of the [ProofRequest] struct.
    pub fn verify_auth<H>(
        &self,
        domain: &Eip712Domain,
        authorization: Authorization<H>,
        steel_verifier: &SteelVerifier<&EvmEnv<StateDb, H, Commitment>>,
    ) -> Result<[u8; 32]>
    where
        H: EvmBlockHeader,
    {
        let hash = self.request.eip712_signing_hash(domain);

        if self.request.is_smart_contract_signed()? {
            let Authorization::ERC1271 { signature, steel_input } = authorization else {
                bail!("wrong signature type provided for smart contract authorized request");
            };
            self.verify_erc1271_inner(hash, signature, steel_input, steel_verifier)?;
        } else {
            let Authorization::ECDSA(signature) = authorization else {
                bail!("wrong signature type provided for EOA authorized request");
            };
            self.verify_ecdsa_inner(hash, &signature)?;
        }
        Ok(hash.into())
    }

    /// Verify authorization of the request using an ECDSA signature from an EOA.
    pub fn verify_ecdsa(&self, domain: &Eip712Domain, signature: &[u8]) -> Result<[u8; 32]> {
        ensure!(self.request.is_smart_contract_signed()?, "request authorization is not ECDSA");
        let hash = self.request.eip712_signing_hash(domain);
        self.verify_ecdsa_inner(hash, signature)?;
        Ok(hash.into())
    }

    fn verify_ecdsa_inner(&self, hash: B256, signature: &[u8]) -> Result<()> {
        let signature = PrimitiveSignature::try_from(signature)?;
        // NOTE: This could be optimized by accepting the public key as input, checking it against
        // the address, and using it to verify the signature instead of recovering the
        // public key. It would save ~1M cycles.
        let recovered = signature.recover_address_from_prehash(&hash)?;
        let client_addr = self.request.client_address()?;
        if recovered != client_addr {
            bail!("Invalid signature: mismatched addr {recovered} - {client_addr}");
        }
        Ok(())
    }

    /// Verify authorization of the request using an ERC-1271 smart contract signature.
    pub fn verify_erc1271<H>(
        &self,
        domain: &Eip712Domain,
        signature: Vec<u8>,
        steel_input: EvmInput<H>,
        steel_verifier: &SteelVerifier<&EvmEnv<StateDb, H, Commitment>>,
    ) -> Result<[u8; 32]>
    where
        H: EvmBlockHeader,
    {
        ensure!(
            self.request.is_smart_contract_signed()?,
            "request authorization is not smart contract signature"
        );
        let hash = self.request.eip712_signing_hash(domain);
        self.verify_erc1271_inner(hash, signature, steel_input, steel_verifier)?;
        Ok(hash.into())
    }

    fn verify_erc1271_inner<H>(
        &self,
        hash: B256,
        signature: Vec<u8>,
        steel_input: EvmInput<H>,
        steel_verifier: &SteelVerifier<&EvmEnv<StateDb, H, Commitment>>,
    ) -> Result<()>
    where
        H: EvmBlockHeader,
    {
        let env = steel_input.into_env();

        // Ensure that the EvmEnv exists in the chain with the SteelVerifier.
        steel_verifier.verify(env.commitment());

        let call = IERC1271::isValidSignatureCall { hash, signature: signature.into() };
        let returns =
            Contract::new(self.request.client_address()?, &env).call_builder(&call).call();
        if returns.magicValue != ERC1271_MAGICVALUE {
            bail!("Invalid smart contract signature; addr = {}", self.request.client_address()?);
        }
        Ok(())
    }

    /// Evaluates the requirements of the request.
    pub fn evaluate_requirements(&self) -> Result<()> {
        if !self.request.requirements.predicate.eval(&self.journal) {
            bail!("Predicate evaluation failed");
        }
        Ok(())
    }

    /// Returns a [ReceiptClaim] for the fulfillment.
    pub fn receipt_claim(&self) -> ReceiptClaim {
        let image_id = Digest::from_bytes(self.request.requirements.imageId.0);
        ReceiptClaim::ok(image_id, self.journal.clone())
    }
}

/// Input of the Assessor guest.
#[derive(Clone, Deserialize, Serialize)]
pub struct AssessorInput {
    /// List of fulfillments that the prover has completed.
    pub fills: Vec<Fulfillment>,
    /// [Authorization] corresponding to the list of fulfillments.
    // NOTE: Seperated from the [Fulfillment] struct because [Authorization] is consumed upon
    // verification.
    pub authorization: Vec<Authorization<EthBlockHeader>>,
    /// [EvmInput] containing inclusion proofs needed to show each ERC-1271 signature is part of
    /// the same chain. This authorization root is used to derive the commitment that is placed in
    /// the journal and verified on-chain.
    pub authorization_root: Option<EvmInput<EthBlockHeader>>,
    /// EIP-712 domain checking the signature of the request.
    ///
    /// The EIP-712 domain contains the chain ID and smart contract address.
    /// This smart contract address is used solely to construct the EIP-712 Domain
    /// and complete signature checks on the requests.
    pub domain: EIP721DomainSaltless,
    /// The address of the prover.
    pub prover_address: Address,
}

impl AssessorInput {
    /// Serializes the AssessorInput to a Vec<u8> using postcard.
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
    use std::sync::LazyLock;

    use super::*;
    use boundless_market::contracts::{
        eip712_domain, request_id_to_parts, Input, InputType, Offer, Predicate, PredicateType,
        ProofRequest, Requirements,
    };
    use guest_assessor::ASSESSOR_GUEST_ELF;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use revm::primitives::SpecId;
    use risc0_steel::{
        alloy::{
            primitives::{Address, B256, U256},
            providers::ProviderBuilder,
            signers::local::PrivateKeySigner,
        },
        host::BlockNumberOrTag,
        Contract,
    };
    use risc0_steel::{config::ChainSpec, ethereum::EthEvmEnv};
    use risc0_zkvm::{
        default_executor,
        sha::{Digest, Digestible},
        ExecutorEnv, ExitCode, FakeReceipt, InnerReceipt, MaybePruned, Receipt,
    };

    const ANVIL_CHAIN_SPEC: LazyLock<ChainSpec> =
        LazyLock::new(|| ChainSpec::new_single(31337, SpecId::CANCUN)); // 20 = Cancun

    fn proving_request(id: u32, signer: Address, image_id: B256, prefix: Vec<u8>) -> ProofRequest {
        ProofRequest::new(
            id,
            &signer,
            Requirements::new(
                Digest::from_bytes(image_id.0),
                Predicate { predicateType: PredicateType::PrefixMatch, data: prefix.into() },
            ),
            "test",
            Input { inputType: InputType::Url, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockTimeout: 1000,
                lockStake: U256::from(0),
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

        let claim =
            Fulfillment { request: proving_request, journal: vec![1, 2, 3], require_payment: true };

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

    async fn setup_proving_request_and_signature(
        signer: &PrivateKeySigner,
    ) -> (ProofRequest, Vec<u8>) {
        let request = proving_request(
            1,
            signer.address(),
            to_b256(ECHO_ID.into()),
            "test".as_bytes().to_vec(),
        );
        let signature =
            request.sign_request(signer, Address::ZERO, 1).await.unwrap().as_bytes().to_vec();
        (request, signature)
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

    // TODO: Test using ERC-1271
    /*
        // Create an EVM environment from that provider defaulting to the latest block.
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .on_anvil_with_wallet_and_config(|anvil| anvil.args(["--hardfork", "cancun"]));
        let mut env = EthEvmEnv::builder()
            .provider(provider.clone())
            .block_number_or_tag(BlockNumberOrTag::Latest)
            .build()
            .await
            .unwrap()
            .with_chain_spec(&ANVIL_CHAIN_SPEC);

        for claim in claims {
            let request_id = claim.request.id;
            let (addr, _index, is_contract_sig) = request_id_to_parts(request_id);
            if is_contract_sig {
                // Prepare the function call
                let hash = claim.request.eip712_signing_hash(&assessor_input.domain.alloy_struct());

                let call =
                    IERC1271::isValidSignatureCall { hash, signature: claim.signature.into() };

                // Preflight the call to prepare the input that is required to execute the function in
                // the guest without RPC access. It also returns the result of the call.
                let mut contract = Contract::preflight(addr, &mut env);
                contract.call_builder(&call).call().await.expect("invalid signature");
            }
        }
    */

    #[must_use] // DO NOT MERGE: Is this doing anything?
    async fn assessor<H: EvmBlockHeader>(
        claims: Vec<Fulfillment>,
        auth: Vec<Authorization<H>>,
        auth_root: Option<EvmInput<H>>,
        receipts: Vec<Receipt>,
    ) {
        let assessor_input = AssessorInput {
            domain: eip712_domain(Address::ZERO, 1),
            fills: claims.clone(),
            authorization: auth,
            authorization_root: auth_root,
            prover_address: Address::ZERO,
        };

        let evm_input = env.into_input().await.unwrap();
        let mut env_builder = ExecutorEnv::builder();
        env_builder.write_slice(&assessor_input.to_vec());
        for receipt in receipts {
            env_builder.add_assumption(receipt);
        }
        let env = env_builder.build().unwrap();

        let session = default_executor().execute(env, ASSESSOR_GUEST_ELF).unwrap();
        assert_eq!(session.exit_code, ExitCode::Halted(0));
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_assessor_e2e_singleton() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a request
        let (request, signature) = setup_proving_request_and_signature(&signer).await;

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();

        // 3. Prove the Assessor
        let claims = vec![Fulfillment { request, journal, require_payment: true }];
        assessor(claims, vec![Authorization::ECDSA(signature)], None, vec![application_receipt])
            .await;
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_assessor_e2e_two_leaves() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a request
        let (request, signature) = setup_proving_request_and_signature(&signer).await;

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();
        let claim = Fulfillment { request, journal, require_payment: true };

        // 3. Prove the Assessor reusing the same leaf twice
        let claims = vec![claim.clone(), claim];
        let auth = vec![Authorization::ECDSA(signature.clone()), Authorization::ECDSA(signature)];
        assessor(claims, auth, None, vec![application_receipt.clone(), application_receipt]).await;
    }
}
