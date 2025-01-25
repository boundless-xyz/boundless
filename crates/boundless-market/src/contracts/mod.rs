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

use std::borrow::Cow;
#[cfg(not(target_os = "zkvm"))]
use std::str::FromStr;

#[cfg(not(target_os = "zkvm"))]
use alloy::{
    contract::Error as ContractErr,
    primitives::{PrimitiveSignature, SignatureError},
    signers::Signer,
    sol_types::{Error as DecoderErr, SolInterface, SolStruct},
    transports::TransportError,
};
use alloy_primitives::{aliases::U160, Address, Bytes, B256, U256};
use alloy_sol_types::{eip712_domain, Eip712Domain};
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "zkvm"))]
use std::time::Duration;
#[cfg(not(target_os = "zkvm"))]
use thiserror::Error;
#[cfg(not(target_os = "zkvm"))]
use token::IHitPoints::{self, IHitPointsErrors};
use url::Url;

use risc0_zkvm::sha::Digest;

#[cfg(not(target_os = "zkvm"))]
pub use risc0_ethereum_contracts::{encode_seal, IRiscZeroSetVerifier};

#[cfg(not(target_os = "zkvm"))]
const TXN_CONFIRM_TIMEOUT: Duration = Duration::from_secs(45);

// boundless_market_generated.rs contains the Boundless contract types
// with alloy derive statements added.
// See the build.rs script in this crate for more details.
include!(concat!(env!("OUT_DIR"), "/boundless_market_generated.rs"));
pub use boundless_market_contract::*;

#[allow(missing_docs)]
#[cfg(not(target_os = "zkvm"))]
pub mod token {
    use alloy::{
        primitives::{Address, PrimitiveSignature},
        signers::Signer,
        sol_types::SolStruct,
    };
    use alloy_sol_types::eip712_domain;
    use anyhow::Result;
    use serde::Serialize;

    alloy::sol!(
        #![sol(rpc, all_derives)]
        "src/contracts/artifacts/IHitPoints.sol"
    );

    alloy::sol! {
        #[derive(Debug, Serialize)]
        struct Permit {
            address owner;
            address spender;
            uint256 value;
            uint256 nonce;
            uint256 deadline;
        }
    }

    alloy::sol! {
        #[sol(rpc)]
        interface IERC20 {
            function approve(address spender, uint256 value) external returns (bool);
            function balanceOf(address account) external view returns (uint256);
        }
    }

    alloy::sol! {
        #[sol(rpc)]
        interface IERC20Permit {
            function nonces(address owner) external view returns (uint256);
            function DOMAIN_SEPARATOR() external view returns (bytes32);
        }
    }

    impl Permit {
        /// Signs the [Permit] with the given signer and EIP-712 domain derived from the given
        /// contract address and chain ID.
        pub async fn sign(
            &self,
            signer: &impl Signer,
            contract_addr: Address,
            chain_id: u64,
        ) -> Result<PrimitiveSignature> {
            let domain = eip712_domain! {
                name: "HitPoints",
                version: "1",
                chain_id: chain_id,
                verifying_contract: contract_addr,
            };
            let hash = self.eip712_signing_hash(&domain);
            Ok(signer.sign_hash(&hash).await?)
        }
    }
}

/// Status of a proof request
#[derive(Debug, PartialEq)]
pub enum ProofStatus {
    /// The request has expired.
    Expired,
    /// The request is locked in and waiting for fulfillment.
    Locked,
    /// The request has been fulfilled.
    Fulfilled,
    /// The request has an unknown status.
    ///
    /// This is used to represent the status of a request
    /// with no evidence in the state. The request may be
    /// open for bidding or it may not exist.
    Unknown,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
/// EIP-712 domain separator without the salt field.
pub struct EIP721DomainSaltless {
    /// The name of the domain.
    pub name: Cow<'static, str>,
    /// The protocol version.
    pub version: Cow<'static, str>,
    /// The chain ID.
    pub chain_id: u64,
    /// The address of the verifying contract.
    pub verifying_contract: Address,
}

impl EIP721DomainSaltless {
    /// Returns the EIP-712 domain with the salt field set to zero.
    pub fn alloy_struct(&self) -> Eip712Domain {
        eip712_domain! {
            name: self.name.clone(),
            version: self.version.clone(),
            chain_id: self.chain_id,
            verifying_contract: self.verifying_contract,
        }
    }
}

pub(crate) fn request_id(addr: &Address, id: u32) -> U256 {
    #[allow(clippy::unnecessary_fallible_conversions)] // U160::from does not compile
    let addr = U160::try_from(*addr).unwrap();
    (U256::from(addr) << 32) | U256::from(id)
}

#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
/// Errors that can occur when creating a proof request.
pub enum RequestError {
    /// The request ID is malformed.
    #[error("malformed request ID")]
    MalformedRequestId,

