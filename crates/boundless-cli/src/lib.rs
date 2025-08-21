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

#![deny(missing_docs)]

pub mod client;
#[cfg(feature = "cli")]
pub mod prover;
pub(crate) mod rpc;
pub mod util;

pub use boundless_core::input::{GuestEnv, GuestEnvBuilder};
pub use boundless_core::storage::{
    fetch_url, StandardStorageProvider, StorageProvider, StorageProviderConfig,
};
#[cfg(feature = "cli")]
pub use prover::*;

use alloy_primitives::{Address, Bytes, B256, U256};
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;
use tracing::level_filters::LevelFilter;
use url::Url;

#[cfg(feature = "cli")]
use boundless_market::contracts::RequestStatus as BoundlessRequestStatus;

/// Define the selector types.
///
/// This is used to indicate the type of proof that is being requested.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ProofType {
    /// Any proof type.
    #[default]
    Any,
    /// Groth16 proof type.
    Groth16,
    /// Inclusion proof type.
    Inclusion,
}

/// Configuration for a deployment of the Boundless Market.
// NOTE: See https://github.com/clap-rs/clap/issues/5092#issuecomment-1703980717 about clap usage.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Builder, Serialize, Deserialize)]
pub struct Deployment {
    /// EIP-155 chain ID of the network.
    #[builder(setter(into, strip_option), default)]
    pub chain_id: Option<u64>,

    /// Address of the [BoundlessMarket] contract.
    #[builder(setter(into))]
    pub boundless_market_address: Address,

    /// Address of the [RiscZeroVerifierRouter] contract.
    ///
    /// The verifier router implements [IRiscZeroVerifier]. Each network has a canonical router,
    /// that is deployed by the core team. You can additionally deploy and manage your own verifier
    /// instead. See the [Boundless docs for more details].
    ///
    /// [RiscZeroVerifierRouter]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroVerifierRouter.sol
    /// [IRiscZeroVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/IRiscZeroVerifier.sol
    /// [Boundless docs for more details]: https://docs.beboundless.xyz/developers/smart-contracts/verifier-contracts
    #[builder(setter(strip_option), default)]
    pub verifier_router_address: Option<Address>,

    /// Address of the [RiscZeroSetVerifier] contract.
    ///
    /// [RiscZeroSetVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroSetVerifier.sol
    #[builder(setter(into))]
    pub set_verifier_address: Address,

    /// Address of the stake token contract. The staking token is an ERC-20.
    #[builder(setter(strip_option), default)]
    pub stake_token_address: Option<Address>,

