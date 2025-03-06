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
use alloy_primitives::{
    aliases::{U160, U96},
    Address, Bytes, FixedBytes, B256, U256,
};
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
use crate::input::InputBuilder;

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

    /// The offer lock timeout is zero.
    #[error("offer lock timeout must be greater than 0")]
    OfferLockTimeoutIsZero,

    /// The offer max price is zero.
    #[error("offer maxPrice must be greater than 0")]
    OfferMaxPriceIsZero,

    /// The offer max price is less than the min price.
    #[error("offer maxPrice must be greater than or equal to minPrice")]
    OfferMaxPriceIsLessThanMin,

    /// The offer bidding start is zero.
    #[error("offer biddingStart must be greater than 0")]
    OfferBiddingStartIsZero,

    /// The requirements are missing.
    #[error("missing requirements")]
    MissingRequirements,

    /// The image URL is missing.
    #[error("missing image URL")]
    MissingImageUrl,

    /// The input is missing.
    #[error("missing input")]
    MissingInput,

    /// The offer is missing.
    #[error("missing offer")]
    MissingOffer,
}

#[cfg(not(target_os = "zkvm"))]
impl From<SignatureError> for RequestError {
    fn from(err: alloy::primitives::SignatureError) -> Self {
        RequestError::SignatureError(err.into())
    }
}

/// A proof request builder.
pub struct ProofRequestBuilder {
    requirements: Option<Requirements>,
    image_url: Option<String>,
    input: Option<Input>,
    offer: Option<Offer>,
}

impl Default for ProofRequestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ProofRequestBuilder {
    /// Creates a new proof request builder.
    pub fn new() -> Self {
        Self { requirements: None, image_url: None, input: None, offer: None }
    }

    /// Builds the proof request.
    pub fn build(self) -> Result<ProofRequest, RequestError> {
        let requirements = self.requirements.ok_or(RequestError::MissingRequirements)?;
        let image_url = self.image_url.ok_or(RequestError::MissingImageUrl)?;
        let input = self.input.ok_or(RequestError::MissingInput)?;
        let offer = self.offer.ok_or(RequestError::MissingOffer)?;

        Ok(ProofRequest::new(0, &Address::ZERO, requirements, &image_url, input, offer))
    }

    /// Sets the input data to be fetched from the given URL.
    pub fn with_image_url(self, image_url: impl Into<String>) -> Self {
        Self { image_url: Some(image_url.into()), ..self }
    }

    /// Sets the requirements for the request.
    pub fn with_requirements(self, requirements: impl Into<Requirements>) -> Self {
        Self { requirements: Some(requirements.into()), ..self }
    }

    /// Sets the guest's input for the request.
    pub fn with_input(self, input: impl Into<Input>) -> Self {
        Self { input: Some(input.into()), ..self }
    }

    /// Sets the offer for the request.
    pub fn with_offer(self, offer: impl Into<Offer>) -> Self {
        Self { offer: Some(offer.into()), ..self }
    }
}

impl ProofRequest {
    /// Create a new [ProofRequestBuilder]
    pub fn builder() -> ProofRequestBuilder {
        ProofRequestBuilder::new()
    }