    /// The signature is invalid.
    #[cfg(not(target_os = "zkvm"))]
    #[error("signature error: {0}")]
    SignatureError(#[from] alloy::signers::Error),

    /// The image URL is empty.
    #[error("image URL must not be empty")]
    EmptyImageUrl,

    /// The image URL is malformed.
    #[error("malformed image URL: {0}")]
    MalformedImageUrl(#[from] url::ParseError),

    /// The image ID is zero.
    #[error("image ID must not be ZERO")]
    ImageIdIsZero,

    /// The offer timeout is zero.
    #[error("offer timeout must be greater than 0")]
    OfferTimeoutIsZero,

    /// The offer max price is zero.
    #[error("offer maxPrice must be greater than 0")]
    OfferMaxPriceIsZero,

    /// The offer max price is less than the min price.
    #[error("offer maxPrice must be greater than or equal to minPrice")]
    OfferMaxPriceIsLessThanMin,

    /// The offer bidding start is zero.
    #[error("offer biddingStart must be greater than 0")]
    OfferBiddingStartIsZero,
}

#[cfg(not(target_os = "zkvm"))]
impl From<SignatureError> for RequestError {
    fn from(err: alloy::primitives::SignatureError) -> Self {
        RequestError::SignatureError(err.into())
    }
}

impl ProofRequest {
    /// Creates a new proof request with the given parameters.
    ///
    /// The request ID is generated by combining the address and given idx.
    pub fn new(
        idx: u32,
        addr: &Address,
        requirements: Requirements,
        image_url: &str,
        input: Input,
        offer: Offer,
    ) -> Self {
        Self {
            id: request_id(addr, idx),
            requirements,
            imageUrl: image_url.to_string(),
            input,
            offer,
        }
    }

    /// Returns the client address from the request ID.
    pub fn client_address(&self) -> Result<Address, RequestError> {
        let shifted_id: U256 = self.id >> 32;
        if self.id >> 192 != U256::ZERO {
            return Err(RequestError::MalformedRequestId);
        }
        let shifted_bytes: [u8; 32] = shifted_id.to_be_bytes();
        let addr_bytes: [u8; 20] = shifted_bytes[12..32]
            .try_into()
            .expect("error in converting slice of 20 bytes into array of 20 bytes");
        let lower_160_bits = U160::from_be_bytes(addr_bytes);

        Ok(Address::from(lower_160_bits))
    }

    /// Sets the input data to be fetched from the given URL.
    pub fn with_image_url(self, image_url: &str) -> Self {
        Self { imageUrl: image_url.to_string(), ..self }
    }

    /// Sets the requirements for the request.
    pub fn with_requirements(self, requirements: Requirements) -> Self {
        Self { requirements, ..self }
    }

    /// Sets the guest's input for the request.
    pub fn with_input(self, input: Input) -> Self {
        Self { input, ..self }
    }

    /// Sets the offer for the request.
    pub fn with_offer(self, offer: Offer) -> Self {
        Self { offer, ..self }
    }

    /// Returns the block number at which the request expires.
    pub fn expires_at(&self) -> u64 {
        self.offer.biddingStart + self.offer.timeout as u64
    }

    /// Check that the request is valid and internally consistent.
    ///
    /// If any field are empty, or if two fields conflict (e.g. the max price is less than the min
    /// price) this function will return an error.
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.imageUrl.is_empty() {
            return Err(RequestError::EmptyImageUrl);
        };
        Url::parse(&self.imageUrl).map(|_| ())?;

        if self.requirements.imageId == B256::default() {
            return Err(RequestError::ImageIdIsZero);
        };
        if self.offer.timeout == 0 {
            return Err(RequestError::OfferTimeoutIsZero);
        };
        if self.offer.maxPrice == U256::ZERO {
            return Err(RequestError::OfferMaxPriceIsZero);
        };
        if self.offer.maxPrice < self.offer.minPrice {
            return Err(RequestError::OfferMaxPriceIsLessThanMin);
        }
        if self.offer.biddingStart == 0 {
            return Err(RequestError::OfferBiddingStartIsZero);
        };

        Ok(())
    }
}

#[cfg(not(target_os = "zkvm"))]
impl ProofRequest {
    /// Signs the request with the given signer and EIP-712 domain derived from the given
    /// contract address and chain ID.
    pub async fn sign_request(
        &self,
        signer: &impl Signer,
        contract_addr: Address,
        chain_id: u64,
    ) -> Result<PrimitiveSignature, RequestError> {
        let domain = eip712_domain(contract_addr, chain_id);
        let hash = self.eip712_signing_hash(&domain.alloy_struct());
        Ok(signer.sign_hash(&hash).await?)
    }

