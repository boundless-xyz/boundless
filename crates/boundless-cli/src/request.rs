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

//! The Boundless CLI is a command-line interface for interacting with Boundless.

use std::{fmt, ops::Not};

use alloy_primitives::{
    aliases::{U160, U32, U96},
    Address, Bytes, FixedBytes, Uint, B256, U256,
};
use boundless_core::{
    input::GuestEnvBuilder,
    util::{is_dev_mode, now_timestamp},
};
use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};
use serde_with::DisplayFromStr;
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::selector::Selector;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Represents a proof request that can be sent to the Boundless Market.
pub struct ProofRequest {
    /// Identifier for the request.
    pub id: RequestId,
    /// Requirements for the request, including image ID, predicate, and selector.
    pub requirements: Requirements,
    /// URL of the image to be processed.
    ///
    /// This URL should point to a location where the image can be fetched.
    pub image_url: String,
    /// Input data for the request, which can be inline or a URL.
    pub input: RequestInput,
    /// Offer details for the request.
    pub offer: Offer,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Represents the input data for a proof request.
pub struct RequestInput {
    /// Type of the input data, which can be inline or a URL.
    pub input_type: RequestInputType,
    /// The actual data for the input, which can be bytes or a URL.
    pub data: Bytes,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
/// Represents the type of input data for a proof request.
pub enum RequestInputType {
    /// Inline input data, which is directly included in the request.
    Inline,
    /// URL input data, which points to a location where the data can be fetched.
    Url,
    /// Invalid input type, used for deserialization errors.
    __Invalid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Represents an offer for a proof request.
pub struct Offer {
    /// Minimum price for the offer.
    pub min_price: Uint<256, 4>,
    /// Maximum price for the offer.
    pub max_price: Uint<256, 4>,
    /// Start time for bidding.
    pub bidding_start: u64,
    /// Duration for ramping up the price.
    pub ramp_up_period: u32,
    /// Timeout for the lock.
    pub lock_timeout: u32,
    /// Overall timeout for the offer.
    pub timeout: u32,
    /// Stake required to lock the offer.
    pub lock_stake: Uint<256, 4>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Represents the requirements for a proof request.
pub struct Requirements {
    /// Image ID identifying the program to be executed.
    pub image_id: FixedBytes<32>,
    /// Callback to be executed when the proof is fulfilled.
    pub callback: Callback,
    /// Predicate specifying what conditions the proof must satisfy.
    pub predicate: Predicate,
    /// Selector specifying the type of proof required.
    pub selector: FixedBytes<4>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Represents a predicate for a proof request.
pub struct Predicate {
    /// Type of the predicate, which can be a digest match or a prefix match.
    pub predicate_type: PredicateType,
    /// Data associated with the predicate, which can be a digest or a prefix.
    pub data: Bytes,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
/// Represents the type of predicate for a proof request.
pub enum PredicateType {
    /// Digest match predicate.
    DigestMatch,
    /// Prefix match predicate.
    PrefixMatch,
    /// Invalid predicate type, used for deserialization errors.
    __Invalid,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Represents a callback to be executed when the proof is fulfilled.
pub struct Callback {
    /// Address of the contract to call when the proof is fulfilled.
    pub addr: Address,
    /// Gas limit for the callback when the proof is fulfilled.
    pub gas_limit: Uint<96, 2>,
}

/// Structured represent of a request ID.
///
/// This struct can be packed and unpacked from a U256 value.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestId {
    /// Address of the wallet or contract authorizing the request.
    pub addr: Address,
    /// Index of the request, assigned by the requester.
    ///
    /// Each index can correspond to a single fulfilled request. If multiple requests have the same
    /// address and index, only one can ever be fulfilled. This is similar to how transaction
    /// nonces work on Ethereum.
    pub index: u32,
    /// A flag set to true when the signature over the request is provided by a smart contract,
    /// using ERC-1271. When set to false, the request is signed using ECDSA.
    pub smart_contract_signed: bool,
}

impl RequestId {
    /// Create a [RequestId] with the given [Address] and index. Sets flags to default values.
    pub fn new(addr: Address, index: u32) -> Self {
        Self { addr, index, smart_contract_signed: false }
    }