    /// Creates a new proof request with the given parameters.
    ///
    /// The request ID is generated by combining the address and given idx.
    pub fn new(
        idx: u32,
        addr: &Address,
        requirements: Requirements,
        image_url: impl Into<String>,
        input: impl Into<Input>,
        offer: Offer,
    ) -> Self {
        Self {
            id: request_id(addr, idx),
            requirements,
            imageUrl: image_url.into(),
            input: input.into(),
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
        if self.offer.lockTimeout == 0 {
            return Err(RequestError::OfferLockTimeoutIsZero);
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
        Self {
            imageId: <[u8; 32]>::from(image_id.into()).into(),
            predicate,
            callback: Callback::default(),
            selector: FixedBytes::<4>([0; 4]),
        }
    }

    /// Sets the image ID.
    pub fn with_image_id(self, image_id: impl Into<Digest>) -> Self {
        Self { imageId: <[u8; 32]>::from(image_id.into()).into(), ..self }
    }

    /// Sets the predicate.
    pub fn with_predicate(self, predicate: Predicate) -> Self {
        Self { predicate, ..self }
    }

    /// Sets the callback.
    pub fn with_callback(self, callback: Callback) -> Self {
        Self { callback, ..self }
    }

    /// Sets the selector.
    pub fn with_selector(self, selector: FixedBytes<4>) -> Self {
        Self { selector, ..self }
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

impl Callback {
    /// Sets the address of the callback.
    pub fn with_addr(self, addr: impl Into<Address>) -> Self {
        Self { addr: addr.into(), ..self }
    }

    /// Sets the gas limit of the callback.
    pub fn with_gas_limit(self, gas_limit: u64) -> Self {
        Self { gasLimit: U96::from(gas_limit), ..self }
    }
}

impl Input {
    /// Create a new [InputBuilder] for use in constructing and encoding the guest zkVM environment.
    #[cfg(not(target_os = "zkvm"))]
    pub fn builder() -> InputBuilder {
        InputBuilder::new()
    }

    /// Sets the input type to inline and the data to the given bytes.
    ///
    /// See [InputBuilder] for more details on how to write input data.
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::contracts::Input;
    ///
    /// let input_vec = Input::builder().write(&[0x41, 0x41, 0x41, 0x41])?.build_vec()?;
    /// let input = Input::inline(input_vec);
    /// # anyhow::Ok(())
    /// ```
    pub fn inline(data: impl Into<Bytes>) -> Self {
        Self { inputType: InputType::Inline, data: data.into() }
    }

    /// Sets the input type to URL and the data to the given URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self { inputType: InputType::Url, data: url.into().into() }
    }
}

impl From<Url> for Input {
    /// Create a URL input from the given URL.
    fn from(value: Url) -> Self {
        Self::url(value)
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

    /// Sets the offer lock-in timeout as number of blocks from the bidding start before expiring.
    pub fn with_lock_timeout(self, lock_timeout: u32) -> Self {
        Self { lockTimeout: lock_timeout, ..self }
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
    use crate::contracts::{
        boundless_market::BoundlessMarketService,
        hit_points::{default_allowance, HitPointsService},
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::AnvilInstance,
        primitives::{Address, FixedBytes},
        providers::{ext::AnvilApi, Provider, ProviderBuilder, WalletProvider},
        signers::local::PrivateKeySigner,
        sol_types::SolCall,
    };
    use anyhow::{Context, Result};
    use risc0_ethereum_contracts::set_verifier::SetVerifierService;
    use risc0_zkvm::sha::Digest;

    // Bytecode for the contracts is copied from the contract build output by the build script. It
    // is checked into git so that we can avoid issues with publishing to crates.io. We do not use
    // the full JSON build out because it is less stable.

    alloy::sol! {
        #[sol(rpc, bytecode = "60a034607557601f6106a438819003918201601f19168301916001600160401b03831184841017607957808492602094604052833981010312607557516001600160e01b031981168103607557608052604051610616908161008e82396080518181816101b2015281816102af015261031e0152f35b5f80fd5b634e487b7160e01b5f52604160045260245ffdfe6080806040526004361015610012575f80fd5b5f3560e01c908163053c238d146101a0575080631599ead51461012d5780633a115bb11461010e57806366cf0e4b146100c85763ab750e7514610053575f80fd5b346100c45760603660031901126100c4576004356001600160401b0381116100c457366023820112156100c4578060040135906001600160401b0382116100c45736602483830101116100c4576100c29160246100bb6100b660443583356103e5565b610518565b920161030a565b005b5f80fd5b346100c45760403660031901126100c4576100e1610288565b5061010a6100fe6100f96100b66024356004356103e5565b6102a1565b604051918291826101e2565b0390f35b346100c45760203660031901126100c45761010a6100fe6004356102a1565b346100c45760203660031901126100c4576004356001600160401b0381116100c45780360360406003198201126100c457600482013590602219018112156100c45781016004810135906001600160401b0382116100c4576024019080360382136100c45760246100c29301359161030a565b346100c4575f3660031901126100c4577f00000000000000000000000000000000000000000000000000000000000000006001600160e01b0319168152602090f35b60208060809381845280516040838601528051938491826060880152018686015e5f84840186015201516040830152601f01601f1916010190565b604081019081106001600160401b0382111761023857604052565b634e487b7160e01b5f52604160045260245ffd5b60a081019081106001600160401b0382111761023857604052565b90601f801991011681019081106001600160401b0382111761023857604052565b604051906102958261021d565b5f602083606081520152565b6102a9610288565b506040517f00000000000000000000000000000000000000000000000000000000000000006001600160e01b031916602082015260248082018390528152906102f3604483610267565b604051916103008361021d565b8252602082015290565b81600411806100c4576001600160e01b03197f00000000000000000000000000000000000000000000000000000000000000008116908335168082036103d05750506100c45760031982016001600160401b038111610238576040519161037b601b8501601f191660200184610267565b818352602083019336818301116100c4575f926004601c93018637830101525190209060405160208101918252602081526103b7604082610267565b519020036103c157565b63439cc0cd60e01b5f5260045ffd5b632e2ce35360e21b5f5260045260245260445ffd5b905f60806040516103f58161024c565b82815282602082015260405161040a8161021d565b838152836020820152604082015282606082015201526040519061042d8261021d565b5f82525f6020830152604051906104438261021d565b8152602081015f815260205f600c6040516b1c9a5cd8cc0b93dd5d1c1d5d60a21b815260025afa1561050d576020915f918251915190516040519185830193845260408301526060820152600160f91b6080820152606281526104a7608282610267565b604051918291518091835e8101838152039060025afa1561050d575f5190604051926104d28461024c565b83527fa3acc27117418996340b84e5a90f3ef4c49d22c79e44aad822ec9c313e1eb8e2602084015260408301525f6060830152608082015290565b6040513d5f823e3d90fd5b60205f60126040517172697363302e52656365697074436c61696d60701b815260025afa1561050d575f5190606081015191815192602083015193604060808501519401938451519060038210156105f557945160209081015160408051808401978852908101959095526060850193909352608084019690965260a08301949094526001600160f81b031960f894851b811660c0840152931b90921660c4830152600160fa1b60c883015260aa82525f916105d560ca82610267565b604051918291518091835e8101838152039060025afa1561050d575f5190565b634e487b7160e01b5f52602160045260245ffdfea164736f6c634300081a000a")]
        contract MockVerifier {
            constructor(bytes4 selector) {}
        }
    }

    alloy::sol! {
        #[sol(rpc, bytecode = "60e0806040523461030f57610e9c803803809161001c8285610313565b833981019060608183031261030f5780516001600160a01b038116810361030f576020820151604083015190926001600160401b03821161030f570183601f8201121561030f578051906001600160401b0382116102fb576040519461008c601f8401601f191660200187610313565b8286526020838301011161030f57815f9260208093018388015e8501015260805260c081905281516001600160401b0381116102fb575f54600181811c911680156102f1575b60208210146102dd57601f811161027b575b50602092601f821160011461021c57928192935f92610211575b50508160011b915f199060031b1c1916175f555b60205f602b6040517f72697363302e536574496e636c7573696f6e526563656970745665726966696581526a72506172616d657465727360a81b8482015260025afa15610206575f602091815190604051908482019283526040820152600160f81b606082015260428152610188606282610313565b604051918291518091835e8101838152039060025afa15610206575f516001600160e01b03191660a052604051610b6590816103378239608051818181610494015281816106ac01526108ce015260a0518181816106ed0152610829015260c0518181816101280152818161051c0152818161094b0152610b130152f35b6040513d5f823e3d90fd5b015190505f806100fe565b601f198216935f8052805f20915f5b868110610263575083600195961061024b575b505050811b015f55610112565b01515f1960f88460031b161c191690555f808061023e565b9192602060018192868501518155019401920161022b565b5f80527f290decd9548b62a8d60345a988386fc84ba6bc95484008f6362f93160ef3e563601f830160051c810191602084106102d3575b601f0160051c01905b8181106102c857506100e4565b5f81556001016102bb565b90915081906102b2565b634e487b7160e01b5f52602260045260245ffd5b90607f16906100d2565b634e487b7160e01b5f52604160045260245ffd5b5f80fd5b601f909101601f19168101906001600160401b038211908210176102fb5760405256fe6080806040526004361015610012575f80fd5b5f905f3560e01c908163053c238d146106db5750806308c84e70146106975780631599ead51461062257806348cbdfca146105f35780636691f6471461045e578063ab750e75146101de578063cdc97123146100ca5763ffa1ad7414610076575f80fd5b346100c757806003193601126100c757506100c36040516100986040826107b8565b600a815269302e332e302d72632e3160b01b602082015260405191829160208352602083019061074a565b0390f35b80fd5b50346100c757806003193601126100c75760405190808054908160011c916001811680156101d4575b6020841081146101c057838652908115610199575060011461015a575b6100c384610120818603826107b8565b6040519182917f0000000000000000000000000000000000000000000000000000000000000000835260406020840152604083019061074a565b80805260208120939250905b80821061017f5750909150810160200161012082610110565b919260018160209254838588010152019101909291610166565b60ff191660208087019190915292151560051b850190920192506101209150839050610110565b634e487b7160e01b83526022600452602483fd5b92607f16926100f3565b50346100c75760603660031901126100c7576004356001600160401b03811161045a5761020f90369060040161071d565b9082608060405161021f8161076e565b8281528260208201526040516102348161079d565b838152836020820152604082015282606082015201526040516102568161079d565b83815283602082015260405161026b8161079d565b6044358152846020820191818352602082600c6040516b1c9a5cd8cc0b93dd5d1c1d5d60a21b815260025afa1561044d576020928251915190516040519185830193845260408301526060820152600160f91b6080820152606281526102d26082826107b8565b604051918291518091835e8101838152039060025afa1561044257835190604051906102fd8261076e565b602435825260208201907fa3acc27117418996340b84e5a90f3ef4c49d22c79e44aad822ec9c313e1eb8e282526040830190815260608301938785526080840190815260208860126040517172697363302e52656365697074436c61696d60701b815260025afa15610437578751945193519251905190825151926003841015610423575160209081015160408051808401998a52908101979097526060870195909552608086019190915260a08501919091526001600160f81b031960f892831b811660c08601529290911b90911660c4830152600160fa1b60c883015260aa82529185916103ee60ca826107b8565b604051918291518091835e8101838152039060025afa1561041857610415918351916107f9565b80f35b6040513d84823e3d90fd5b634e487b7160e01b8a52602160045260248afd5b6040513d89823e3d90fd5b6040513d85823e3d90fd5b50604051903d90823e3d90fd5b5080fd5b50346105ef5760403660031901126105ef576004356024356001600160401b0381116105ef5761049290369060040161071d565b7f00000000000000000000000000000000000000000000000000000000000000006001600160a01b031660205f816104c987610b0d565b604051918183925191829101835e8101838152039060025afa156105e4575f51813b156105ef575f90604051928380809363ab750e7560e01b82526060600483015261051960648301898b6107d9565b907f00000000000000000000000000000000000000000000000000000000000000006024840152604483015203915afa80156105e4576105ac575b50907fcb874ca5a04ca17d10924a9784b666fb412b518f2394912f61f4ddf614c5de1691838552600160205260408520600160ff198254161790556105a66040519283926020845260208401916107d9565b0390a280f35b7fcb874ca5a04ca17d10924a9784b666fb412b518f2394912f61f4ddf614c5de16929194505f6105db916107b8565b5f939091610554565b6040513d5f823e3d90fd5b5f80fd5b346105ef5760203660031901126105ef576004355f526001602052602060ff60405f2054166040519015158152f35b346105ef5760203660031901126105ef576004356001600160401b0381116105ef5780360360406003198201126105ef57600482013590602219018112156105ef5781016004810135906001600160401b0382116105ef576024019080360382136105ef576024610695930135916107f9565b005b346105ef575f3660031901126105ef576040517f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03168152602090f35b346105ef575f3660031901126105ef577f00000000000000000000000000000000000000000000000000000000000000006001600160e01b0319168152602090f35b9181601f840112156105ef578235916001600160401b0383116105ef57602083818601950101116105ef57565b805180835260209291819084018484015e5f828201840152601f01601f1916010190565b60a081019081106001600160401b0382111761078957604052565b634e487b7160e01b5f52604160045260245ffd5b604081019081106001600160401b0382111761078957604052565b90601f801991011681019081106001600160401b0382111761078957604052565b908060209392818452848401375f828201840152601f01601f1916010190565b929091926040516108098161079d565b60608152606060208201529280600411806105ef576001600160e01b03197f0000000000000000000000000000000000000000000000000000000000000000811690843516808203610af8575050600482116109b7575b505050815192905f915b84518310156108ac5760208360051b86010151908181105f1461089b575f52602052600160405f205b92019161086a565b905f52602052600160405f20610893565b60209093018051519194509150156109915760205f816108f660018060a01b037f000000000000000000000000000000000000000000000000000000000000000016945195610b0d565b604051918183925191829101835e8101838152039060025afa156105e4575f5191813b156105ef575f916109489160405180958194829363ab750e7560e01b845260606004850152606484019061074a565b907f00000000000000000000000000000000000000000000000000000000000000006024840152604483015203915afa80156105e4576109855750565b5f61098f916107b8565b565b505f52600160205260ff60405f205416156109a857565b63439cc0cd60e01b5f5260045ffd5b90919293506105ef57810190602081830360031901126105ef576004810135906001600160401b0382116105ef570190604082820360031901126105ef5760405191610a028361079d565b60048101356001600160401b0381116105ef5760049082010182601f820112156105ef578035906001600160401b038211610789578160051b60405192610a4c60208301856107b8565b8352602080840191830101918583116105ef57602001905b828210610ae857505050835260248101356001600160401b0381116105ef57600491010181601f820112156105ef578035906001600160401b0382116107895760405192610abc601f8401601f1916602001856107b8565b828452602083830101116105ef57815f92602080930183860137830101526020820152905f8080610860565b8135815260209182019101610a64565b632e2ce35360e21b5f5260045260245260445ffd5b604051907f00000000000000000000000000000000000000000000000000000000000000006020830152600160ff1b6040830152606082015260608152610b556080826107b8565b9056fea164736f6c634300081a000a")]
        contract SetVerifier {
            constructor(address verifier, bytes32 imageId, string memory imageUrl) {}
        }
    }

    alloy::sol! {
        #[sol(rpc, bytecode = "610100346101ab57601f615b4638819003918201601f19168301916001600160401b038311848410176101af578084926060946040528339810103126101ab578051906001600160a01b03821682036101ab576020810151604090910151916001600160a01b03831683036101ab573060805260a05260c05260e0527ff0c57e16840df040f15088dc2f81fe391c3923bec73e23a9662efc9c229c6a005460ff8160401c1661019c576002600160401b03196001600160401b03821601610133575b60405161598290816101c48239608051818181610f8301526110c7015260a05181818161033c01528181612c1f015261349f015260c0518181816116fe01528181611a500152612dc3015260e0518181816104d101528181610ba4015281816111be0152818161144901526144870152f35b6001600160401b0319166001600160401b039081177ff0c57e16840df040f15088dc2f81fe391c3923bec73e23a9662efc9c229c6a00556040519081527fc7f505b2f371ae2175ee4913f4499e1f2633a7b5936321eed1cdaeb6115181d290602090a15f6100c1565b63f92ee8a960e01b5f5260045ffd5b5f80fd5b634e487b7160e01b5f52604160045260245ffdfe60806040526004361015610011575f80fd5b5f3560e01c806308c84e701461031457806317e828c41461030f57806325d5971f1461030a5780632abff1f2146103055780632e1a7d4d14610300578063380f9c38146102fb5780633de6bd3f146102f657806341451f94146102f157806345bc4d10146102ec5780634f1ef286146102e7578063515990bc146102e257806352d1902d146102dd578063553c0248146102d85780635746d7f7146102d35780635b07fdd8146102ce5780635fbb4921146102c957806360dfd4a9146102c45780636a8bcd16146102bf57806370a08231146102ba578063715018a6146102b557806379ba5097146102b05780637c5fc3fb146102ab5780638094e614146102a657806381bf6c24146102a157806384b0196e1461029c57806384ed72fd146102975780638da5cb5b14610292578063956b09601461028d5780639fe9428c14610288578063aa9cb1d214610283578063ad3cb1cc1461027e578063ae7330f114610279578063b4206dd214610274578063c515c15f1461026f578063cb09e7c01461026a578063cb74db1114610265578063cb82cc8f14610260578063cdc971231461025b578063cfbebd8b14610256578063d0e30db014610251578063d4f51f2c1461024c578063da6a0eec14610247578063e30c397814610242578063f2800f1a1461023d578063f2fde38b14610238578063f399e22e14610233578063f7780f481461022e5763ffa1ad7414610229575f80fd5b611f11565b611e75565b611d03565b611c7b565b611c38565b611c04565b611bed565b611bd6565b611bc3565b611af1565b611a00565b6119e3565b6119c5565b61197e565b6118e1565b611888565b6117f4565b6117ad565b611721565b6116e7565b6116cb565b611697565b611655565b6115ab565b6114dc565b611419565b6113e4565b611398565b61131b565b6112d7565b6112c0565b6111ed565b6111a9565b611187565b611170565b61110c565b6110b5565b611099565b610f41565b610ab0565b61099f565b610904565b61075c565b61069a565b610595565b61040e565b6103f5565b610327565b5f91031261032357565b5f80fd5b34610323575f366003190112610323576040517f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03168152602090f35b90816101609103126103235790565b9181601f84011215610323578235916001600160401b038311610323576020838186019501011161032357565b906040600319830112610323576004356001600160401b03811161032357826103d29160040161036b565b91602435906001600160401b038211610323576103f19160040161037a565b9091565b346103235761040c610406366103a7565b9161217b565b005b346103235760203660031901126103235760043561043e61042e33612204565b5460601c6001600160601b031690565b6001600160601b0361045e61045284613eac565b6001600160601b031690565b91161061057f576104a661047182613eac565b6104a061047d33612204565b9161049383546001600160601b039060601c1690565b036001600160601b031690565b9061221d565b60405163a9059cbb60e01b8152336004820152602481018290526020816044815f6001600160a01b037f0000000000000000000000000000000000000000000000000000000000000000165af190811561057a575f9161054b575b501561053c5760405190815233907ff0ed97f7b968f9d8268bc8d104a11b3586ceeadd0e0af5f73769e2b479f9d0ae9080602081015b0390a2005b6312171d8360e31b5f5260045ffd5b61056d915060203d602011610573575b6105658183610e82565b81019061225a565b5f610501565b503d61055b565b61226f565b63112fed8b60e31b5f523360045260245ffd5b5ffd5b34610323576020366003190112610323576004356001600160401b038111610323576105c590369060040161037a565b6105cd613edd565b6001600160401b038111610695576105ef816105ea60025461227a565b6122b2565b5f601f821160011461062c57819061061c935f92610621575b50508160011b915f199060031b1c19161790565b600255005b013590505f80610608565b60025f52601f198216925f80516020615836833981519152915f5b85811061067d57508360019510610664575b505050811b01600255005b01355f19600384901b60f8161c191690555f8080610659565b90926020600181928686013581550194019101610647565b610e1d565b34610323576020366003190112610323576004356106c76106ba33612204565b546001600160601b031690565b6001600160601b036106db61045284613eac565b91161061057f576107126106ee82613eac565b61070c6106fa33612204565b9161049383546001600160601b031690565b9061246f565b5f80808084335af161072261248a565b501561053c5760405190815233907f7fcf532c15f0a6db0bd6d0e038bea71d30d808c7d98cb3bf7268a95bf5081b65908060208101610537565b610765366103a7565b9091346108db575b8035926040519160408352846040840152602081013590609e1981360301821215610323577f514a642174f202700c54726383c13321326925ff87df30f0bfbf49d9adfc41a69484936108ce6108c06108a285610537970161016060608a015280356101a08a015260208101356107e381610e0c565b6001600160a01b03166101c08a01526001600160601b0361080660408301611f2c565b166101e08a0152610883610871608061086a8c61028061085861082c60608901896124b9565b60a0610200850152803561083f81611f40565b610848816124e1565b61024085015260208101906124f0565b91909260406102608201520191612521565b9301611faa565b6001600160e01b0319166102208b0152565b61089060408801886124f0565b8a8303603f190160808c015290612521565b6108af60608701876124b9565b888203603f190160a08a0152612541565b93608060c08801910161257b565b8483036020860152612521565b6108e3613302565b61076d565b908160c09103126103235790565b908160809103126103235790565b34610323576080366003190112610323576004356001600160401b0381116103235761093490369060040161036b565b6024356001600160401b0381116103235761095390369060040161037a565b6044929192356001600160401b038111610323576109759036906004016108e8565b90606435936001600160401b0385116103235761099961040c9536906004016108f6565b936125f3565b34610323576020366003190112610323576004356109bc81613178565b15610a9e575f525f602052610a9a610a8060405f20610a7b610a6a6001604051936109e685610e31565b610a39610a2f8254848060a01b03811688526001600160401b038160a01c166020890152610a29610a1d8262ffffff9060e01c1690565b62ffffff1660408a0152565b60f81c90565b60ff166060870152565b01546001600160601b03811660808501526001600160601b03606082901c1660a08501526001600160c01b03191690565b6001600160c01b03191660c0830152565b613efd565b6040516001600160401b0390911681529081906020820190565b0390f35b63d2be005d60e01b5f5260045260245ffd5b3461032357602036600319011261032357600435610ae0610ad082613996565b610adb829392612204565b613f31565b5015610df857610aff610afa835f525f60205260405f2090565b612608565b6060810151600416610de4576060810151600116610dd057610b2f610b2382613efd565b6001600160401b031690565b431115610da257610b77610b4a845f525f60205260405f2090565b80546001600160f81b03811660f891821c60041790911b6001600160f81b0319161781556001905f910155565b60a08101610ba2610b9a610b9561045284516001600160601b031690565b612685565b612710900490565b7f00000000000000000000000000000000000000000000000000000000000000006001600160a01b031693909190843b1561032357604051630852cd8d60e31b815260048101849052945f908690602490829084905af194851561057a57610c4c84610c4761045261053796610c51957f79ca7c80cf57b513ffdf8aa37ec70e40757f5e0d35219241860bb4b4c2fa76169b610d88575b50516001600160601b031690565b61270f565b613eac565b84519094906001600160a01b031691610c71826060600291015116151590565b15610d1b5750610cc7610c9782610cba610c9c610c97610cea965160018060a01b031690565b612204565b6104a08a610cb583546001600160601b039060601c1690565b61271c565b516001600160a01b031690565b805460ff60c01b19811660c091821c60ff1660011790911b60ff60c01b16179055565b604080519384526001600160601b0390941660208401526001600160a01b0316928201929092529081906060820190565b9150610cc7610c97610cea92610d833095610d51610d3830612204565b6104a08c610cb583546001600160601b039060601c1690565b61070c610d71610d6b60808601516001600160601b031690565b92612204565b91610cb583546001600160601b031690565b610cba565b80610d965f610d9c93610e82565b80610319565b5f610c39565b82610daf61059292613efd565b63079c66ab60e41b5f526004919091526001600160401b0316602452604490565b631cfdeebb60e01b5f52600483905260245ffd5b633231064d60e11b5f52600483905260245ffd5b63d2be005d60e01b5f52600482905260245ffd5b6001600160a01b0381160361032357565b634e487b7160e01b5f52604160045260245ffd5b60e081019081106001600160401b0382111761069557604052565b604081019081106001600160401b0382111761069557604052565b608081019081106001600160401b0382111761069557604052565b90601f801991011681019081106001600160401b0382111761069557604052565b60405190610eb260a083610e82565b565b60405190610eb2604083610e82565b60405190610eb260e083610e82565b6001600160401b03811161069557601f01601f191660200190565b929192610ef982610ed2565b91610f076040519384610e82565b829481845281830111610323578281602093845f960137010152565b9080601f8301121561032357816020610f3e93359101610eed565b90565b604036600319011261032357600435610f5981610e0c565b6024356001600160401b03811161032357610f78903690600401610f23565b906001600160a01b037f000000000000000000000000000000000000000000000000000000000000000016308114908115611077575b5061106857610fbb613edd565b6040516352d1902d60e01b8152916020836004816001600160a01b0386165afa5f9381611037575b5061100457634c9c8ce360e01b5f526001600160a01b03821660045260245ffd5b905f805160206158d683398151915283036110235761040c9250614ce5565b632a87526960e21b5f52600483905260245ffd5b61105a91945060203d602011611061575b6110528183610e82565b810190614008565b925f610fe3565b503d611048565b63703e46dd60e11b5f5260045ffd5b5f805160206158d6833981519152546001600160a01b0316141590505f610fae565b34610323575f36600319011261032357602060405161c3508152f35b34610323575f366003190112610323577f00000000000000000000000000000000000000000000000000000000000000006001600160a01b031630036110685760206040515f805160206158d68339815191528152f35b34610323575f3660031901126103235760206040515f8152f35b906040600319830112610323576004356001600160401b0381116103235782611151916004016108e8565b91602435906001600160401b03821161032357610f3e916004016108f6565b346103235761040c61118136611126565b90612bd8565b34610323575f3660031901126103235760206111a1614de8565b604051908152f35b34610323575f366003190112610323576040517f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03168152602090f35b34610323576020366003190112610323576004355f525f602052610a9a61123260405f20611226610a6a6001604051936109e685610e31565b60600151600416151590565b60405190151581529081906020820190565b9181601f84011215610323578235916001600160401b038311610323576020808501948460051b01011161032357565b6040600319820112610323576004356001600160401b038111610323578161129e91600401611244565b92909291602435906001600160401b03821161032357610f3e916004016108f6565b346103235761040c6112d136611274565b91612f28565b34610323576020366003190112610323576004356112f481610e0c565b60018060a01b03165f52600160205260206001600160601b0360405f205416604051908152f35b34610323575f36600319011261032357611333613edd565b5f8051602061595683398151915280546001600160a01b03199081169091555f80516020615876833981519152805491821690555f906001600160a01b03167f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e08280a3005b34610323575f366003190112610323575f8051602061595683398151915254336001600160a01b03909116036113d15761040c336143e3565b63118cdaa760e01b5f523360045260245ffd5b34610323575f36600319011261032357335f908152600160205260409020805460ff60c01b198116607f60c11b909116179055005b346103235760a0366003190112610323576004356024359060443560ff81168091036103235760643590608435907f00000000000000000000000000000000000000000000000000000000000000006001600160a01b0316803b156103235761040c955f60e4928195604051978896879563d505accf60e01b87523360048801523060248801528b60448801526064870152608486015260a485015260c48401525af16114c8575b503361444e565b80610d965f6114d693610e82565b5f6114c1565b346103235760203660031901126103235760206114fa6004356130b3565b6040519015158152f35b805180835260209291819084018484015e5f828201840152601f01601f1916010190565b92939161154a61155892600f60f81b865260e0602087015260e0860190611504565b908482036040860152611504565b92606083015260018060a01b031660808201525f60a082015260c0818303910152602080835192838152019201905f5b8181106115955750505090565b8251845260209384019390920191600101611588565b34610323575f366003190112610323575f805160206158b683398151915254158061163f575b15611602576115de613188565b6115e6613255565b90610a9a6115f26130e1565b6040519384933091469186611528565b60405162461bcd60e51b81526020600482015260156024820152741152540dcc4c8e88155b9a5b9a5d1a585b1a5e9959605a1b6044820152606490fd5b505f8051602061593683398151915254156115d1565b346103235760203660031901126103235760043561167281610e0c565b6001600160a01b03165f9081526001602081815260409092205460c01c1615156114fa565b34610323575f366003190112610323575f80516020615876833981519152546040516001600160a01b039091168152602090f35b34610323575f366003190112610323576020604051611d4c8152f35b34610323575f3660031901126103235760206040517f00000000000000000000000000000000000000000000000000000000000000008152f35b346103235760a03660031901126103235760043561173e81610e0c565b602435906044356001600160401b0381116103235761176190369060040161037a565b906064356001600160401b03811161032357611781903690600401611244565b929091608435956001600160401b038711610323576117a761040c9736906004016108f6565b95613113565b34610323575f36600319011261032357610a9a6040516117ce604082610e82565b60058152640352e302e360dc1b6020820152604051918291602083526020830190611504565b34610323575f60603660031901126103235760043561181281610e0c565b6024356044356001600160401b0381116103235761183490369060040161037a565b90926001600160a01b0316803b156103235761186a935f809460405196879586948593636691f64760e01b8552600485016130fc565b03925af1801561057a5761187c575080f35b61040c91505f90610e82565b346103235761040c611899366103a7565b90916118cb6118a88235613996565b939094856118c66118c16118bc36886120e1565b613be0565b613d37565b613d5d565b916118d781858461457d565b94909333936146d8565b34610323576020366003190112610323576004355f9081526020818152604091829020805460019091015483516001600160a01b038316815260a083811c6001600160401b03169482019490945260e083811c62ffffff169582019590955260f89290921c6060808401919091526001600160601b0380831660808501529082901c16928201929092526001600160c01b031990911660c0820152f35b346103235760203660031901126103235760043561199b81610e0c565b60018060a01b03165f52600160205260206001600160601b0360405f205460601c16604051908152f35b346103235760203660031901126103235760206114fa600435613178565b346103235760203660031901126103235761040c6004353361444e565b34610323575f366003190112610323576040515f600254611a208161227a565b8084529060018116908115611acd5750600114611a82575b610a9a83611a4881850382610e82565b6040519182917f00000000000000000000000000000000000000000000000000000000000000008352604060208401526040830190611504565b60025f9081525f80516020615836833981519152939250905b808210611ab357509091508101602001611a48611a38565b919260018160209254838588010152019101909291611a9b565b60ff191660208086019190915291151560051b84019091019150611a489050611a38565b34610323576060366003190112610323576004356001600160401b03811161032357611b2190369060040161036b565b6024356001600160401b03811161032357611b4090369060040161037a565b90916044356001600160401b03811161032357611b99611ba193611ba761040c96611b72611bb095369060040161037a565b9790611b7e8835613996565b969095611b916118c16118bc368d6120e1565b948786613d5d565b983691610eed565b9061541f565b90949194615463565b611bbb82828661457d565b9590946146d8565b5f3660031901126103235761040c613302565b346103235761040c611be736611126565b9061335c565b346103235761040c611bfe36611274565b91613461565b34610323575f366003190112610323575f80516020615956833981519152546040516001600160a01b039091168152602090f35b3461032357602036600319011261032357600435611c5581613178565b15610a9e575f525f60205260206001600160401b0360405f205460a01c16604051908152f35b3461032357602036600319011261032357600435611c9881610e0c565b611ca0613edd565b5f8051602061595683398151915280546001600160a01b0319166001600160a01b039283169081179091555f80516020615876833981519152549091167f38d16b8cac22d99fc7c124b9cd0de2d3fa1faef420bfe791d8c362d765e227005f80a3005b3461032357604036600319011261032357600435611d2081610e0c565b6024356001600160401b03811161032357611d3f90369060040161037a565b5f805160206159168339815191525492916001600160401b03611d7a60ff604087901c1615611d6d565b1590565b956001600160401b031690565b1680159081611e6d575b6001149081611e63575b159081611e5a575b50611e4b57611dd99284611dd060016001600160401b03195f805160206159168339815191525416175f8051602061591683398151915255565b611e275761373b565b611ddf57005b5f80516020615916833981519152805460ff60401b19169055604051600181527fc7f505b2f371ae2175ee4913f4499e1f2633a7b5936321eed1cdaeb6115181d290602090a1005b5f80516020615916833981519152805460ff60401b1916600160401b17905561373b565b63f92ee8a960e01b5f5260045ffd5b9050155f611d96565b303b159150611d8e565b859150611d84565b34610323576080366003190112610323576004356001600160401b03811161032357611ea5903690600401611244565b906024356001600160401b03811161032357611ec5903690600401611244565b906044356001600160401b03811161032357611ee5903690600401611244565b929091606435956001600160401b03871161032357611f0b61040c9736906004016108f6565b95613931565b34610323575f36600319011261032357602060405160018152f35b35906001600160601b038216820361032357565b6002111561032357565b91906040838203126103235760405190611f6382610e4c565b81938035611f7081611f40565b83526020810135916001600160401b03831161032357602092611f939201610f23565b910152565b6001600160e01b031981160361032357565b3590610eb282611f98565b91908281039260a084126103235760405191611fd083610e67565b6040839583358552601f19011261032357604051611fed81610e4c565b6020830135611ffb81610e0c565b815261200960408401611f2c565b602082015260208401526060820135906001600160401b038211610323578261203b608092611f939460609601611f4a565b604086015201611faa565b35906001600160401b038216820361032357565b359063ffffffff8216820361032357565b91908260e09103126103235760405161208381610e31565b60c080829480358452602081013560208501526120a260408201612046565b60408501526120b36060820161205a565b60608501526120c46080820161205a565b60808501526120d560a0820161205a565b60a08501520135910152565b91909161016081840312610323576120f7610ea3565b928135845260208201356001600160401b038111610323578161211b918401611fb5565b602085015260408201356001600160401b038111610323578161213f918401610f23565b604085015260608201356001600160401b0381116103235782612169836080936121749601611f4a565b60608701520161206b565b6080830152565b91610c4c6121cb6020936121c260806121b06121db968935948a6118c66118c16118bc6121a78a613996565b509336906120e1565b9701916121bd368461206b565b614c16565b5050369061206b565b6001600160401b03431690613dde565b6001600160601b03604051916121f083610e4c565b600183521691018190526001607f1b17905d565b6001600160a01b03165f90815260016020526040902090565b80546bffffffffffffffffffffffff60601b191660609290921b6bffffffffffffffffffffffff60601b16919091179055565b8015150361032357565b908160209103126103235751610f3e81612250565b6040513d5f823e3d90fd5b90600182811c921680156122a8575b602083101461229457565b634e487b7160e01b5f52602260045260245ffd5b91607f1691612289565b601f81116122be575050565b60025f5260205f20906020601f840160051c830193106122f8575b601f0160051c01905b8181106122ed575050565b5f81556001016122e2565b90915081906122d9565b601f811161230e575050565b5f805160206158568339815191525f5260205f20906020601f840160051c83019310612354575b601f0160051c01905b818110612349575050565b5f815560010161233e565b9091508190612335565b601f821161236b57505050565b5f5260205f20906020601f840160051c830193106123a3575b601f0160051c01905b818110612398575050565b5f815560010161238d565b9091508190612384565b91906001600160401b038111610695576123d3816123cc60025461227a565b600261235e565b5f601f821160011461240557819061240093945f926106215750508160011b915f199060031b1c19161790565b600255565b60025f52601f198216935f80516020615836833981519152915f5b868110612457575083600195961061243e575b505050811b01600255565b01355f19600384901b60f8161c191690555f8080612433565b90926020600181928686013581550194019101612420565b906001600160601b03166001600160601b0319825416179055565b3d156124b4573d9061249b82610ed2565b916124a96040519384610e82565b82523d5f602084013e565b606090565b9035603e1982360301811215610323570190565b634e487b7160e01b5f52602160045260245ffd5b600211156124eb57565b6124cd565b9035601e19823603018112156103235701602081359101916001600160401b03821161032357813603831361032357565b908060209392818452848401375f828201840152601f01601f1916010190565b90604061256b610f3e93803561255681611f40565b61255f816124e1565b845260208101906124f0565b9190928160208201520191612521565b60c0809180358452602081013560208501526001600160401b036125a160408301612046565b16604085015263ffffffff6125b86060830161205a565b16606085015263ffffffff6125cf6080830161205a565b16608085015263ffffffff6125e660a0830161205a565b1660a08501520135910152565b9161260391610eb295949361217b565b61335c565b90610eb260405161261881610e31565b83546001600160a01b038116825260a081901c6001600160401b0316602083015260e081901c62ffffff16604083015260f81c606082015292839060c09061266290600190610a39565b6001600160c01b031916910152565b634e487b7160e01b5f52601160045260245ffd5b90611d4c820291808304611d4c149015171561269d57565b612671565b908160011b918083046002149015171561269d57565b600181901b91906001600160ff1b0381160361269d57565b8181029291811591840414171561269d57565b81156126ed570490565b634e487b7160e01b5f52601260045260245ffd5b5f1981019190821161269d57565b9190820391821161269d57565b906001600160601b03809116911601906001600160601b03821161269d57565b903590601e198136030182121561032357018035906001600160401b0382116103235760200191813603831361032357565b908092918237015f815290565b6020815260406020612797845183838601526060850190611504565b93015191015290565b6001600160401b0381116106955760051b60200190565b604080519091906127c88382610e82565b6001815291601f1901366020840137565b906127e3826127a0565b6127f06040519182610e82565b8281528092612801601f19916127a0565b0190602036910137565b634e487b7160e01b5f52603260045260245ffd5b80511561282c5760200190565b61280b565b805182101561282c5760209160051b010190565b903590601e198136030182121561032357018035906001600160401b03821161032357602001918160061b3603831361032357565b903590601e198136030182121561032357018035906001600160401b0382116103235760200191606082023603831361032357565b35610f3e81610e0c565b61ffff81160361032357565b919082606091031261032357604051606081018181106001600160401b03821117610695576040526040611f938183958035612900816128b9565b8552602081013561291081610e0c565b602086015201611f2c565b90929192612928816127a0565b936129366040519586610e82565b606060208684815201920283019281841161032357915b83831061295a5750505050565b602060609161296984866128c5565b81520192019161294d565b929192612980826127a0565b9361298e6040519586610e82565b602085848152019260061b82019181831161032357925b8284106129b25750505050565b60408483031261032357602060409182516129cc81610e4c565b86356129d7816128b9565b8152828701356129e681611f98565b838201528152019301926129a5565b90602080835192838152019201905f5b818110612a125750505090565b8251805161ffff1685526020818101516001600160a01b0316818701526040918201516001600160601b03169186019190915260609094019390920191600101612a05565b90602080835192838152019201905f5b818110612a745750505090565b8251805161ffff1685526020908101516001600160e01b0319168186015260409094019390920191600101612a67565b6020815260c0810182519060a060208401528151809152602060e084019201905f5b818110612b25575050509060a06080612b0b612af5610f3e956020880151601f198783030160408801526129f5565b6040870151858203601f19016060870152612a57565b6060860151848301529401516001600160a01b0316910152565b8251845260209384019390920191600101612ac6565b805191908290602001825e015f815290565b901561282c5790565b919081101561282c5760061b0190565b35610f3e81611f98565b906004116103235790600490565b356001600160e01b0319811692919060048210612b99575050565b6001600160e01b031960049290920360031b82901b16169150565b90612bce9060409396959496606084526060840191612521565b9460208201520152565b90604082013560205f612bee606086018661273c565b90612bfe6040518093819361276e565b039060025afa1561057a57612c18612c1d915f5190614039565b614124565b7f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03169290919060808101612c6e612c5c828461273c565b9190612c66610eb4565b923691610eed565b8152846020820152853b15610323575f612c9c9160405180938192631599ead560e01b83526004830161277b565b03818961c350fa801561057a57612eb0575b50612cb76127b7565b6020830135612cc58261281f565b526040840194612cd58686612845565b9190612ce4602088018861287a565b9390612cf260608a016128af565b94612cfb610ea3565b9687523690612d099261291b565b60208601523690612d1992612974565b604084015260608301526001600160a01b0316608082015260405180916020820190612d4491612aa4565b03601f1981018252612d569082610e82565b604051612d64818093612b3b565b03905a915f916002602094fa1561057a575f5193612d828185612845565b9050151580612e6a575b612e0b5750505080612d9d9161273c565b919092803b1561032357612dec935f936040519586948593849363ab750e7560e01b85527f00000000000000000000000000000000000000000000000000000000000000009160048601612bb4565b03915afa801561057a57612dfd5750565b80610d965f610eb293610e82565b612e3c612e4892612e366020612e30612e2a612e42966105929a612845565b90612b4d565b01612b66565b9461273c565b90612b70565b90612b7e565b632e2ce35360e21b5f526001600160e01b031991821660045216602452604490565b50612e7d6020612e30612e2a8488612845565b612e9d612e90612e42612e3c868861273c565b6001600160e01b03191690565b6001600160e01b03199091161415612d8c565b80610d965f612ebe93610e82565b5f612cae565b919081101561282c576060020190565b610f3e9036906128c5565b919081101561282c5760051b8101359060be1981360301821215610323570190565b9290612f1a90610f3e9593604086526040860191612521565b926020818503910152612521565b90612f34838284613461565b60208301612f42818561287a565b9190505f5b828110612ffa575050505f5b818110612f605750505050565b80612f83612f716001938587612edf565b612f7d606088016128af565b906142f6565b612f8e818486612edf565b357fcc611d18a8ca7881d594afc83d6cb63ae83d28d155975309f5ff633ecc642730612fc8612fbe848789612edf565b606081019061273c565b90612ff1612fe4612fda878a8c612edf565b608081019061273c565b9060405194859485612f01565b0390a201612f53565b8061301961301460019361300e868b61287a565b90612ec4565b612ed4565b61303961303261302b835161ffff1690565b61ffff1690565b8789612edf565b908135613048611d69826130b3565b613056575b50505001612f47565b60208201516130ab936130a391613083906040906001600160a01b03165b9501516001600160601b031690565b604082013590613096606084018461273c565b949093608081019061273c565b969095614211565b5f808061304d565b6130bf6130dc91613996565b6001600160a01b039091165f908152600160205260409020613f31565b905090565b604051906130f0602083610e82565b5f808352366020840137565b604090610f3e949281528160208201520191612521565b919594939290916001600160a01b0316803b156103235761314e965f8094604051998a9586948593636691f64760e01b8552600485016130fc565b03925af193841561057a57610eb294613168575b50612f28565b5f61317291610e82565b5f613162565b6130bf61318491613996565b5090565b604051905f825f8051602061585683398151915254916131a78361227a565b808352926001811690811561323657506001146131cb575b610eb292500383610e82565b505f805160206158568339815191525f90815290917f42ad5d3e1f2e6e70edcf6d991b8a3023d3fca8047a131592f9edb9fd9b89d57d5b81831061321a575050906020610eb2928201016131bf565b6020919350806001915483858901015201910190918492613202565b60209250610eb294915060ff191682840152151560051b8201016131bf565b604051905f825f8051602061589683398151915254916132748361227a565b8083529260018116908115613236575060011461329757610eb292500383610e82565b505f805160206158968339815191525f90815290917f5f9ce34815f8e11431c7bb75a8e6886a91478f7ffc1dbb0a98dc240fddd76b755b8183106132e6575050906020610eb2928201016131bf565b60209193508060019154838589010152019101909184926132ce565b61332e61330e34613eac565b335f52600160205261070c60405f20916001600160601b0383541661271c565b6040513481527fe1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c60203392a2565b9061338c6060613392926133708186612bd8565b6020810161337e818361287a565b90506133db575b50016128af565b826142f6565b7fcc611d18a8ca7881d594afc83d6cb63ae83d28d155975309f5ff633ecc642730612fe48235926133d66133c9606083018361273c565b939092608081019061273c565b0390a2565b613014612e2a6133eb928461287a565b8535906133fa611d69836130b3565b613405575b50613385565b6020810151613450929061342e906040906001600160a01b03169301516001600160601b031690565b604089013590613440878b018b61273c565b9290916130a360808d018d61273c565b5f806133ff565b35610f3e816128b9565b919061ffff81116137235791613476836127d9565b90613480846127d9565b9160408401916134908386612845565b90505f5b8181106136935750507f00000000000000000000000000000000000000000000000000000000000000006001600160a01b031695859291905f5b818110613597575050506135746020946135586135175f9661353e6135809760606135336134fe6135669a61491b565b9461352061350e8f86018661287a565b98909286612845565b949095016128af565b96613529610ea3565b998a52369161291b565b8c8801523691612974565b604085015260608401526001600160a01b03166080830152565b604051928391878301612aa4565b03601f198101835282610e82565b60405191828092612b3b565b039060025afa1561057a57612d9d5f51918061273c565b90919293506135a7818385612edf565b60208101356135b68389612831565b5260408101359060205f6135cd606084018461273c565b906135dd6040518093819361276e565b039060025afa1561057a576135fa612c1861360f935f5190614039565b6136048488612831565b52608081019061273c565b919061363061361e8388612831565b5191613628610eb4565b943691610eed565b83526020830152883b15610323575f61365d9260405180948192631599ead560e01b83526004830161277b565b03818c61c350fa91821561057a5760019261367f575b500190869392916134ce565b80610d965f61368d93610e82565b5f613673565b866136df612e42612e3c612fda878d6136d9999e9961302b6136d48a8f806136ce6020612e308f6136c86136c8968892612845565b90612b56565b9c612845565b613457565b91612edf565b906001600160e01b031980831690821603613701575050600101969196613494565b632e2ce35360e21b5f526001600160e01b03199081166004521660245260445ffd5b6377e4aa5360e11b5f5260045261ffff60245260445ffd5b929190926137476152d9565b61374f6152d9565b6001600160a01b0381161561391e57613767906143e3565b61376f6152d9565b604092835161377e8582610e82565b601081526f12509bdd5b991b195cdcd3585c9ad95d60821b60208201526137a785519586610e82565b60018552603160f81b60208601526137bd6152d9565b6137c56152d9565b8051906001600160401b038211610695576137f6826137f15f805160206158568339815191525461227a565b612302565b602090601f831160011461387b578261384193610eb297989361382d935f926138705750508160011b915f199060031b1c19161790565b5f8051602061585683398151915255615304565b6138565f5f805160206158b683398151915255565b61386b5f5f8051602061593683398151915255565b6123ad565b015190505f80610608565b5f805160206158568339815191525f52601f19831691907f42ad5d3e1f2e6e70edcf6d991b8a3023d3fca8047a131592f9edb9fd9b89d57d925f5b818110613906575092600192859261384196610eb29a9b96106138ee575b505050811b015f8051602061585683398151915255615304565b01515f1960f88460031b161c191690555f80806138d4565b929360206001819287860151815501950193016138b6565b631e4fbdf760e01b5f525f60045260245ffd5b96929395919490955f5b87811015613984578060051b90818a01359161015e198b360301831215610323578782101561282c5760019261397661397e928b018b61273c565b918d0161217b565b0161393b565b5092965092509250610eb29350612f28565b906001600160c01b031982166139be57602082901c6001600160a01b03169163ffffffff1690565b6341abc80160e01b5f5260045ffd5b604051906139dc606083610e82565b60268252654c696d69742960d01b6040837f43616c6c6261636b286164647265737320616464722c75696e7439362067617360208201520152565b60405190613a26606083610e82565b60218252602960f81b6040837f496e7075742875696e743820696e707574547970652c6279746573206461746160208201520152565b60405190613a6b60c083610e82565b6084825263616b652960e01b60a0837f4f666665722875696e74323536206d696e50726963652c75696e74323536206d60208201527f617850726963652c75696e7436342062696464696e6753746172742c75696e7460408201527f33322072616d705570506572696f642c75696e743332206c6f636b54696d656f60608201527f75742c75696e7433322074696d656f75742c75696e74323536206c6f636b537460808201520152565b60405190613b25606083610e82565b602982526874657320646174612960b81b6040837f5072656469636174652875696e743820707265646963617465547970652c627960208201520152565b60405190613b72608083610e82565b605382527274652c6279746573342073656c6563746f722960681b6060837f526571756972656d656e7473286279746573333220696d61676549642c43616c60208201527f6c6261636b2063616c6c6261636b2c507265646963617465207072656469636160408201520152565b604051613bee608082610e82565b605a81527f50726f6f66526571756573742875696e743235362069642c526571756972656d60208201527f656e747320726571756972656d656e74732c737472696e6720696d616765557260408201527f6c2c496e70757420696e7075742c4f66666572206f66666572290000000000006060820152613cb5613cbb613c726139cd565b613566613c7d613a17565b613cb5613c88613a5c565b613cb5613c93613b16565b91613cb5613c9f613b63565b956040519a8b99613cb560208c019e8f90612b3b565b90612b3b565b51902090613d318151613566613cd46020850151614a01565b9360408101516020815191012090613cfc6080613cf46060840151614af3565b920151614b47565b9160405196879560208701998a9260a094919796959260c0850198855260208501526040840152606083015260808201520152565b51902090565b604290613d42614de8565b906040519161190160f01b8352600283015260228201522090565b92613d70613d7f93613d76923691610eed565b8461541f565b90939193615463565b6001600160a01b03908116911603613d945790565b638baa579f60e01b5f5260045ffd5b906001600160401b03809116911601906001600160401b03821161269d57565b906001820180921161269d57565b9190820180921161269d57565b6040810191613df483516001600160401b031690565b906001600160401b03808316911690811115613ea457613e35610b236060850193613e2f613e26865163ffffffff1690565b63ffffffff1690565b90613da3565b811115613e4757505060209150015190565b92613e99613e9e92613e91610f3e96613e8b610b23613e7d613e26613e7260208c01518c519061270f565b965163ffffffff1690565b96516001600160401b031690565b9061270f565b9451946126d0565b6126e3565b90613dd1565b505090505190565b6001600160601b038111613ec6576001600160601b031690565b6306dfcc6560e41b5f52606060045260245260445ffd5b5f80516020615876833981519152546001600160a01b031633036113d157565b610f3e9062ffffff60406001600160401b036020840151169201511690613da3565b630200000082101561282c5701905f90565b9063ffffffff16601c811015613fa757613f8666ffffffffffffff613f5a613f97945460c81c90565b613f7e6003613f6b610b23876126a2565b6001600160401b038080931691161b1690565b1616916126a2565b6001600160401b03809216901c1690565b9060026001831615159216151590565b613fea613fe4613fda613fbe601c613ff09561270f565b946001613fd3613fcd886126a2565b60081c90565b9101613f1f565b90549060031b1c90565b926126a2565b60ff1690565b906003821b16901c9060026001831615159216151590565b90816020910312610323575190565b6040519061402482610e4c565b5f6020838281520152565b600311156124eb57565b9060405160a081018181106001600160401b03821117610695575f9160809160405282815282602082015261406c614017565b604082015282606082015201526140a4614084610eb4565b915f83525f6020840152614096610eb4565b9081525f6020820152614d87565b906140ad610ea3565b9283527fa3acc27117418996340b84e5a90f3ef4c49d22c79e44aad822ec9c313e1eb8e2602084015260408301525f6060830152608082015290565b60205f60126040517172697363302e52656365697074436c61696d60701b815260025afa1561057a575f5190565b5160038110156124eb5790565b5f6141df6020926135746141366140e9565b613566606084015193805190888101519060406080820151910190614187614171613fea8d61417d6141688751614117565b6141718161402f565b60181b63ff0000001690565b9551015160ff1690565b604080518d8101988952602089019a909a52870194909452606086019290925260808501919091526001600160e01b031960e091821b811660a086015291901b1660a4830152600160fa1b60a8830152839160aa0190565b039060025afa1561057a575f5190565b6001600160a01b039091168152604060208201819052610f3e92910190611504565b96909594929392906001600160a01b038716803b15610323576001600160601b035f966142796142679989956040519b8c9a8b998a9663a12da43f60e01b88526004880152606060248801526064870191612521565b84810360031901604486015291612521565b039416f190816142c7575b506142c3577f5c5960582bfc7a494183b4e9a66bfe8ecffc07a83a48d136e732400f7b98bf50906142b361248a565b906133d6604051928392836141ef565b5050565b80610d965f6142d593610e82565b5f614284565b35610f3e81612250565b906020610f3e928181520190611504565b919082359061430482613996565b9061431282610adb83612204565b9290156143d35761432d610afa865f525f60205260405f2090565b9161434260208401516001600160401b031690565b436001600160401b0391909116106143be576143639560208901359361511d565b915b825161437057509050565b60a061437c91016142db565b156143875750615287565b906143b97f210e4fd706e561df48472433bcc50b4589f2c13e784e9992f4c3e6de26eb356491604051918291826142e5565b0390a1565b6143cd95602089013593614f9a565b91614365565b6143cd9491602088013592614e49565b5f8051602061595683398151915280546001600160a01b03199081169091555f8051602061587683398151915280549182166001600160a01b0393841690811790915591167f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e05f80a3565b6040516323b872dd60e01b602082019081526001600160a01b0380841660248401819052306044850152606480850187905284529493927f0000000000000000000000000000000000000000000000000000000000000000909116916144d2915f9182916144bd608482610e82565b519082865af16144cb61248a565b90836157d7565b805190811515918261455f575b50506145445750816145346133d6926104a061451e610d6b7f1da2c4060997c108162f55c30ad2268924b069210f8c686ba302e1acc6909ccc97613eac565b91610cb583546001600160601b039060601c1690565b6040519081529081906020820190565b635274afe760e01b5f526001600160a01b031660045260245ffd5b614576925090602080611d6993830101910161225a565b5f806144df565b9290926145b761459c614593366080850161206b565b92358093614c16565b94909560018060a01b03165f52600160205260405f20613f31565b906145d5576145c35750565b631cfdeebb60e01b5f5260045260245ffd5b5063a905765160e01b5f5260045260245ffd5b906001600160401b03809116911603906001600160401b03821161269d57565b815160208301516040840151606085015160f81b6001600160f81b03191667ffffffffffffffff60a01b60a09390931b929092166001600160a01b039093169290921762ffffff60e01b60e09390931b9290921691909117178155610eb291906146bc9060c0906001019261469061468a60808301516001600160601b031690565b8561246f565b6146ad6146a760a08301516001600160601b031690565b8561221d565b01516001600160c01b03191690565b81546001600160c01b03166001600160c01b0319909116179055565b919394956146e984610adb87612204565b90614907576148f357614705610c4c6121cb366080870161206b565b9061470f86612204565b9561472187546001600160601b031690565b906001600160601b0384166001600160601b038316106148d8575061474588612204565b90614757826001905460c01c16151590565b6148bc578154610140870135949392919060601c6001600160601b031685116148a0578a91908490036001600160601b0316614793908a61246f565b61479c85613eac565b815460601c6001600160601b0316036001600160601b03166147bd9161221d565b6147c6916145e8565b6001600160401b03166147d89061528f565b916147e290613eac565b916147eb610ec3565b6001600160a01b0389168152986001600160401b031660208a015262ffffff1660408901525f60608901526001600160601b031660808801526001600160601b031660a08701526001600160c01b03191660c08601523593614854855f525f60205260405f2090565b9061485e91614608565b6148679161562a565b6040516001600160a01b039190911681527ffb1dd4383bc31d469e205e04236dbca09ae13ab0373414dccbe098a861ddef5590602090a2565b63112fed8b60e31b5f526001600160a01b038a1660045260245ffd5b6327951b3f60e11b5f526001600160a01b03891660045260245ffd5b63112fed8b60e31b5f526001600160a01b031660045260245ffd5b631cfdeebb60e01b5f52823560045260245ffd5b63a905765160e01b5f52833560045260245ffd5b8051156139be5760018151146149f8578051905b6001821161494557614941915061281f565b5190565b61495761495183613dc3565b60011c90565b925f5b6149648460011c90565b8110156149b657806149a561498361497d6001946126b8565b86612831565b5161499e614998614993856126b8565b613dc3565b87612831565b51906152b8565b6149af8286612831565b520161495a565b509290916001808216146149cc575b509061492f565b6149d86149de91612701565b83612831565b516149f16149eb83612701565b84612831565b525f6149c5565b6149419061281f565b614a09613b63565b613cb5614a36614a176139cd565b613566614a22613b16565b604051948593613cb5602086018099612b3b565b51902090613d3181516135666020840151614a4f6139cd565b60208151910120906001600160601b03602060018060a01b038351169201511660405191602083019384526040830152606082015260608152614a93608082610e82565b51902093614ab96060614aa96040840151615417565b9201516001600160e01b03191690565b604080516020810198895290810194909452606084019590955260808301526001600160e01b031990931660a082015291829060c0820190565b614afb613a17565b60208151910120906020815191614b11836124e1565b0151602081519101206040519160208301938452614b2e816124e1565b6040830152606082015260608152613d31608082610e82565b614b4f613a5c565b604051614b6481613566602082018095612b3b565b51902090613d318151613566602084015193614b8a60408201516001600160401b031690565b90614b9c606082015163ffffffff1690565b608082015163ffffffff169060c0614bbb60a085015163ffffffff1690565b93015193604051988997602089019b8c9463ffffffff94906001600160401b0386949260e099949c9b9a9686946101008b019e8b5260208b015260408a01521660608801521660808601521660a08401521660c08201520152565b91614c28606084015163ffffffff1690565b60a084019063ffffffff614c43613e26845163ffffffff1690565b9116116139be5783516020850151106139be5763ffffffff614c7d613e26614c72608088015163ffffffff1690565b935163ffffffff1690565b9116116139be57614c96614c90846154df565b93615502565b9162ffffff6001600160401b03614cad86866145e8565b16116139be57436001600160401b03841610614cc65750565b63873fd26b60e01b5f526004526001600160401b03821660245260445ffd5b90813b15614d66575f805160206158d683398151915280546001600160a01b0319166001600160a01b0384169081179091557fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b5f80a2805115614d4e57614d4b91615525565b50565b505034614d5757565b63b398979f60e01b5f5260045ffd5b50634c9c8ce360e01b5f9081526001600160a01b0391909116600452602490fd5b60205f600c6040516b1c9a5cd8cc0b93dd5d1c1d5d60a21b815260025afa1561057a575f8051825160209384015160408051808701949094528301919091526060820152600160f91b6080820152606281526141df90613574608282610e82565b614df0615542565b614df8615599565b6040519060208201927f8b73c3c69bb8fe3d512ecc4cf759cc79239f7b179b0ffacaa9a75d522b39400f8452604083015260608201524660808201523060a082015260a08152613d3160c082610e82565b93959492606096614f4d57614e5d906155cb565b614e6a611d698251151590565b614f2057602001516001600160601b031693614e8f614e8883612204565b93846156e5565b5f805160206158f68339815191525f80a281546001600160601b0316906001600160601b0385166001600160601b03831610614ee957508392610c9761070c9361070c610eb297610d7195906001600160601b0391031690565b60405163112fed8b60e31b60208201526001600160a01b039091166024820152949550610f3e935084925050604482019050613566565b5050604051635502416760e01b60208201526024810193909352509192509050610f3e8160448101613566565b5050604051631cfdeebb60e01b60208201526024810193909352509192509050610f3e8160448101613566565b906001600160601b03809116911603906001600160601b03821161269d57565b92969593909496606097614fad85615607565b6150eb5790614fc39291156150c1575b506155cb565b614fd0611d698251151590565b614f2057602001516001600160601b031693614ff9614ff3608061307485612204565b86614f7a565b9161500b84546001600160601b031690565b6001600160601b03808516911610615088575092610c9761070c9361070c610d719461507885615046610eb29b9a5f525f60205260405f2090565b80546001600160a01b0319166001600160a01b0390921691909117815580546001600160f81b0316600160f91b179055565b82546001600160601b0316610493565b60405163112fed8b60e31b60208201526001600160a01b039091166024820152959650610f3e9450859350506044830191506135669050565b6150d3906150ce85612204565b6156e5565b855f805160206158f68339815191525f80a25f614fbd565b5050604051631cfdeebb60e01b6020820152602481019590955250939450919250610f3e915082905060448101613566565b9396959492909260609761513086615607565b6152565760c08601516001600160c01b0319908116911680820361522f5750501561520a575b505081516001600160a01b038481169116036151de5761451e610d6b60a0610eb295946151ad6151906104a0965f525f60205260405f2090565b80546001600160f81b0316600160f81b1781555f60019190910155565b6151d06151c460808301516001600160601b031690565b61070c610d7189612204565b01516001600160601b031690565b60405163a905765160e01b60208201526024810191909152929350610f3e915082905060448101613566565b6150ce61521692612204565b805f805160206158f68339815191525f80a25f80615156565b6321c9e4d560e11b5f5260048690526001600160c01b03199081166024521660445260645ffd5b5050604051631cfdeebb60e01b60208201526024810193909352509394509250610f3e915082905060448101613566565b602081519101fd5b62ffffff81116152a15762ffffff1690565b6306dfcc6560e41b5f52601860045260245260445ffd5b818110156152cc575f5260205260405f2090565b905f5260205260405f2090565b60ff5f805160206159168339815191525460401c16156152f557565b631afcd79f60e31b5f5260045ffd5b9081516001600160401b03811161069557615343816153305f805160206158968339815191525461227a565b5f8051602061589683398151915261235e565b602092601f821160011461538357615372929382915f926138705750508160011b915f199060031b1c19161790565b5f8051602061589683398151915255565b5f805160206158968339815191525f52601f198216937f5f9ce34815f8e11431c7bb75a8e6886a91478f7ffc1dbb0a98dc240fddd76b75915f5b8681106153ff57508360019596106153e7575b505050811b015f8051602061589683398151915255565b01515f1960f88460031b161c191690555f80806153d0565b919260206001819286850151815501940192016153bd565b614afb613b16565b815191906041830361544f576154489250602082015190606060408401519301515f1a9061575f565b9192909190565b50505f9160029190565b600411156124eb57565b61546c81615459565b80615475575050565b61547e81615459565b600181036154955763f645eedf60e01b5f5260045ffd5b61549e81615459565b600281036154b9575063fce698f760e01b5f5260045260245ffd5b806154c5600392615459565b146154cd5750565b6335e2f38360e21b5f5260045260245ffd5b610f3e9063ffffffff60806001600160401b036040840151169201511690613da3565b610f3e9063ffffffff60a06001600160401b036040840151169201511690613da3565b5f80610f3e93602081519101845af461553c61248a565b916157d7565b61554a613188565b805190811561555a576020012090565b50505f805160206158b68339815191525480156155745790565b507fc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a47090565b6155a1613255565b80519081156155b1576020012090565b50505f805160206159368339815191525480156155745790565b6155d3614017565b505c6155dd614017565b506001600160601b03604051916155f383610e4c565b6001607f1b81161515835216602082015290565b6060810151600116151590811561561c575090565b606001516002161515905090565b9063ffffffff16601c81101561568f579061566b61565961564d610eb2946126a2565b66ffffffffffffff1690565b600166ffffffffffffff9182161b1690565b815460c81c82546001600160c81b0316911760c81b6001600160c81b031916179055565b601c810390811161269d576156c2610eb29260016156b860ff6156b1866126a2565b16946126a2565b60081c9101613f1f565b81545f1960039290921b91821b198116600190941b90821c17901b919091179055565b9063ffffffff16601c81101561571a579061566b61570861564d610eb2946126a2565b600266ffffffffffffff9182161b1690565b601c810390811161269d5761573c610eb29260016156b860ff6156b1866126a2565b81545f1960039290921b91821b198116600290941b90821c17901b919091179055565b91906fa2a8918ca85bafe22016d0b997e4df60600160ff1b0384116157cc579160209360809260ff5f9560405194855216868401526040830152606082015282805260015afa1561057a575f516001600160a01b038116156157c257905f905f90565b505f906001905f90565b5050505f9160039190565b906157fb57508051156157ec57805190602001fd5b630a12f52160e11b5f5260045ffd5b8151158061582c575b61580c575090565b639996b31560e01b5f9081526001600160a01b0391909116600452602490fd5b50803b1561580456fe405787fa12a823e0f2b7631cc41b3ba8828b3321ca811111fa75cd3aa3bb5acea16a46d94261c7517cc8ff89f61c0ce93598e3c849801011dee649a6a557d1029016d09d72d40fdae2fd8ceac6b6234c7706214fd39c1cd1e609a0528c199300a16a46d94261c7517cc8ff89f61c0ce93598e3c849801011dee649a6a557d103a16a46d94261c7517cc8ff89f61c0ce93598e3c849801011dee649a6a557d100360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc1cedb001a5114ea90393cd9f134224e9e1312545ee8a9b9533d780be6a9bf8b7f0c57e16840df040f15088dc2f81fe391c3923bec73e23a9662efc9c229c6a00a16a46d94261c7517cc8ff89f61c0ce93598e3c849801011dee649a6a557d101237e158222e3e6968b72b9db0d8043aacf074ad9f650f0d1606b4d82ee432c00a164736f6c634300081a000a")]
        contract BoundlessMarket {
            constructor(address verifier, bytes32 assessorId, address stakeTokenContract) {}
            function initialize(address initialOwner, string calldata imageUrl) {}
        }
    }

    alloy::sol! {
        #[sol(rpc, bytecode = "60806040526102748038038061001481610168565b92833981016040828203126101645781516001600160a01b03811692909190838303610164576020810151906001600160401b03821161016457019281601f8501121561016457835161006e610069826101a1565b610168565b9481865260208601936020838301011161016457815f926020809301865e86010152823b15610152577f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc80546001600160a01b031916821790557fbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b5f80a282511561013a575f8091610122945190845af43d15610132573d91610113610069846101a1565b9283523d5f602085013e6101bc565b505b6040516059908161021b8239f35b6060916101bc565b50505034156101245763b398979f60e01b5f5260045ffd5b634c9c8ce360e01b5f5260045260245ffd5b5f80fd5b6040519190601f01601f191682016001600160401b0381118382101761018d57604052565b634e487b7160e01b5f52604160045260245ffd5b6001600160401b03811161018d57601f01601f191660200190565b906101e057508051156101d157805190602001fd5b630a12f52160e11b5f5260045ffd5b81511580610211575b6101f1575090565b639996b31560e01b5f9081526001600160a01b0391909116600452602490fd5b50803b156101e956fe60806040527f360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc545f9081906001600160a01b0316368280378136915af43d5f803e156048573d5ff35b3d5ffdfea164736f6c634300081a000a")]
        contract ERC1967Proxy {
            constructor(address implementation, bytes memory data) payable {}
        }
    }

    alloy::sol! {
        #[sol(rpc, bytecode = "61016080604052346104cb5760208161223c803803809161002082856104cf565b8339810103126104cb57516001600160a01b038116908181036104cb5760405161004b6040826104cf565b600981526020810168486974506f696e747360b81b8152604051906100716040836104cf565b6009825268486974506f696e747360b81b6020830152604051926100966040856104cf565b6002845261048560f41b6020850152604051936100b46040866104cf565b60018552603160f81b60208601908152845190946001600160401b0382116103ce5760035490600182811c921680156104c1575b60208310146103b05781601f849311610453575b50602090601f83116001146103ed575f926103e2575b50508160011b915f199060031b1c1916176003555b8051906001600160401b0382116103ce5760045490600182811c921680156103c4575b60208310146103b05781601f849311610342575b50602090601f83116001146102dc575f926102d1575b50508160011b915f199060031b1c1916176004555b61019281610623565b6101205261019f846107aa565b61014052519020918260e05251902080610100524660a0526040519060208201927f8b73c3c69bb8fe3d512ecc4cf759cc79239f7b179b0ffacaa9a75d522b39400f8452604083015260608201524660808201523060a082015260a0815261020860c0826104cf565b5190206080523060c05281156102be57600980546001600160a01b03198116841790915561026392906001600160a01b03167f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e05f80a36104f2565b5061026c610568565b506040516118f990816108e382396080518161127a015260a05181611337015260c05181611244015260e051816112c9015261010051816112ef0152610120518161060c015261014051816106350152f35b631e4fbdf760e01b5f525f60045260245ffd5b015190505f80610174565b60045f9081528281209350601f198516905b81811061032a5750908460019594939210610312575b505050811b01600455610189565b01515f1960f88460031b161c191690555f8080610304565b929360206001819287860151815501950193016102ee565b60045f529091507f8a35acfbc15ff81a39ae7d344fd709f28e8600b4aa8c65c6b64bfe7fe36bd19b601f840160051c810191602085106103a6575b90601f859493920160051c01905b818110610398575061015e565b5f815584935060010161038b565b909150819061037d565b634e487b7160e01b5f52602260045260245ffd5b91607f169161014a565b634e487b7160e01b5f52604160045260245ffd5b015190505f80610112565b60035f9081528281209350601f198516905b81811061043b5750908460019594939210610423575b505050811b01600355610127565b01515f1960f88460031b161c191690555f8080610415565b929360206001819287860151815501950193016103ff565b60035f529091507fc2575a0e9e593c00f959f8c92f12db2869c3395a3b0502d05e2516446f71f85b601f840160051c810191602085106104b7575b90601f859493920160051c01905b8181106104a957506100fc565b5f815584935060010161049c565b909150819061048e565b91607f16916100e8565b5f80fd5b601f909101601f19168101906001600160401b038211908210176103ce57604052565b6001600160a01b0381165f9081525f8051602061221c833981519152602052604090205460ff16610563576001600160a01b03165f8181525f8051602061221c83398151915260205260408120805460ff191660011790553391905f805160206121dc8339815191528180a4600190565b505f90565b5f80525f805160206121fc8339815191526020527f01be3e6f97001cf116ea8f5cec42d044a10e897cbd383685b72e0237d0cef4bf5460ff1661061f575f8080525f805160206121fc8339815191526020527f01be3e6f97001cf116ea8f5cec42d044a10e897cbd383685b72e0237d0cef4bf805460ff1916600117905533907f1c64e5e5a710fb0608449c7cd933acf9cf88b08f7bfa074e5c97e006d88247125f805160206121dc8339815191528280a4600190565b5f90565b908151602081105f1461069d575090601f81511161065d57602081519101516020821061064e571790565b5f198260200360031b1b161790565b604460209160405192839163305a27a960e01b83528160048401528051918291826024860152018484015e5f828201840152601f01601f19168101030190fd5b6001600160401b0381116103ce57600554600181811c911680156107a0575b60208210146103b057601f811161076d575b50602092601f821160011461070c57928192935f92610701575b50508160011b915f199060031b1c19161760055560ff90565b015190505f806106e8565b601f1982169360055f52805f20915f5b868110610755575083600195961061073d575b505050811b0160055560ff90565b01515f1960f88460031b161c191690555f808061072f565b9192602060018192868501518155019401920161071c565b60055f52601f60205f20910160051c810190601f830160051c015b81811061079557506106ce565b5f8155600101610788565b90607f16906106bc565b908151602081105f146107d5575090601f81511161065d57602081519101516020821061064e571790565b6001600160401b0381116103ce57600654600181811c911680156108d8575b60208210146103b057601f81116108a5575b50602092601f821160011461084457928192935f92610839575b50508160011b915f199060031b1c19161760065560ff90565b015190505f80610820565b601f1982169360065f52805f20915f5b86811061088d5750836001959610610875575b505050811b0160065560ff90565b01515f1960f88460031b161c191690555f8080610867565b91926020600181928685015181550194019201610854565b60065f52601f60205f20910160051c810190601f830160051c015b8181106108cd5750610806565b5f81556001016108c0565b90607f16906107f456fe6080806040526004361015610012575f80fd5b5f3560e01c90816301ffc9a714610c8a5750806306fdde0314610be5578063095ea7b314610bbf57806318160ddd14610ba257806323b872dd14610b6a578063248a9ca314610b3f5780632738cf0814610b165780632f2ff15d14610ad8578063313ce56714610abd5780633644e51514610a9b57806336568abe14610a575780633dd1eb6114610a2e57806340c10f191461085357806342966c681461083657806369e2f0fb1461080d57806370a08231146107d6578063715018a61461077b578063732076d31461075457806379cc6790146107245780637ecebe00146106ec57806384b0196e146105f45780638da5cb5b146105cc57806391d148541461058357806395d89b41146104a1578063a217fddf14610487578063a9059cbb14610456578063b8f3c3071461042d578063d505accf146102ea578063d547741f146102a5578063dd62ed3e14610255578063f2fde38b146101a75763fe6d81241461017c575f80fd5b346101a3575f3660031901126101a35760206040515f8051602061182d8339815191528152f35b5f80fd5b346101a35760203660031901126101a3576101c0610d01565b6101c8610ff1565b6009546101dd906001600160a01b0316611459565b506101e78161111d565b506101f0610ff1565b6001600160a01b0316801561024257600980546001600160a01b0319811683179091556001600160a01b03167f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e05f80a3005b631e4fbdf760e01b5f525f60045260245ffd5b346101a35760403660031901126101a35761026e610d01565b610276610d17565b6001600160a01b039182165f908152600160209081526040808320949093168252928352819020549051908152f35b346101a35760403660031901126101a3576102e86004356102c4610d17565b906102e36102de825f526008602052600160405f20015490565b611207565b6114c8565b005b346101a35760e03660031901126101a357610303610d01565b61030b610d17565b604435906064359260843560ff811681036101a35784421161041a576103df6103e89160018060a01b03841696875f52600760205260405f20908154916001830190556040519060208201927f6e71edae12b1b97f4d1f60370fef10105fa2faae0126114a169c64845d6126c984528a604084015260018060a01b038916606084015289608084015260a083015260c082015260c081526103ad60e082610de6565b5190206103b8611241565b906040519161190160f01b83526002830152602282015260c43591604260a43592206116f5565b90929192611778565b6001600160a01b031684810361040357506102e893506115f8565b84906325c0072360e11b5f5260045260245260445ffd5b8463313c898160e11b5f5260045260245ffd5b346101a35760203660031901126101a3576102e8610449610d01565b610451610ff1565b6113db565b346101a35760403660031901126101a35761047c610472610d01565b6024359033610eef565b602060405160018152f35b346101a3575f3660031901126101a35760206040515f8152f35b346101a3575f3660031901126101a3576040515f6004546104c181610d2d565b808452906001811690811561055f5750600114610501575b6104fd836104e981850382610de6565b604051918291602083526020830190610cdd565b0390f35b60045f9081527f8a35acfbc15ff81a39ae7d344fd709f28e8600b4aa8c65c6b64bfe7fe36bd19b939250905b808210610545575090915081016020016104e96104d9565b91926001816020925483858801015201910190929161052d565b60ff191660208086019190915291151560051b840190910191506104e990506104d9565b346101a35760403660031901126101a35761059c610d17565b6004355f52600860205260405f209060018060a01b03165f52602052602060ff60405f2054166040519015158152f35b346101a3575f3660031901126101a3576009546040516001600160a01b039091168152602090f35b346101a3575f3660031901126101a3576106906106307f000000000000000000000000000000000000000000000000000000000000000061165b565b6106597f00000000000000000000000000000000000000000000000000000000000000006116be565b602061069e6040519261066c8385610de6565b5f84525f368137604051958695600f60f81b875260e08588015260e0870190610cdd565b908582036040870152610cdd565b4660608501523060808501525f60a085015283810360c08501528180845192838152019301915f5b8281106106d557505050500390f35b8351855286955093810193928101926001016106c6565b346101a35760203660031901126101a3576001600160a01b0361070d610d01565b165f526007602052602060405f2054604051908152f35b346101a35760403660031901126101a3576102e8610740610d01565b6024359061074f823383610e29565b611539565b346101a3575f3660031901126101a35760206040515f805160206118cd8339815191528152f35b346101a3575f3660031901126101a357610793610ff1565b600980546001600160a01b031981169091555f906001600160a01b03167f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e08280a3005b346101a35760203660031901126101a3576001600160a01b036107f7610d01565b165f525f602052602060405f2054604051908152f35b346101a35760203660031901126101a3576102e8610829610d01565b610831610ff1565b61135d565b346101a35760203660031901126101a3576102e860043533611539565b346101a35760403660031901126101a35761086c610d01565b335f9081525f8051602061180d8339815191526020526040902054602435919060ff1615610a0a5760018060a01b0316805f525f6020526001600160601b036108b98360405f2054610e1c565b116109e55780156109d2575f80525f8051602061184d8339815191526020527f01be3e6f97001cf116ea8f5cec42d044a10e897cbd383685b72e0237d0cef4bf5460ff1615806109ae575b61099f5761091482600254610e1c565b600255805f525f60205260405f20828154019055805f5f8051602061188d8339815191526020604051868152a3805f525f6020526001600160601b0360405f20541161095c57005b805f525f60205260405f20549082820391821161098b5763538fd55b60e11b5f5260045260245260445260645ffd5b634e487b7160e01b5f52601160045260245ffd5b6325cdf54f60e21b5f5260045ffd5b505f8181525f8051602061184d833981519152602052604090205460ff1615610904565b63ec442f0560e01b5f525f60045260245ffd5b805f525f60205260405f20549063538fd55b60e11b5f5260045260245260445260645ffd5b63e2517d3f60e01b5f52336004525f8051602061182d83398151915260245260445ffd5b346101a35760203660031901126101a3576102e8610a4a610d01565b610a52610ff1565b61109d565b346101a35760403660031901126101a357610a70610d17565b336001600160a01b03821603610a8c576102e8906004356114c8565b63334bd91960e11b5f5260045ffd5b346101a3575f3660031901126101a3576020610ab5611241565b604051908152f35b346101a3575f3660031901126101a357602060405160128152f35b346101a35760403660031901126101a3576102e8600435610af7610d17565b90610b116102de825f526008602052600160405f20015490565b61118e565b346101a35760203660031901126101a3576102e8610b32610d01565b610b3a610ff1565b611018565b346101a35760203660031901126101a3576020610ab56004355f526008602052600160405f20015490565b346101a35760603660031901126101a35761047c610b86610d01565b610b8e610d17565b60443591610b9d833383610e29565b610eef565b346101a3575f3660031901126101a3576020600254604051908152f35b346101a35760403660031901126101a35761047c610bdb610d01565b60243590336115f8565b346101a3575f3660031901126101a3576040515f600354610c0581610d2d565b808452906001811690811561055f5750600114610c2c576104fd836104e981850382610de6565b60035f9081527fc2575a0e9e593c00f959f8c92f12db2869c3395a3b0502d05e2516446f71f85b939250905b808210610c70575090915081016020016104e96104d9565b919260018160209254838588010152019101909291610c58565b346101a35760203660031901126101a3576004359063ffffffff60e01b82168092036101a357602091637965db0b60e01b8114908115610ccc575b5015158152f35b6301ffc9a760e01b14905083610cc5565b805180835260209291819084018484015e5f828201840152601f01601f1916010190565b600435906001600160a01b03821682036101a357565b602435906001600160a01b03821682036101a357565b90600182811c92168015610d5b575b6020831014610d4757565b634e487b7160e01b5f52602260045260245ffd5b91607f1691610d3c565b5f9291815491610d7483610d2d565b8083529260018116908115610dc95750600114610d9057505050565b5f9081526020812093945091925b838310610daf575060209250010190565b600181602092949394548385870101520191019190610d9e565b915050602093945060ff929192191683830152151560051b010190565b90601f8019910116810190811067ffffffffffffffff821117610e0857604052565b634e487b7160e01b5f52604160045260245ffd5b9190820180921161098b57565b6001600160a01b039081165f8181526001602081815260408084209587168452949052929020549392918401610e60575b50505050565b828410610ecc578015610eb9576001600160a01b03821615610ea6575f52600160205260405f209060018060a01b03165f5260205260405f20910390555f808080610e5a565b634a1406b160e11b5f525f60045260245ffd5b63e602df0560e01b5f525f60045260245ffd5b508290637dc7a0d960e11b5f5260018060a01b031660045260245260445260645ffd5b6001600160a01b0316908115610fde576001600160a01b03169081156109d2575f8181525f8051602061184d833981519152602052604090205460ff161580610fba575b61099f57805f525f60205260405f2054838110610fa05790838392825f525f6020520360405f2055815f525f60205260405f208481540190555f8051602061188d8339815191526020604051868152a3805f525f6020526001600160601b0360405f20541161095c575050565b915063391434e360e21b5f5260045260245260445260645ffd5b505f8281525f8051602061184d833981519152602052604090205460ff1615610f33565b634b637e8f60e11b5f525f60045260245ffd5b6009546001600160a01b0316330361100557565b63118cdaa760e01b5f523360045260245ffd5b6001600160a01b0381165f9081525f8051602061184d833981519152602052604090205460ff16611098576001600160a01b03165f8181525f8051602061184d83398151915260205260408120805460ff191660011790553391905f805160206118cd833981519152905f805160206117ed8339815191529080a4600190565b505f90565b6001600160a01b0381165f9081525f8051602061180d833981519152602052604090205460ff16611098576001600160a01b03165f8181525f8051602061180d83398151915260205260408120805460ff191660011790553391905f8051602061182d833981519152905f805160206117ed8339815191529080a4600190565b6001600160a01b0381165f9081525f8051602061186d833981519152602052604090205460ff16611098576001600160a01b03165f8181525f8051602061186d83398151915260205260408120805460ff191660011790553391905f805160206117ed8339815191528180a4600190565b5f8181526008602090815260408083206001600160a01b038616845290915290205460ff16611201575f8181526008602090815260408083206001600160a01b0395909516808452949091528120805460ff19166001179055339291905f805160206117ed8339815191529080a4600190565b50505f90565b5f81815260086020908152604080832033845290915290205460ff161561122b5750565b63e2517d3f60e01b5f523360045260245260445ffd5b307f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03161480611334575b1561129c577f000000000000000000000000000000000000000000000000000000000000000090565b60405160208101907f8b73c3c69bb8fe3d512ecc4cf759cc79239f7b179b0ffacaa9a75d522b39400f82527f000000000000000000000000000000000000000000000000000000000000000060408201527f000000000000000000000000000000000000000000000000000000000000000060608201524660808201523060a082015260a0815261132e60c082610de6565b51902090565b507f00000000000000000000000000000000000000000000000000000000000000004614611273565b6001600160a01b0381165f9081525f8051602061180d833981519152602052604090205460ff1615611098576001600160a01b03165f8181525f8051602061180d83398151915260205260408120805460ff191690553391905f8051602061182d833981519152905f805160206118ad8339815191529080a4600190565b6001600160a01b0381165f9081525f8051602061184d833981519152602052604090205460ff1615611098576001600160a01b03165f8181525f8051602061184d83398151915260205260408120805460ff191690553391905f805160206118cd833981519152905f805160206118ad8339815191529080a4600190565b6001600160a01b0381165f9081525f8051602061186d833981519152602052604090205460ff1615611098576001600160a01b03165f8181525f8051602061186d83398151915260205260408120805460ff191690553391905f805160206118ad8339815191528180a4600190565b5f8181526008602090815260408083206001600160a01b038616845290915290205460ff1615611201575f8181526008602090815260408083206001600160a01b0395909516808452949091528120805460ff19169055339291905f805160206118ad8339815191529080a4600190565b9091906001600160a01b03168015610fde575f8181525f8051602061184d833981519152602052604090205460ff1615806115b8575b61099f57805f525f60205260405f2054838110610fa0576020845f94955f8051602061188d833981519152938587528684520360408620558060025403600255604051908152a3565b505f80525f8051602061184d8339815191526020527f01be3e6f97001cf116ea8f5cec42d044a10e897cbd383685b72e0237d0cef4bf5460ff161561156f565b6001600160a01b0316908115610eb9576001600160a01b0316918215610ea65760207f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b92591835f526001825260405f20855f5282528060405f2055604051908152a3565b60ff81146116a15760ff811690601f8211611692576040519161167f604084610de6565b6020808452838101919036833783525290565b632cd44ac360e21b5f5260045ffd5b506040516116bb816116b4816005610d65565b0382610de6565b90565b60ff81146116e25760ff811690601f8211611692576040519161167f604084610de6565b506040516116bb816116b4816006610d65565b91906fa2a8918ca85bafe22016d0b997e4df60600160ff1b03841161176d579160209360809260ff5f9560405194855216868401526040830152606082015282805260015afa15611762575f516001600160a01b0381161561175857905f905f90565b505f906001905f90565b6040513d5f823e3d90fd5b5050505f9160039190565b60048110156117d8578061178a575050565b600181036117a15763f645eedf60e01b5f5260045ffd5b600281036117bc575063fce698f760e01b5f5260045260245ffd5b6003146117c65750565b6335e2f38360e21b5f5260045260245ffd5b634e487b7160e01b5f52602160045260245ffdfe2f8788117e7eff1d82e926ec794901d17c78024a50270940304540a733656f0dba682078eba37bca8662ade60eacd8e3fb6b879f4ad882618e7fd467572c020af0887ba65ee2024ea881d91b74c2450ef19e1557f03bed3ea9f16b037cbe2dc99f27e1ced449d0391672dfd8b33c5ac9642366ee4163605233bae574eb26e48c5eff886ea0ce6ca488a3d6e336d6c0f75f46d19b42c06ce5ee98e42c96d256c7ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3eff6391f5c32d9c69d2a47ea670b442974b53935d1edc7fd64eb21e047a839171b1c64e5e5a710fb0608449c7cd933acf9cf88b08f7bfa074e5c97e006d8824712a164736f6c634300081a000a2f8788117e7eff1d82e926ec794901d17c78024a50270940304540a733656f0d9f27e1ced449d0391672dfd8b33c5ac9642366ee4163605233bae574eb26e48c5eff886ea0ce6ca488a3d6e336d6c0f75f46d19b42c06ce5ee98e42c96d256c7")]
        contract HitPoints {
            constructor(address initialOwner) payable {}
        }
    }

    pub struct TestCtx<P> {
        pub verifier_addr: Address,
        pub set_verifier_addr: Address,
        pub hit_points_addr: Address,
        pub boundless_market_addr: Address,
        pub prover_signer: PrivateKeySigner,
        pub customer_signer: PrivateKeySigner,
        pub prover_provider: P,
        pub prover_market: BoundlessMarketService<P>,
        pub customer_provider: P,
        pub customer_market: BoundlessMarketService<P>,
        pub set_verifier: SetVerifierService<P>,
        pub hit_points_service: HitPointsService<P>,
    }

    pub async fn deploy_mock_verifier<P: Provider>(deployer_provider: P) -> Result<Address> {
        let instance = MockVerifier::deploy(deployer_provider, FixedBytes::ZERO)
            .await
            .context("failed to deploy RiscZeroMockVerifier")?;
        Ok(*instance.address())
    }

    pub async fn deploy_set_verifier<P: Provider>(
        deployer_provider: P,
        verifier_address: Address,
        set_builder_id: Digest,
    ) -> Result<Address> {
        let instance = SetVerifier::deploy(
            deployer_provider,
            verifier_address,
            <[u8; 32]>::from(set_builder_id).into(),
            String::default(),
        )
        .await
        .context("failed to deploy RiscZeroSetVerifier")?;
        Ok(*instance.address())
    }

    pub async fn deploy_hit_points<P: Provider>(
        deployer_signer: &PrivateKeySigner,
        deployer_provider: P,
    ) -> Result<Address> {
        let deployer_address = deployer_signer.address();
        let instance = HitPoints::deploy(deployer_provider, deployer_address)
            .await
            .context("failed to deploy HitPoints contract")?;
        Ok(*instance.address())
    }

    pub async fn deploy_boundless_market<P: Provider>(
        deployer_signer: &PrivateKeySigner,
        deployer_provider: P,
        set_verifier: Address,
        hit_points: Address,
        assessor_guest_id: Digest,
        allowed_prover: Option<Address>,
    ) -> Result<Address> {
        let deployer_address = deployer_signer.address();
        let market_instance = BoundlessMarket::deploy(
            &deployer_provider,
            set_verifier,
            <[u8; 32]>::from(assessor_guest_id).into(),
            hit_points,
        )
        .await
        .context("failed to deploy BoundlessMarket implementation")?;

        let proxy_instance = ERC1967Proxy::deploy(
            &deployer_provider,
            *market_instance.address(),
            BoundlessMarket::initializeCall {
                initialOwner: deployer_address,
                imageUrl: "".to_string(),
            }
            .abi_encode()
            .into(),
        )
        .await
        .context("failed to deploy BoundlessMarket proxy")?;
        let proxy = *proxy_instance.address();

        if hit_points != Address::ZERO {
            let hit_points_service =
                HitPointsService::new(hit_points, &deployer_provider, deployer_signer.address());
            hit_points_service.grant_minter_role(hit_points_service.caller()).await?;
            hit_points_service.grant_authorized_transfer_role(proxy).await?;
            if let Some(prover) = allowed_prover {
                hit_points_service.mint(prover, default_allowance()).await?;
            }
        }

        Ok(proxy)
    }

    async fn deploy_contracts(
        anvil: &AnvilInstance,
        set_builder_id: Digest,
        assessor_guest_id: Digest,
    ) -> Result<(Address, Address, Address, Address)> {
        let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let deployer_provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(deployer_signer.clone()))
            .on_builtin(&anvil.endpoint())
            .await?;

        // Deploy contracts
        let verifier = deploy_mock_verifier(&deployer_provider).await?;
        let set_verifier =
            deploy_set_verifier(&deployer_provider, verifier, set_builder_id).await?;
        let hit_points = deploy_hit_points(&deployer_signer, &deployer_provider).await?;
        let boundless_market = deploy_boundless_market(
            &deployer_signer,
            &deployer_provider,
            set_verifier,
            hit_points,
            assessor_guest_id,
            None,
        )
        .await?;

        // Mine forward some blocks using the provider
        deployer_provider.anvil_mine(Some(10), Some(2)).await.unwrap();
        deployer_provider.anvil_set_interval_mining(2).await.unwrap();

        Ok((verifier, set_verifier, hit_points, boundless_market))
    }

    pub struct DefaultTestCtx;

    impl DefaultTestCtx {
        pub async fn create(
            anvil: &AnvilInstance,
            set_builder_id: impl Into<Digest>,
            assessor_guest_id: impl Into<Digest>,
        ) -> Result<TestCtx<impl Provider + WalletProvider + Clone + 'static>> {
            Self::create_with_rpc_url(anvil, &anvil.endpoint(), set_builder_id, assessor_guest_id)
                .await
        }

        pub async fn create_with_rpc_url(
            anvil: &AnvilInstance,
            rpc_url: &str,
            set_builder_id: impl Into<Digest>,
            assessor_guest_id: impl Into<Digest>,
        ) -> Result<TestCtx<impl Provider + WalletProvider + Clone + 'static>> {
            let (verifier_addr, set_verifier_addr, hit_points_addr, boundless_market_addr) =
                deploy_contracts(anvil, set_builder_id.into(), assessor_guest_id.into())
                    .await
                    .unwrap();

            let prover_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
            let customer_signer: PrivateKeySigner = anvil.keys()[2].clone().into();
            let verifier_signer: PrivateKeySigner = anvil.keys()[0].clone().into();

            let prover_provider = ProviderBuilder::new()
                .wallet(EthereumWallet::from(prover_signer.clone()))
                .on_builtin(rpc_url)
                .await?;
            let customer_provider = ProviderBuilder::new()
                .wallet(EthereumWallet::from(customer_signer.clone()))
                .on_builtin(rpc_url)
                .await?;
            let verifier_provider = ProviderBuilder::new()
                .wallet(EthereumWallet::from(verifier_signer.clone()))
                .on_builtin(rpc_url)
                .await?;

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
            requirements: Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            imageUrl: "https://dev.null".to_string(),
            input: Input::builder().build_inline().unwrap(),
            offer: Offer {
                minPrice: U256::from(0),
                maxPrice: U256::from(1),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockTimeout: 1000,
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