    /// Verifies the request signature with the given signer and EIP-712 domain derived from
    /// the given contract address and chain ID.
    pub fn verify_signature(
        &self,
        signature: &Bytes,
        contract_addr: Address,
        chain_id: u64,
    ) -> Result<(), RequestError> {
        let sig = PrimitiveSignature::try_from(signature.as_ref())?;
        let domain = eip712_domain(contract_addr, chain_id);
        let hash = self.eip712_signing_hash(&domain.alloy_struct());
        let addr = sig.recover_address_from_prehash(&hash)?;
        if addr == self.client_address()? {
            Ok(())
        } else {
            Err(SignatureError::FromBytes("Address mismatch").into())
        }
    }
}

impl Requirements {
    /// Creates a new requirements with the given image ID and predicate.
    pub fn new(image_id: impl Into<Digest>, predicate: Predicate) -> Self {
        Self { imageId: <[u8; 32]>::from(image_id.into()).into(), predicate }
    }

    /// Sets the image ID.
    pub fn with_image_id(self, image_id: impl Into<Digest>) -> Self {
        Self { imageId: <[u8; 32]>::from(image_id.into()).into(), ..self }
    }

    /// Sets the predicate.
    pub fn with_predicate(self, predicate: Predicate) -> Self {
        Self { predicate, ..self }
    }
}

impl Predicate {
    /// Returns a predicate to match the journal digest. This ensures that the request's
    /// fulfillment will contain a journal with the same digest.
    pub fn digest_match(digest: impl Into<Digest>) -> Self {
        Self {
            predicateType: PredicateType::DigestMatch,
            data: digest.into().as_bytes().to_vec().into(),
        }
    }

    /// Returns a predicate to match the journal prefix. This ensures that the request's
    /// fulfillment will contain a journal with the same prefix.
    pub fn prefix_match(prefix: impl Into<Bytes>) -> Self {
        Self { predicateType: PredicateType::PrefixMatch, data: prefix.into() }
    }
}

impl Input {
    /// Sets the input type to inline and the data to the given bytes.
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::{contracts::Input, input::InputBuilder};
    ///
    /// let input = Input::inline(InputBuilder::new().write(&vec![0x41, 0x41, 0x41, 0x41]).unwrap().build());
    /// ```
    ///
    /// See [`InputBuilder`][crate::input::InputBuilder] for more details on how to write input data.
    pub fn inline(data: impl Into<Bytes>) -> Self {
        Self { inputType: InputType::Inline, data: data.into() }
    }

    /// Sets the input type to URL and the data to the given URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self { inputType: InputType::Url, data: url.into().into() }
    }
}

impl Offer {
    /// Sets the offer minimum price.
    pub fn with_min_price(self, min_price: U256) -> Self {
        Self { minPrice: min_price, ..self }
    }