    /// Create a packed [RequestId] with the given [Address] and index. Sets flags to default values.
    pub fn u256(addr: Address, index: u32) -> U256 {
        Self::new(addr, index).into()
    }

    /// Set the smart contract signed flag to true. This indicates that the signature associated
    /// with the request should be validated using ERC-1271's isValidSignature function.
    pub fn set_smart_contract_signed_flag(self) -> Self {
        Self { addr: self.addr, index: self.index, smart_contract_signed: true }
    }

    /// Unpack a [RequestId] from a [U256] ignoring bits that do not correspond to known fields.
    ///
    /// Note that this is a lossy conversion in that converting the resulting [RequestId] back into
    /// a [U256] is not guaranteed to give the original value. If flags are added in future
    /// versions of the Boundless Market, this function will ignore them.
    pub fn from_lossy(value: U256) -> Self {
        let mut addr_u256 = value >> U256::from(32);
        addr_u256 &= (U256::from(1) << U256::from(160)) - U256::from(1); // mask out the flags
        let addr = Address::from(addr_u256.to::<U160>());
        Self {
            addr,
            index: (value & U32::MAX.to::<U256>()).to::<u32>(),
            smart_contract_signed: (value & (U256::from(1) << 192)) != U256::ZERO,
        }
    }
}

impl fmt::LowerHex for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let u256: U256 = self.clone().into();
        write!(f, "{u256:#x}")
    }
}

impl TryFrom<U256> for RequestId {
    type Error = RequestError;

    fn try_from(value: U256) -> Result<Self, Self::Error> {
        // Check if any bits above the smart contract signed flag are set.
        // An error here could indicate that this logic has not been updated to support new flags
        if value >> 193 != U256::ZERO {
            return Err(RequestError::MalformedRequestId);
        }
        Ok(RequestId::from_lossy(value))
    }
}

impl From<RequestId> for U256 {
    fn from(value: RequestId) -> Self {
        #[allow(clippy::unnecessary_fallible_conversions)] // U160::from does not compile
        let addr = U160::try_from(value.addr).unwrap();
        let smart_contract_signed_flag =
            if value.smart_contract_signed { U256::from(1) } else { U256::ZERO };
        (smart_contract_signed_flag << 192) | (U256::from(addr) << 32) | U256::from(value.index)
    }
}

impl Serialize for RequestId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let u256: U256 = self.clone().into();
        serde_with::As::<DisplayFromStr>::serialize(&u256, serializer)
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize as U256 string
        let u256_str = String::deserialize(deserializer)?;
        let u256 = U256::from_str_radix(&u256_str, 10).map_err(serde::de::Error::custom)?;

        RequestId::try_from(u256).map_err(serde::de::Error::custom)
    }
}

#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
/// Errors that can occur when creating a proof request.
pub enum RequestError {
    /// The request ID is malformed.
    #[error("malformed request ID")]
    MalformedRequestId,

    /// The client address is all zeroes.
    #[error("request ID has client address of all zeroes")]
    ClientAddrIsZero,

    /// The client address does not match with the expected address.
    #[error("request ID has client address {0}, but expected {1}")]
    AddressMismatch(Address, Address),

    /// The signature is invalid.
    #[error("signature error: {0}")]
    SignatureError(#[from] alloy_primitives::SignatureError),

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

    /// The offer ramp up period is longer than the lock timeout
    #[error("offer ramp up period must be less than or equal to the lock timeout")]
    OfferRampUpGreaterThanLockTimeout,

    /// The offer lock timeout is greater than the timeout
    #[error("offer lock timeout must be less than or equal to the timeout")]
    OfferLockTimeoutGreaterThanTimeout,

    /// Difference between timeout and lockTimeout much be less than 2^24
    ///
    /// This is a requirement of the BoundlessMarket smart to optimize use of storage.
    #[error("difference between timeout and lockTimeout much be less than 2^24")]
    OfferTimeoutRangeTooLarge,

    /// The offer max price is zero.
    #[error("offer maxPrice must be greater than 0")]
    OfferMaxPriceIsZero,

    /// The offer max price is less than the min price.
    #[error("offer maxPrice must be greater than or equal to minPrice")]
    OfferMaxPriceIsLessThanMin,

    /// The offer bidding start is zero.
    #[error("offer biddingStart must be greater than 0")]
    OfferBiddingStartIsZero,

    /// The requirements are missing from the request.
    #[error("missing requirements")]
    MissingRequirements,

    /// The image URL is missing from the request.
    #[error("missing image URL")]
    MissingImageUrl,

    /// The input is missing from the request.
    #[error("missing input")]
    MissingInput,

    /// The offer is missing from the request.
    #[error("missing offer")]
    MissingOffer,

    /// The request ID is missing from the request.
    #[error("missing request ID")]
    MissingRequestId,

    /// Request digest mismatch.
    #[error("request digest mismatch")]
    DigestMismatch,
}

impl ProofRequest {
    /// Creates a new proof request with the given parameters.
    ///
    /// The request ID is generated by combining the address and given idx.
    pub fn new(
        request_id: RequestId,
        requirements: impl Into<Requirements>,
        image_url: impl Into<String>,
        input: impl Into<RequestInput>,
        offer: impl Into<Offer>,
    ) -> Self {
        Self {
            id: request_id,
            requirements: requirements.into(),
            image_url: image_url.into(),
            input: input.into(),
            offer: offer.into(),
        }
    }