    /// URL for the offchain [order stream service].
    #[builder(setter(into, strip_option), default)]
    pub order_stream_url: Option<Cow<'static, str>>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct JsonRequest {
    pub config: JsonConfig,
    pub command: JsonCommand,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct JsonConfig {
    pub rpc_url: Url,
    pub private_key: Option<String>,
    pub tx_timeout_secs: Option<u64>,
    #[serde(with = "level_filter_serde")]
    pub log_level: LevelFilter,
    pub json: bool,
    pub deployment: Option<Deployment>,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum JsonCommand {
    Account(AccountCommand),
    Ops(OpsCommand),
    Request(RequestCommand),
    Proving(ProvingCommand),
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum AccountCommand {
    Deposit { amount: U256 },
    Withdraw { amount: U256 },
    Balance { address: Option<Address> },
    DepositStake { amount: String },
    WithdrawStake { amount: String },
    StakeBalance { address: Option<Address> },
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum OpsCommand {
    Slash { request_id: U256 },
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum RequestCommand {
    Submit {
        yaml_request: String,
        wait: bool,
        offchain: bool,
        no_preflight: bool,
        storage_config: Box<StorageProviderConfig>,
    },
    SubmitOffer(Box<RequestParams>),
    Status {
        request_id: U256,
        expires_at: Option<u64>,
    },
    GetProof {
        request_id: U256,
    },
    VerifyProof {
        request_id: U256,
        image_id: B256,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum ProvingCommand {
    Execute {
        request_path: Option<String>,
        request_id: Option<U256>,
        request_digest: Option<B256>,
        tx_hash: Option<B256>,
    },
    Fulfill {
        request_ids: Vec<U256>,
        request_digests: Option<Vec<B256>>,
        tx_hashes: Option<Vec<B256>>,
        withdraw: bool,
    },
    Lock {
        request_id: U256,
        request_digest: Option<B256>,
        tx_hash: Option<B256>,
    },
    Benchmark {
        request_ids: Vec<U256>,
        bonsai_api_url: Option<String>,
        bonsai_api_key: Option<String>,
    },
}

/// Status of a proof request
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq)]
pub enum RequestStatus {
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
    #[default]
    Unknown,
}

/// Parameters for submitting a request.
#[derive(Clone, Debug, Default, Builder, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestParams {
    #[builder(setter(into, strip_option, prefix = "with"), default)]
    /// The ID of the request.
    id: Option<u32>,
    #[builder(setter(into, prefix = "with"), default)]
    /// The program to use for the request.
    program: SubmitOfferProgram,
    #[builder(setter(into, prefix = "with"), default)]
    /// Whether to wait for the request to be fulfilled.
    wait: bool,
    #[builder(setter(into, prefix = "with"), default)]
    /// Whether to submit the request offchain.
    offchain: bool,
    #[builder(setter(into, prefix = "with"), default)]
    /// Whether to encode the input.
    encode_input: bool,
    #[builder(setter(into, prefix = "with"), default)]
    /// The input to use for the request.
    input: SubmitOfferInput,
    #[builder(setter(into, prefix = "with"), default)]
    /// The requirements for the request.
    requirements: SubmitOfferRequirements,
    #[builder(setter(into, strip_option, prefix = "with"), default)]
    /// The offer parameters to use for the request.
    offer_params: Option<OfferParams>,
    #[builder(setter(into, prefix = "with"), default)]
    /// The storage provider configuration to use for the request.
    storage_config: StorageProviderConfig,
}

#[non_exhaustive]
#[derive(Clone, Debug, Default, Builder, Serialize, Deserialize)]
/// A partial [Offer], with all the fields as optional. Used in the [OfferLayer] to override
/// defaults set in the [OfferLayerConfig].
pub struct OfferParams {
    /// Minimum price willing to pay for the proof, in wei.
    #[builder(setter(strip_option, into), default)]
    pub min_price: Option<U256>,

    /// Maximum price willing to pay for the proof, in wei.
    #[builder(setter(strip_option, into), default)]
    pub max_price: Option<U256>,

    /// Timestamp when bidding will start for this request.
    #[builder(setter(strip_option), default)]
    pub bidding_start: Option<u64>,

    /// Duration in seconds for the price to ramp up from min to max.
    #[builder(setter(strip_option), default)]
    pub ramp_up_period: Option<u32>,

    /// Time in seconds that a prover has to fulfill a locked request.
    #[builder(setter(strip_option), default)]
    pub lock_timeout: Option<u32>,

    /// Maximum time in seconds that a request can remain active.
    #[builder(setter(strip_option), default)]
    pub timeout: Option<u32>,

    /// Amount of the stake token that the prover must stake when locking a request.
    #[builder(setter(strip_option, into), default)]
    pub lock_stake: Option<U256>,
}

impl From<OfferParamsBuilder> for OfferParams {
    fn from(value: OfferParamsBuilder) -> Self {
        // Builder should be infallible.
        value.build().expect("implementation error in OfferParams")
    }
}

// Allows for a nicer builder pattern in RequestParams.
impl From<&mut OfferParamsBuilder> for OfferParams {
    fn from(value: &mut OfferParamsBuilder) -> Self {
        value.clone().into()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SubmitOfferProgram {
    /// Program binary to use as the guest image, given as a path.
    ///
    /// The program will be uploaded to a public URL using the configured storage provider before
    /// the proof request is sent.
    path: Option<PathBuf>,
    /// Program binary to use as a guest image, given as a public URL.
    ///
    /// This option accepts a pre-uploaded program. If also using small inputs, a storage provider
    /// is not required when using a pre-uploaded program.
    url: Option<Url>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SubmitOfferRequirements {
    /// Address of the callback to use in the requirements.
    callback_address: Option<Address>,
    /// Gas limit of the callback to use in the requirements.
    callback_gas_limit: Option<u64>,
    /// Request a groth16 proof (i.e., a Groth16).
    proof_type: ProofType,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SubmitOfferInput {
    /// The input data for the offer.
    input: Option<String>,
    /// The input file for the offer.
    input_file: Option<PathBuf>,
}

impl RequestParamsBuilder {
    /// Seth the program for the offer using a local path.
    pub fn with_program_path<P: Into<PathBuf>>(&mut self, path: P) -> &mut Self {
        self.program = Some(SubmitOfferProgram { path: Some(path.into()), url: None });
        self
    }

    /// Sets the program for the offer using a public URL.
    pub fn with_program_url<U: Into<Url>>(&mut self, url: U) -> &mut Self {
        self.program = Some(SubmitOfferProgram { path: None, url: Some(url.into()) });
        self
    }

    /// Sets the callback address and gas limit for the offer.
    pub fn with_callback(&mut self, address: Address, gas_limit: u64) -> &mut Self {
        self.requirements = Some(SubmitOfferRequirements {
            callback_address: Some(address),
            callback_gas_limit: Some(gas_limit),
            proof_type: ProofType::Groth16,
        });
        self
    }

    /// Sets the proof type for the offer.
    pub fn with_proof_type(&mut self, proof_type: ProofType) -> &mut Self {
        if let Some(requirements) = &mut self.requirements {
            requirements.proof_type = proof_type;
        } else {
            self.requirements = Some(SubmitOfferRequirements {
                callback_address: None,
                callback_gas_limit: None,
                proof_type,
            });
        }
        self
    }

    /// Sets the input for the offer.
    ///
    /// The data must be already encoded.
    pub fn with_stdin(&mut self, data: impl Into<Vec<u8>>) -> &mut Self {
        self.input =
            Some(SubmitOfferInput { input: Some(hex::encode(data.into())), input_file: None });
        self
    }
}

#[cfg(feature = "cli")]
impl From<BoundlessRequestStatus> for RequestStatus {
    fn from(status: BoundlessRequestStatus) -> Self {
        match status {
            BoundlessRequestStatus::Expired => RequestStatus::Expired,
            BoundlessRequestStatus::Locked => RequestStatus::Locked,
            BoundlessRequestStatus::Fulfilled => RequestStatus::Fulfilled,
            _ => RequestStatus::Unknown,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "command", rename_all = "kebab-case")]
/// Output of the CLI commands.
pub enum Output {
    /// Indicates that the CLI command was successful.
    Ok,
    /// The account balance in ether.
    AccountAmount {
        /// The account balance amount in ether.
        amount_eth: String,
    },
    /// The account stake amount.
    AccountStakeAmount {
        /// The account stake amount.
        amount: String,
        /// The number of decimals for the stake amount.
        decimals: u8,
        /// The symbol for the stake amount (e.g. "HP" or "USDC").
        symbol: String,
    },
    /// The status of the proof request.
    RequestStatus {
        /// The status of the request.
        status: RequestStatus,
    },
    /// The ID and expiration time of the submitted request.
    RequestSubmitted {
        /// The ID of the request.
        request_id: U256,
        /// The expiration time of the request.
        expires_at: u64,
    },
    /// The journal and seal of the fulfilled request.
    RequestFulfilled {
        /// The journal of the fulfilled request.
        journal: Bytes,
        /// The seal of the fulfilled request.
        seal: Bytes,
    },
}

#[derive(Serialize, Deserialize)]
/// Response structure for CLI commands.
pub struct Response<T: Serialize> {
    /// Indicates whether the command was successful.
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// The data returned by the command, if any.
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// The error message, if any.
    pub error: Option<String>,
}

#[cfg(feature = "cli")]
/// Serializes a private key signer to a hex string, or `None` if the signer is not present.
pub mod private_key_signer_serde {
    use alloy::signers::local::PrivateKeySigner;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes a private key signer to a hex string, or `None` if the signer is not present.
    pub fn serialize<S>(key: &Option<PrivateKeySigner>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match key {
            Some(pk) => {
                // The address isn't enough — we want the secret key
                let hex_str = format!("0x{}", hex::encode(pk.to_bytes()));
                s.serialize_str(&hex_str)
            }
            None => s.serialize_none(),
        }
    }

    /// Deserializes a private key signer from a hex string, or `None` if the string is empty.
    pub fn deserialize<'de, D>(d: D) -> Result<Option<PrivateKeySigner>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<String>::deserialize(d)?;
        match opt {
            Some(hex_str) => {
                let s = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
                let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
                let pk = PrivateKeySigner::from_bytes(&alloy::primitives::FixedBytes(
                    bytes[..32].try_into().unwrap(),
                ))
                .map_err(serde::de::Error::custom)?;
                Ok(Some(pk))
            }
            None => Ok(None),
        }
    }
}

/// Serializes a logging level filter to a string.
pub mod level_filter_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::str::FromStr;

    /// Serializes a logging level filter to a string.
    pub fn serialize<S>(lf: &LevelFilter, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(lf.to_string().as_str())
    }

    /// Deserializes a logging level filter from a string.
    pub fn deserialize<'de, D>(d: D) -> Result<LevelFilter, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        LevelFilter::from_str(&s).map_err(serde::de::Error::custom)
    }
}