    /// Sets the offer maximum price.
    pub fn with_max_price(self, max_price: U256) -> Self {
        Self { maxPrice: max_price, ..self }
    }

    /// Sets the offer lock-in stake.
    pub fn with_lock_stake(self, lock_stake: U256) -> Self {
        Self { lockStake: lock_stake, ..self }
    }

    /// Sets the offer bidding start as block number.
    pub fn with_bidding_start(self, bidding_start: u64) -> Self {
        Self { biddingStart: bidding_start, ..self }
    }

    /// Sets the offer timeout as number of blocks from the bidding start before expiring.
    pub fn with_timeout(self, timeout: u32) -> Self {
        Self { timeout, ..self }
    }

    /// Sets the offer ramp-up period as number of blocks from the bidding start before the price
    /// starts to increase until the maximum price.
    pub fn with_ramp_up_period(self, ramp_up_period: u32) -> Self {
        Self { rampUpPeriod: ramp_up_period, ..self }
    }

    /// Sets the offer minimum price based on the desired price per million cycles.
    pub fn with_min_price_per_mcycle(self, mcycle_price: U256, mcycle: u64) -> Self {
        let min_price = mcycle_price * U256::from(mcycle);
        Self { minPrice: min_price, ..self }
    }

    /// Sets the offer maximum price based on the desired price per million cycles.
    pub fn with_max_price_per_mcycle(self, mcycle_price: U256, mcycle: u64) -> Self {
        let max_price = mcycle_price * U256::from(mcycle);
        Self { maxPrice: max_price, ..self }
    }

    /// Sets the offer lock-in stake based on the desired price per million cycles.
    pub fn with_lock_stake_per_mcycle(self, mcycle_price: U256, mcycle: u64) -> Self {
        let lock_stake = mcycle_price * U256::from(mcycle);
        Self { lockStake: lock_stake, ..self }
    }
}

// TODO: These are not so much "default" as they are "empty". Default is not quite the right
// semantics here. This would be replaced by a builder or an `empty` function.
impl Default for ProofRequest {
    fn default() -> Self {
        Self {
            id: U256::ZERO,
            requirements: Default::default(),
            imageUrl: Default::default(),
            input: Default::default(),
            offer: Default::default(),
        }
    }
}

#[allow(clippy::derivable_impls)] // struct defined in generated code
impl Default for Requirements {
    fn default() -> Self {
        Self { imageId: Default::default(), predicate: Default::default() }
    }
}

impl Default for Predicate {
    fn default() -> Self {
        Self { predicateType: PredicateType::PrefixMatch, data: Default::default() }
    }
}

impl Default for Input {
    fn default() -> Self {
        Self { inputType: InputType::Inline, data: Default::default() }
    }
}

use sha2::{Digest as _, Sha256};
#[cfg(not(target_os = "zkvm"))]
use IBoundlessMarket::IBoundlessMarketErrors;
#[cfg(not(target_os = "zkvm"))]
use IRiscZeroSetVerifier::IRiscZeroSetVerifierErrors;

impl Predicate {
    /// Evaluates the predicate against the given journal.
    #[inline]
    pub fn eval(&self, journal: impl AsRef<[u8]>) -> bool {
        match self.predicateType {
            PredicateType::DigestMatch => self.data.as_ref() == Sha256::digest(journal).as_slice(),
            PredicateType::PrefixMatch => journal.as_ref().starts_with(&self.data),
            PredicateType::__Invalid => panic!("invalid PredicateType"),
        }
    }
}

#[cfg(not(target_os = "zkvm"))]
/// The Boundless market module.
pub mod boundless_market;
#[cfg(not(target_os = "zkvm"))]
/// The Hit Points module.
pub mod hit_points;
#[cfg(not(target_os = "zkvm"))]
/// The Set Verifier module.
pub mod set_verifier;

#[cfg(not(target_os = "zkvm"))]
#[derive(Error, Debug)]
/// Errors that can occur when interacting with the contracts.
pub enum TxnErr {
    /// Error from the SetVerifier contract.
    #[error("SetVerifier error: {0:?}")]
    SetVerifierErr(IRiscZeroSetVerifierErrors),