    /// Returns the client address from the request ID.
    pub fn client_address(&self) -> Address {
        self.id.addr
    }

    /// Returns the time, in seconds since the UNIX epoch, at which the request expires.
    pub fn expires_at(&self) -> u64 {
        self.offer.bidding_start + self.offer.timeout as u64
    }

    /// Returns true if the expiration time has passed, according to the system clock.
    ///
    /// NOTE: If the system clock has significant has drifted relative to the chain's clock, this
    /// may not give the correct result.
    #[cfg(not(target_os = "zkvm"))]
    pub fn is_expired(&self) -> bool {
        self.expires_at() < now_timestamp()
    }

    /// Returns the time, in seconds since the UNIX epoch, at which the request lock expires.
    pub fn lock_expires_at(&self) -> u64 {
        self.offer.bidding_start + self.offer.lock_timeout as u64
    }

    /// Returns true if the lock expiration time has passed, according to the system clock.
    ///
    /// NOTE: If the system clock has significant has drifted relative to the chain's clock, this
    /// may not give the correct result.
    pub fn is_lock_expired(&self) -> bool {
        self.lock_expires_at() < now_timestamp()
    }

    /// Return true if the request ID indicates that it is authorized by a smart contract, rather
    /// than an EOA (i.e. an ECDSA key).
    pub fn is_smart_contract_signed(&self) -> bool {
        self.id.smart_contract_signed
    }

    /// Check that the request is valid and internally consistent.
    ///
    /// If any field are empty, or if two fields conflict (e.g. the max price is less than the min
    /// price) this function will return an error.
    ///
    /// NOTE: This does not check whether the request has expired. You can use
    /// [ProofRequest::is_lock_expired] to do so.
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.id.addr == Address::ZERO {
            return Err(RequestError::ClientAddrIsZero);
        }
        if self.image_url.is_empty() {
            return Err(RequestError::EmptyImageUrl);
        }
        Url::parse(&self.image_url).map(|_| ())?;

        if self.requirements.image_id == B256::default() {
            return Err(RequestError::ImageIdIsZero);
        }
        if self.offer.timeout == 0 {
            return Err(RequestError::OfferTimeoutIsZero);
        }
        if self.offer.lock_timeout == 0 {
            return Err(RequestError::OfferLockTimeoutIsZero);
        }
        if self.offer.ramp_up_period > self.offer.lock_timeout {
            return Err(RequestError::OfferRampUpGreaterThanLockTimeout);
        }
        if self.offer.lock_timeout > self.offer.timeout {
            return Err(RequestError::OfferLockTimeoutGreaterThanTimeout);
        }
        if self.offer.timeout - self.offer.lock_timeout >= 1 << 24 {
            return Err(RequestError::OfferTimeoutRangeTooLarge);
        }
        if self.offer.max_price == U256::ZERO {
            return Err(RequestError::OfferMaxPriceIsZero);
        }
        if self.offer.max_price < self.offer.min_price {
            return Err(RequestError::OfferMaxPriceIsLessThanMin);
        }
        if self.offer.bidding_start == 0 {
            return Err(RequestError::OfferBiddingStartIsZero);
        }