    /// Error from the BoundlessMarket contract.
    #[error("BoundlessMarket Err: {0:?}")]
    BoundlessMarketErr(IBoundlessMarket::IBoundlessMarketErrors),

    /// Error from the HitPoints contract.
    #[error("HitPoints Err: {0:?}")]
    HitPointsErr(IHitPoints::IHitPointsErrors),

    /// Missing data while decoding the error response from the contract.
    #[error("decoding err, missing data, code: {0} msg: {1}")]
    MissingData(i64, String),

    /// Error decoding the error response from the contract.
    #[error("decoding err: bytes decoding")]
    BytesDecode,

    /// Error from the contract.
    #[error("contract error: {0}")]
    ContractErr(ContractErr),

    /// ABI decoder error.
    #[error("abi decoder error: {0} - {1}")]
    DecodeErr(DecoderErr, Bytes),
}

// TODO: Deduplicate the code from the following two conversion methods.
#[cfg(not(target_os = "zkvm"))]
impl From<ContractErr> for TxnErr {
    fn from(err: ContractErr) -> Self {
        match err {
            ContractErr::TransportError(TransportError::ErrorResp(ts_err)) => {
                let Some(data) = ts_err.data else {
                    return TxnErr::MissingData(ts_err.code, ts_err.message.to_string());
                };

                let data = data.get().trim_matches('"');

                let Ok(data) = Bytes::from_str(data) else {
                    return Self::BytesDecode;
                };

                // Trial deocde the error with each possible contract ABI. Right now, there are two.
                if let Ok(decoded_error) = IBoundlessMarketErrors::abi_decode(&data, true) {
                    return Self::BoundlessMarketErr(decoded_error);
                }
                if let Ok(decoded_error) = IHitPointsErrors::abi_decode(&data, true) {
                    return Self::HitPointsErr(decoded_error);
                }
                match IRiscZeroSetVerifierErrors::abi_decode(&data, true) {
                    Ok(decoded_error) => Self::SetVerifierErr(decoded_error),
                    Err(err) => Self::DecodeErr(err, data),
                }
            }
            _ => Self::ContractErr(err),
        }
    }
}

#[cfg(not(target_os = "zkvm"))]
fn decode_contract_err<T: SolInterface>(err: ContractErr) -> Result<T, TxnErr> {
    match err {
        ContractErr::TransportError(TransportError::ErrorResp(ts_err)) => {
            let Some(data) = ts_err.data else {
                return Err(TxnErr::MissingData(ts_err.code, ts_err.message.to_string()));
            };

            let data = data.get().trim_matches('"');

            let Ok(data) = Bytes::from_str(data) else {
                return Err(TxnErr::BytesDecode);
            };

            let decoded_error = match T::abi_decode(&data, true) {
                Ok(res) => res,
                Err(err) => {
                    return Err(TxnErr::DecodeErr(err, data));
                }
            };

            Ok(decoded_error)
        }
        _ => Err(TxnErr::ContractErr(err)),
    }
}

#[cfg(not(target_os = "zkvm"))]
impl IHitPointsErrors {
    pub(crate) fn decode_error(err: ContractErr) -> TxnErr {
        match decode_contract_err(err) {
            Ok(res) => TxnErr::HitPointsErr(res),
            Err(decode_err) => decode_err,
        }
    }
}

#[cfg(not(target_os = "zkvm"))]
/// The EIP-712 domain separator for the Boundless Market contract.
pub fn eip712_domain(addr: Address, chain_id: u64) -> EIP721DomainSaltless {
    EIP721DomainSaltless {
        name: "IBoundlessMarket".into(),
        version: "1".into(),
        chain_id,
        verifying_contract: addr,
    }
}

#[cfg(feature = "test-utils")]
#[allow(missing_docs)]
/// Module for testing utilities.
pub mod test_utils {
    use alloy::{
        network::{Ethereum, EthereumWallet},
        node_bindings::AnvilInstance,
        primitives::{Address, FixedBytes, U256},
        providers::{
            ext::AnvilApi,
            fillers::{
                BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
                WalletFiller,
            },
            Identity, Provider, ProviderBuilder, RootProvider,
        },
        signers::local::PrivateKeySigner,
        sol_types::{SolCall, SolConstructor},
        transports::{BoxTransport, Transport},
    };
    use anyhow::{Context, Result};
    use risc0_zkvm::sha::Digest;
    use std::sync::Arc;

    use crate::contracts::{
        boundless_market::BoundlessMarketService,
        hit_points::{default_allowance, HitPointsService},
        set_verifier::SetVerifierService,
    };

    // Bytecode for the contracts is copied from the contract build output by the build script. It
    // is checked into git so that we can avoid issues with publishing to crates.io. We do not use
    // the full JSON build out because it is less stable.

    const MOCK_VERIFIER_BYTECODE: &str = include_str!("./artifacts/RiscZeroMockVerifier.hex");
    alloy::sol! {
        #[sol(rpc)]
        contract MockVerifier {
            constructor(bytes4 selector) {}
        }
    }

    const SET_VERIFIER_BYTECODE: &str = include_str!("./artifacts/RiscZeroSetVerifier.hex");
    alloy::sol! {
        #![sol(rpc)]
        contract SetVerifier {
            constructor(address verifier, bytes32 imageId, string memory imageUrl) {}
        }
    }

    const BOUNDLESS_MARKET_BYTECODE: &str = include_str!("./artifacts/BoundlessMarket.hex");
    alloy::sol! {
        #![sol(rpc)]
        contract BoundlessMarket {
            constructor(address verifier, bytes32 assessorId, address stakeTokenContract) {}
            function initialize(address initialOwner, string calldata imageUrl) {}
        }
    }

    const ERC1967_PROXY_BYTECODE: &str = include_str!("./artifacts/ERC1967Proxy.hex");
    alloy::sol! {
        #![sol(rpc)]
        contract ERC1967Proxy {
            constructor(address implementation, bytes memory data) payable {}
        }
    }

    const HIT_POINTS_BYTECODE: &str = include_str!("./artifacts/HitPoints.hex");
    alloy::sol! {
        #![sol(rpc)]
        contract HitPoints {
            constructor(address initialOwner) payable {}
        }
    }

    // Note: I was completely unable to solve this with generics or trait objects
    type ProviderWallet = FillProvider<
        JoinFill<
            JoinFill<
                Identity,
                JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
            >,
            WalletFiller<EthereumWallet>,
        >,
        RootProvider<BoxTransport>,
        BoxTransport,
        Ethereum,
    >;

    pub struct TestCtx {
        pub verifier_addr: Address,
        pub set_verifier_addr: Address,
        pub hit_points_addr: Address,
        pub boundless_market_addr: Address,
        pub prover_signer: PrivateKeySigner,
        pub customer_signer: PrivateKeySigner,
        pub prover_provider: ProviderWallet,
        pub prover_market: BoundlessMarketService<BoxTransport, ProviderWallet>,
        pub customer_provider: ProviderWallet,
        pub customer_market: BoundlessMarketService<BoxTransport, ProviderWallet>,
        pub set_verifier: SetVerifierService<BoxTransport, ProviderWallet>,
        pub hit_points_service: HitPointsService<BoxTransport, ProviderWallet>,
    }

    pub async fn deploy_mock_verifier<T, P>(deployer_provider: P) -> Result<Address>
    where
        T: Transport + Clone,
        P: Provider<T, Ethereum> + 'static + Clone,
    {
        alloy::contract::RawCallBuilder::new_raw_deploy(
            deployer_provider,
            [
                hex::decode(MOCK_VERIFIER_BYTECODE).unwrap(),
                MockVerifier::constructorCall { selector: FixedBytes::ZERO }.abi_encode(),
            ]
            .concat()
            .into(),
        )
        .deploy()
        .await
        .context("failed to deploy RiscZeroMockVerifier")
    }