        Ok(())
    }
}

impl Requirements {
    /// Creates a new requirements with the given image ID and predicate.
    pub fn new(image_id: impl Into<Digest>, predicate: Predicate) -> Self {
        Self {
            image_id: <[u8; 32]>::from(image_id.into()).into(),
            predicate,
            callback: Callback::default(),
            selector: UNSPECIFIED_SELECTOR,
        }
    }

    /// Sets the image ID.
    pub fn with_image_id(self, image_id: impl Into<Digest>) -> Self {
        Self { image_id: <[u8; 32]>::from(image_id.into()).into(), ..self }
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

    /// Set the selector for a groth16 proof.
    ///
    /// This will set the selector to the appropriate value based on the current environment.
    /// In dev mode, the selector will be set to `FakeReceipt`, otherwise it will be set
    /// to the latest groth16 version.
    pub fn with_groth16_proof(self) -> Self {
        match is_dev_mode() {
            true => Self { selector: Selector::fake_receipt(), ..self },
            false => Self { selector: Selector::groth16_latest(), ..self },
        }
    }
}

impl Predicate {
    /// Returns a predicate to match the journal digest. This ensures that the request's
    /// fulfillment will contain a journal with the same digest.
    pub fn digest_match(digest: impl Into<Digest>) -> Self {
        Self {
            predicate_type: PredicateType::DigestMatch,
            data: digest.into().as_bytes().to_vec().into(),
        }
    }

    /// Returns a predicate to match the journal prefix. This ensures that the request's
    /// fulfillment will contain a journal with the same prefix.
    pub fn prefix_match(prefix: impl Into<Bytes>) -> Self {
        Self { predicate_type: PredicateType::PrefixMatch, data: prefix.into() }
    }
}

impl Callback {
    /// Constant representing a none callback (i.e. no call will be made).
    pub const NONE: Self = Self { addr: Address::ZERO, gas_limit: U96::ZERO };

    /// Sets the address of the callback.
    pub fn with_addr(self, addr: impl Into<Address>) -> Self {
        Self { addr: addr.into(), ..self }
    }

    /// Sets the gas limit of the callback.
    pub fn with_gas_limit(self, gas_limit: u64) -> Self {
        Self { gas_limit: U96::from(gas_limit), ..self }
    }

    /// Returns true if this is a none callback (i.e. no call will be made).
    ///
    /// NOTE: A callback is considered none if the address is zero, regardless of the gas limit.
    pub fn is_none(&self) -> bool {
        self.addr == Address::ZERO
    }

    /// Convert to an option representation, mapping a none callback to `None`.
    pub fn into_option(self) -> Option<Self> {
        self.is_none().not().then_some(self)
    }

    /// Convert to an option representation, mapping a none callback to `None`.
    pub fn as_option(&self) -> Option<&Self> {
        self.is_none().not().then_some(self)
    }

    /// Convert from an option representation, mapping `None` to [Self::NONE].
    pub fn from_option(opt: Option<Self>) -> Self {
        opt.unwrap_or(Self::NONE)
    }
}

impl RequestInput {
    /// Create a new [GuestEnvBuilder] for use in constructing and encoding the guest zkVM environment.
    #[cfg(not(target_os = "zkvm"))]
    pub fn builder() -> GuestEnvBuilder {
        GuestEnvBuilder::new()
    }

    /// Sets the input type to inline and the data to the given bytes.
    ///
    /// See [GuestEnvBuilder] for more details on how to write input data.
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_cli::request::RequestInput;
    ///
    /// let input_vec = RequestInput::builder().write(&[0x41, 0x41, 0x41, 0x41])?.build_vec()?;
    /// let input = RequestInput::inline(input_vec);
    /// # anyhow::Ok(())
    /// ```
    pub fn inline(data: impl Into<Bytes>) -> Self {
        Self { input_type: RequestInputType::Inline, data: data.into() }
    }

    /// Sets the input type to URL and the data to the given URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self { input_type: RequestInputType::Url, data: url.into().as_bytes().to_vec().into() }
    }
}

impl From<Url> for RequestInput {
    /// Create a URL input from the given URL.
    fn from(value: Url) -> Self {
        Self::url(value)
    }
}

impl Offer {
    /// Sets the offer minimum price.
    pub fn with_min_price(self, min_price: U256) -> Self {
        Self { min_price, ..self }
    }