    pub async fn deploy_set_verifier<T, P>(
        deployer_provider: P,
        verifier_address: Address,
        set_builder_id: Digest,
    ) -> Result<Address>
    where
        T: Transport + Clone,
        P: Provider<T, Ethereum> + 'static + Clone,
    {
        alloy::contract::RawCallBuilder::new_raw_deploy(
            deployer_provider,
            [
                hex::decode(SET_VERIFIER_BYTECODE).unwrap(),
                SetVerifier::constructorCall {
                    verifier: verifier_address,
                    imageId: <[u8; 32]>::from(set_builder_id).into(),
                    imageUrl: String::new(),
                }
                .abi_encode(),
            ]
            .concat()
            .into(),
        )
        .deploy()
        .await
        .context("failed to deploy RiscZeroSetVerifier")
    }

    pub async fn deploy_hit_points<T, P>(
        deployer_signer: &PrivateKeySigner,
        deployer_provider: P,
    ) -> Result<Address>
    where
        T: Transport + Clone,
        P: Provider<T, Ethereum> + 'static + Clone,
    {
        let deployer_address = deployer_signer.address();
        alloy::contract::RawCallBuilder::new_raw_deploy(
            deployer_provider,
            [
                hex::decode(HIT_POINTS_BYTECODE).unwrap(),
                HitPoints::constructorCall { initialOwner: deployer_address }.abi_encode(),
            ]
            .concat()
            .into(),
        )
        .deploy()
        .await
        .context("failed to deploy HitPoints contract")
    }

    pub async fn deploy_boundless_market<T, P>(
        deployer_signer: &PrivateKeySigner,
        deployer_provider: P,
        set_verifier: Address,
        hit_points: Address,
        assessor_guest_id: Digest,
        allowed_prover: Option<Address>,
    ) -> Result<Address>
    where
        T: Transport + Clone,
        P: Provider<T, Ethereum> + 'static + Clone,
    {
        let deployer_address = deployer_signer.address();

        let boundless_market = alloy::contract::RawCallBuilder::new_raw_deploy(
            &deployer_provider,
            [
                hex::decode(BOUNDLESS_MARKET_BYTECODE).unwrap(),
                BoundlessMarket::constructorCall {
                    verifier: set_verifier,
                    assessorId: <[u8; 32]>::from(assessor_guest_id).into(),
                    stakeTokenContract: hit_points,
                }
                .abi_encode(),
            ]
            .concat()
            .into(),
        )
        .deploy()
        .await
        .context("failed to deploy BoundlessMarket implementation")?;

        let proxy = alloy::contract::RawCallBuilder::new_raw_deploy(
            &deployer_provider,
            [
                hex::decode(ERC1967_PROXY_BYTECODE).unwrap(),
                ERC1967Proxy::constructorCall {
                    implementation: boundless_market,
                    data: BoundlessMarket::initializeCall {
                        initialOwner: deployer_address,
                        imageUrl: "".to_string(),
                    }
                    .abi_encode()
                    .into(),
                }
                .abi_encode(),
            ]
            .concat()
            .into(),
        )
        .deploy()
        .await
        .context("failed to deploy BoundlessMarket proxy")?;

        if hit_points != Address::ZERO {
            let hit_points_service = HitPointsService::new(
                hit_points,
                deployer_provider.clone(),
                deployer_signer.address(),
            );
            hit_points_service.grant_minter_role(hit_points_service.caller()).await?;
            hit_points_service.grant_authorized_transfer_role(proxy).await?;
            if let Some(prover) = allowed_prover {
                hit_points_service.mint(prover, default_allowance()).await?;
            }
        }

        Ok(proxy)
    }

    impl TestCtx {
        async fn deploy_contracts(
            anvil: &AnvilInstance,
            set_builder_id: Digest,
            assessor_guest_id: Digest,
        ) -> Result<(Address, Address, Address, Address)> {
            let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
            let deployer_provider = Arc::new(
                ProviderBuilder::new()
                    .with_recommended_fillers()
                    .wallet(EthereumWallet::from(deployer_signer.clone()))
                    .on_builtin(&anvil.endpoint())
                    .await
                    .unwrap(),
            );

            // Deploy contracts
            let verifier = deploy_mock_verifier(Arc::clone(&deployer_provider)).await?;
            let set_verifier =
                deploy_set_verifier(Arc::clone(&deployer_provider), verifier, set_builder_id)
                    .await?;
            let hit_points =
                deploy_hit_points(&deployer_signer, Arc::clone(&deployer_provider)).await?;
            let boundless_market = deploy_boundless_market(
                &deployer_signer,
                Arc::clone(&deployer_provider),
                set_verifier,
                hit_points,
                assessor_guest_id,
                None,
            )
            .await?;

            // Mine forward some blocks using the provider
            deployer_provider.anvil_mine(Some(U256::from(10)), Some(U256::from(2))).await.unwrap();
            deployer_provider.anvil_set_interval_mining(2).await.unwrap();

            Ok((verifier, set_verifier, hit_points, boundless_market))
        }

        pub async fn new(
            anvil: &AnvilInstance,
            set_builder_id: Digest,
            assessor_guest_id: Digest,
        ) -> Result<Self> {
            let (verifier_addr, set_verifier_addr, hit_points_addr, boundless_market_addr) =
                TestCtx::deploy_contracts(anvil, set_builder_id, assessor_guest_id).await.unwrap();

            let prover_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
            let customer_signer: PrivateKeySigner = anvil.keys()[2].clone().into();
            let verifier_signer: PrivateKeySigner = anvil.keys()[0].clone().into();

            let prover_provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(prover_signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap();
            let customer_provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(customer_signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap();
            let verifier_provider = ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(verifier_signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap();

            let prover_market = BoundlessMarketService::new(
                boundless_market_addr,
                prover_provider.clone(),
                prover_signer.address(),
            );

            let customer_market = BoundlessMarketService::new(
                boundless_market_addr,
                customer_provider.clone(),
                customer_signer.address(),
            );

            let set_verifier = SetVerifierService::new(
                set_verifier_addr,
                verifier_provider.clone(),
                verifier_signer.address(),
            );

            let hit_points_service = HitPointsService::new(
                hit_points_addr,
                verifier_provider.clone(),
                verifier_signer.address(),
            );

            hit_points_service.mint(prover_signer.address(), default_allowance()).await?;

            Ok(TestCtx {
                verifier_addr,
                set_verifier_addr,
                hit_points_addr,
                boundless_market_addr,
                prover_signer,
                customer_signer,
                prover_provider,
                prover_market,
                customer_provider,
                customer_market,
                set_verifier,
                hit_points_service,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;

    async fn create_order(
        signer: &impl Signer,
        signer_addr: Address,
        order_id: u32,
        contract_addr: Address,
        chain_id: u64,
    ) -> (ProofRequest, [u8; 65]) {
        let request_id = request_id(&signer_addr, order_id);

        let req = ProofRequest {
            id: request_id,
            requirements: Requirements {
                imageId: B256::ZERO,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            imageUrl: "test".to_string(),
            input: Input { inputType: InputType::Url, data: Default::default() },
            offer: Offer {
                minPrice: U256::from(0),
                maxPrice: U256::from(1),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockStake: U256::from(0),
            },
        };

        let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

        (req, client_sig.as_bytes())
    }

    #[tokio::test]
    async fn validate_sig() {
        let signer: PrivateKeySigner =
            "6f142508b4eea641e33cb2a0161221105086a84584c74245ca463a49effea30b".parse().unwrap();
        let order_id: u32 = 1;
        let contract_addr = Address::ZERO;
        let chain_id = 1;
        let signer_addr = signer.address();

        let (req, client_sig) =
            create_order(&signer, signer_addr, order_id, contract_addr, chain_id).await;

        req.verify_signature(&Bytes::from(client_sig), contract_addr, chain_id).unwrap();
    }

    #[tokio::test]
    #[should_panic(expected = "SignatureError")]
    async fn invalid_sig() {
        let signer: PrivateKeySigner =
            "6f142508b4eea641e33cb2a0161221105086a84584c74245ca463a49effea30b".parse().unwrap();
        let order_id: u32 = 1;
        let contract_addr = Address::ZERO;
        let chain_id = 1;
        let signer_addr = signer.address();

        let (req, mut client_sig) =
            create_order(&signer, signer_addr, order_id, contract_addr, chain_id).await;

        client_sig[0] = 1;
        req.verify_signature(&Bytes::from(client_sig), contract_addr, chain_id).unwrap();
    }
}