    /// Sets the offer maximum price.
    pub fn with_max_price(self, max_price: U256) -> Self {
        Self { max_price, ..self }
    }

    /// Sets the offer lock-in stake.
    pub fn with_lock_stake(self, lock_stake: U256) -> Self {
        Self { lock_stake, ..self }
    }

    /// Sets the offer bidding start time, in seconds since the UNIX epoch.
    pub fn with_bidding_start(self, bidding_start: u64) -> Self {
        Self { bidding_start, ..self }
    }

    /// Sets the offer timeout as seconds from the bidding start before expiring.
    pub fn with_timeout(self, timeout: u32) -> Self {
        Self { timeout, ..self }
    }

    /// Sets the offer lock-in timeout as seconds from the bidding start before expiring.
    pub fn with_lock_timeout(self, lock_timeout: u32) -> Self {
        Self { lock_timeout, ..self }
    }

    /// Sets the duration (in seconds) during which the auction price increases linearly
    /// from the minimum to the maximum price. After this period, the price remains at maximum.
    pub fn with_ramp_up_period(self, ramp_up_period: u32) -> Self {
        Self { ramp_up_period, ..self }
    }

    /// Sets the offer minimum price based on the desired price per million cycles.
    pub fn with_min_price_per_mcycle(self, mcycle_price: U256, mcycle: u64) -> Self {
        let min_price = mcycle_price * U256::from(mcycle);
        Self { min_price, ..self }
    }

    /// Sets the offer maximum price based on the desired price per million cycles.
    pub fn with_max_price_per_mcycle(self, mcycle_price: U256, mcycle: u64) -> Self {
        let max_price = mcycle_price * U256::from(mcycle);
        Self { max_price, ..self }
    }

    /// Sets the offer lock-in stake based on the desired price per million cycles.
    pub fn with_lock_stake_per_mcycle(self, mcycle_price: U256, mcycle: u64) -> Self {
        let lock_stake = mcycle_price * U256::from(mcycle);
        Self { lock_stake, ..self }
    }
}

impl Predicate {
    /// Evaluates the predicate against the given journal.
    #[inline]
    pub fn eval(&self, journal: impl AsRef<[u8]>) -> bool {
        match self.predicate_type {
            PredicateType::DigestMatch => self.data.as_ref() == Sha256::digest(journal).as_slice(),
            PredicateType::PrefixMatch => journal.as_ref().starts_with(&self.data),
            PredicateType::__Invalid => panic!("invalid PredicateType"),
        }
    }
}

/// Constant to specify when no selector is specified.
pub const UNSPECIFIED_SELECTOR: FixedBytes<4> = FixedBytes::<4>([0; 4]);

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_request_id() {
        // Test case 1: Regular signature
        let raw_id1 =
            U256::from_str("3130239009558586413752262552917257075388277690201777635428").unwrap();
        let request_id1 = RequestId::from_lossy(raw_id1);

        let client1 = request_id1.addr;
        let idx1 = request_id1.index;
        let is_smart_contract1 = request_id1.smart_contract_signed;

        assert_eq!(
            client1,
            Address::from_str("0x7FA9385bE102ac3EAc297483Dd6233D62b3e1496").unwrap()
        );
        assert_eq!(idx1, 100);
        assert!(!is_smart_contract1);

        // Test case 2: Smart contract signature
        let raw_id2 =
            U256::from_str("9407340744945267177588051976124923491490633134665812148266").unwrap();
        let request_id2 = RequestId::from_lossy(raw_id2);

        let client2 = request_id2.addr;
        let idx2 = request_id2.index;
        let is_smart_contract2 = request_id2.smart_contract_signed;

        assert_eq!(
            client2,
            Address::from_str("0x7FA9385bE102ac3EAc297483Dd6233D62b3e1496").unwrap()
        );
        assert_eq!(idx2, 42);
        assert!(is_smart_contract2);

        // Test conversion back to U256
        let request_id1_u256: U256 = request_id1.into();
        let request_id2_u256: U256 = request_id2.into();
        assert_eq!(request_id1_u256, raw_id1);
        assert_eq!(request_id2_u256, raw_id2);
    }
}
