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

//! The Boundless CLI is a command-line interface for interacting with the Boundless market.
//!
//! # Environment Variables
//!
//! The CLI relies on the following environment variables:
//!
//! Required variables:
//! - `PRIVATE_KEY`: Private key for your Ethereum wallet
//! - `BOUNDLESS_MARKET_ADDRESS`: Address of the Boundless market contract
//! - `VERIFIER_ADDRESS`: Address of the VerifierRouter contract
//! - `SET_VERIFIER_ADDRESS`: Address of the Set Verifier contract
//!
//! Optional variables:
//! - `RPC_URL`: URL of the Ethereum RPC endpoint (default: http://localhost:8545)
//! - `TX_TIMEOUT`: Transaction timeout in seconds
//! - `LOG_LEVEL`: Log level (error, warn, info, debug, trace; default: info)
//! - `ORDER_STREAM_URL`: URL of the order stream service (for offchain requests)
//!
//! You can set these variables by:
//! 1. Running `source .env.localnet` if using the provided environment file
//! 2. Exporting them directly in your shell: `export PRIVATE_KEY=1234...`

use std::{
    borrow::Cow,
    fs::File,
    io::BufReader,
    num::ParseIntError,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use alloy::{
    network::Ethereum,
    primitives::{
        aliases::U96,
        utils::{format_ether, parse_ether},
        Address, Bytes, FixedBytes, TxKind, B256, U256,
    },
    providers::{network::EthereumWallet, Provider, ProviderBuilder},
    rpc::types::{TransactionInput, TransactionRequest},
    signers::{local::PrivateKeySigner, Signer},
    sol_types::SolValue,
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use boundless_cli::{convert_timestamp, fetch_url, DefaultProver, OrderFulfilled};
use clap::{Args, Parser, Subcommand};
use hex::FromHex;
use risc0_aggregation::SetInclusionReceiptVerifierParameters;
use risc0_ethereum_contracts::{set_verifier::SetVerifierService, IRiscZeroVerifier};
use risc0_zkvm::{
    default_executor,
    sha::{Digest, Digestible},
    ExecutorEnv, Journal, SessionInfo,
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use url::Url;

use boundless_market::{
    client::{Client, ClientBuilder},
    contracts::{
        boundless_market::BoundlessMarketService, Callback, Input, InputType, Offer, Predicate,
        PredicateType, ProofRequest, RequestId, Requirements, UNSPECIFIED_SELECTOR,
    },
    input::{GuestEnv, InputBuilder},
    selector::ProofType,
    storage::{StorageProvider, StorageProviderConfig},
};

#[derive(Subcommand, Clone, Debug)]
enum Command {
    /// Account management commands
    #[command(subcommand)]
    Account(Box<AccountCommands>),

    /// Proof request commands
    #[command(subcommand)]
    Request(Box<RequestCommands>),

    /// Proof execution commands
    #[command(subcommand)]
    Proving(Box<ProvingCommands>),

    /// Operations on the boundless market
    #[command(subcommand)]
    Ops(Box<OpsCommands>),

    /// Display configuration and environment variables
    Config {
        /// Show raw values for sensitive information like private keys
        #[clap(long)]
        show_sensitive: bool,
    },
}

#[derive(Subcommand, Clone, Debug)]
enum OpsCommands {
    /// Slash a prover for a given request
    Slash {
        /// The proof request identifier
        request_id: U256,
    },
}

#[derive(Subcommand, Clone, Debug)]
enum AccountCommands {
    /// Deposit funds into the market
    Deposit {
        /// Amount in ether to deposit
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// Withdraw funds from the market
    Withdraw {
        /// Amount in ether to withdraw
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// Check the balance of an account in the market
    Balance {
        /// Address to check the balance of;
        /// if not provided, defaults to the wallet address
        address: Option<Address>,
    },
    /// Deposit stake funds into the market
    DepositStake {
        /// Amount in HP to deposit.
        ///
        /// e.g. 10 is uint256(10 * 10**18).
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// Withdraw stake funds from the market
    WithdrawStake {
        /// Amount in HP to withdraw.
        ///
        /// e.g. 10 is uint256(10 * 10**18).
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// Check the stake balance of an account in the market
    StakeBalance {
        /// Address to check the balance of;
        /// if not provided, defaults to the wallet address
        address: Option<Address>,
    },
}

#[derive(Subcommand, Clone, Debug)]
enum RequestCommands {
    /// Submit a proof request constructed with the given offer, input, and image
    SubmitOffer(SubmitOfferArgs),

    /// Submit a fully specified proof request
    Submit {
        /// Storage provider to use
        #[clap(flatten)]
        storage_config: Option<StorageProviderConfig>,

        /// Path to a YAML file containing the request
        yaml_request: PathBuf,

        /// Optional identifier for the request
        id: Option<u32>,

        /// Wait until the request is fulfilled
        #[clap(short, long, default_value = "false")]
        wait: bool,

        /// Submit the request offchain via the provided order stream service url
        #[clap(short, long, requires = "order_stream_url")]
        offchain: bool,

        /// Offchain order stream service URL to submit offchain requests to
        #[clap(
            long,
            env = "ORDER_STREAM_URL",
            default_value = "https://order-stream.beboundless.xyz"
        )]
        order_stream_url: Option<Url>,

        /// Skip preflight check (not recommended)
        #[clap(long, default_value = "false")]
        no_preflight: bool,

        /// Set the proof type.
        #[clap(long, value_enum, default_value_t = ProofType::Any)]
        proof_type: ProofType,

        /// Address of the callback to use in the requirements.
        #[clap(long, requires = "callback_gas_limit")]
        callback_address: Option<Address>,

        /// Gas limit of the callback to use in the requirements.
        #[clap(long, requires = "callback_address")]
        callback_gas_limit: Option<u64>,
    },

    /// Get the status of a given request
    Status {
        /// The proof request identifier
        request_id: U256,

        /// The time at which the request expires, in seconds since the UNIX epoch
        expires_at: Option<u64>,
    },

    /// Get the journal and seal for a given request
    GetProof {
        /// The proof request identifier
        request_id: U256,
    },

    /// Verify the proof of the given request against the SetVerifier contract
    VerifyProof {
        /// The proof request identifier
        request_id: U256,

        /// The image id of the original request
        image_id: B256,
    },
}

#[derive(Subcommand, Clone, Debug)]
enum ProvingCommands {
    /// Execute a proof request using the RISC Zero zkVM executor
    Execute {
        /// Path to a YAML file containing the request.
        ///
        /// If provided, the request will be loaded from the given file path.
        #[arg(long, conflicts_with_all = ["request_id", "tx_hash"])]
        request_path: Option<PathBuf>,

        /// The proof request identifier.
        ///
        /// If provided, the request will be fetched from the blockchain.
        #[arg(long, conflicts_with = "request_path")]
        request_id: Option<U256>,

        /// The request digest
        ///
        /// If provided along with request-id, uses the request digest to find the request.
        #[arg(long)]
        request_digest: Option<B256>,

        /// The tx hash of the request submission.
        ///
        /// If provided along with request-id, uses the transaction hash to find the request.
        #[arg(long, conflicts_with = "request_path", requires = "request_id")]
        tx_hash: Option<B256>,

        /// The order stream service URL.
        ///
        /// If provided, the request will be fetched offchain via the provided order stream service URL.
        #[arg(long, env = "ORDER_STREAM_URL", conflicts_with_all = ["request_path", "tx_hash"])]
        order_stream_url: Option<Url>,
    },

    /// Fulfill one or more proof requests using the RISC Zero zkVM default prover.
    ///
    /// This command can process multiple requests in a single batch, which is more efficient
    /// than fulfilling requests individually.
    ///
    /// Example usage:
    ///   --request-ids 0x123,0x456,0x789  # Comma-separated list of request IDs
    ///   --request-digests 0xabc,0xdef,0x012  # Optional, must match request_ids length and order
    ///   --tx-hashes 0x111,0x222,0x333  # Optional, must match request_ids length and order
    Fulfill {
        /// The proof requests identifiers (comma-separated list of hex values)
        #[arg(long, value_delimiter = ',')]
        request_ids: Vec<U256>,

        /// The request digests (comma-separated list of hex values).
        /// If provided, must have the same length and order as request_ids.
        #[arg(long, value_delimiter = ',')]
        request_digests: Option<Vec<B256>>,

        /// The tx hash of the requests submissions (comma-separated list of hex values).
        /// If provided, must have the same length and order as request_ids.
        #[arg(long, value_delimiter = ',')]
        tx_hashes: Option<Vec<B256>>,

        /// The order stream service URL.
        ///
        /// If provided, the request will be fetched offchain via the provided order stream service URL.
        #[arg(long, env = "ORDER_STREAM_URL", conflicts_with_all = ["tx_hash"])]
        order_stream_url: Option<Url>,
    },

    /// Lock a request in the market
    Lock {
        /// The proof request identifier
        #[arg(long)]
        request_id: U256,

        /// The request digest
        #[arg(long)]
        request_digest: Option<B256>,

        /// The tx hash of the request submission
        #[arg(long)]
        tx_hash: Option<B256>,

        /// The order stream service URL.
        ///
        /// If provided, the request will be fetched offchain via the provided order stream service URL.
        #[arg(long, env = "ORDER_STREAM_URL", conflicts_with_all = ["tx_hash"])]
        order_stream_url: Option<Url>,
    },
}

#[derive(Args, Clone, Debug)]
struct SubmitOfferArgs {
    /// Storage provider to use
    #[clap(flatten)]
    storage_config: Option<StorageProviderConfig>,

    /// Path to a YAML file containing the offer
    yaml_offer: PathBuf,

    /// Optional identifier for the request
    id: Option<u32>,

    /// Wait until the request is fulfilled
    #[clap(short, long, default_value = "false")]
    wait: bool,

    /// Submit the request offchain via the provided order stream service url
    #[clap(short, long, requires = "order_stream_url")]
    offchain: bool,

    /// Offchain order stream service URL to submit offchain requests to
    #[clap(long, env = "ORDER_STREAM_URL", default_value = "https://order-stream.beboundless.xyz")]
    order_stream_url: Option<Url>,

    /// Skip preflight check (not recommended)
    #[clap(long, default_value = "false")]
    no_preflight: bool,

    /// Use risc0_zkvm::serde to encode the input as a `Vec<u8>`
    #[clap(short, long)]
    encode_input: bool,

    /// Send the input inline (i.e. in the transaction calldata) rather than uploading it
    #[clap(long)]
    inline_input: bool,

    /// Program binary file to use as the guest image, given as a path
    #[clap(long)]
    program: PathBuf,

    #[command(flatten)]
    input: SubmitOfferInput,

    #[command(flatten)]
    reqs: SubmitOfferRequirements,
}

#[derive(Args, Clone, Debug)]
#[group(required = true, multiple = false)]
struct SubmitOfferInput {
    /// Input for the guest, given as a string.
    #[clap(short, long)]
    input: Option<String>,
    /// Input for the guest, given as a path to a file.
    #[clap(long)]
    input_file: Option<PathBuf>,
}

#[derive(Args, Clone, Debug)]
#[group(required = true, multiple = false)]
struct SubmitOfferRequirements {
    /// Hex encoded journal digest to use as the predicate in the requirements.
    #[clap(short, long)]
    journal_digest: Option<String>,
    /// Journal prefix to use as the predicate in the requirements.
    #[clap(long)]
    journal_prefix: Option<String>,
    /// Address of the callback to use in the requirements.
    #[clap(long, requires = "callback_gas_limit")]
    callback_address: Option<Address>,
    /// Gas limit of the callback to use in the requirements.
    #[clap(long, requires = "callback_address")]
    callback_gas_limit: Option<u64>,
    /// Request a groth16 proof (i.e., a Groth16).
    #[clap(long)]
    proof_type: ProofType,
}

/// Common configuration options for all commands
#[derive(Args, Debug, Clone)]
struct GlobalConfig {
    /// URL of the Ethereum RPC endpoint
    #[clap(short, long, env = "RPC_URL", default_value = "http://localhost:8545")]
    rpc_url: Url,

    /// Private key of the wallet (without 0x prefix)
    #[clap(long, env = "PRIVATE_KEY", hide_env_values = true)]
    private_key: PrivateKeySigner,

    /// Address of the market contract
    #[clap(short, long, env = "BOUNDLESS_MARKET_ADDRESS")]
    boundless_market_address: Address,

    /// Address of the VerifierRouter contract
    #[clap(short, long, env = "VERIFIER_ADDRESS")]
    verifier_address: Address,

    /// Address of the SetVerifier contract
    #[clap(short, long, env = "SET_VERIFIER_ADDRESS")]
    set_verifier_address: Address,

    /// Tx timeout in seconds
    #[clap(long, env = "TX_TIMEOUT", value_parser = |arg: &str| -> Result<Duration, ParseIntError> {Ok(Duration::from_secs(arg.parse()?))})]
    tx_timeout: Option<Duration>,

    /// Log level (error, warn, info, debug, trace)
    #[clap(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: LevelFilter,
}

#[derive(Parser, Debug)]
#[clap(author, version, about = "CLI for the Boundless market", long_about = None)]
struct MainArgs {
    #[command(flatten)]
    config: GlobalConfig,

    /// Subcommand to run
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file if it exists
    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => tracing::debug!("No .env file found"),
        Err(e) => bail!("failed to load .env file: {}", e),
    }

    let args = match MainArgs::try_parse() {
        Ok(args) => args,
        Err(err) => {
            if err.kind() == clap::error::ErrorKind::DisplayHelp {
                // If it's a help request, print the help and exit successfully
                err.print()?;
                return Ok(());
            }
            if err.kind() == clap::error::ErrorKind::MissingRequiredArgument {
                eprintln!("\nThe Boundless CLI requires certain configuration values, which can be provided either:");
                eprintln!("1. As environment variables (PRIVATE_KEY, BOUNDLESS_MARKET_ADDRESS, VERIFIER_ADDRESS, SET_VERIFIER_ADDRESS)");
                eprintln!("2. As command-line arguments (--private-key <KEY> --boundless-market-address <ADDR>  --verifier-address <ADDR> --set-verifier-address <ADDR>)");
                eprintln!();
            }

            return Err(err.into());
        }
    };

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(args.config.log_level.into())
                .from_env_lossy(),
        )
        .init();

    if let Err(e) = run(&args).await {
        tracing::error!("Command failed: {}", e);
        if let Some(ctx) = e.source() {
            tracing::error!("Context: {}", ctx);
        }
        bail!("{}", e)
    }
    Ok(())
}

pub(crate) async fn run(args: &MainArgs) -> Result<()> {
    let caller = args.config.private_key.address();
    let wallet = EthereumWallet::from(args.config.private_key.clone());
    let provider = ProviderBuilder::new().wallet(wallet).on_http(args.config.rpc_url.clone());

    let mut boundless_market =
        BoundlessMarketService::new(args.config.boundless_market_address, provider.clone(), caller);

    if let Some(tx_timeout) = args.config.tx_timeout {
        boundless_market = boundless_market.with_timeout(tx_timeout);
    }

    match &args.command {
        Command::Account(account_cmd) => {
            handle_account_command(account_cmd, boundless_market, args.config.private_key.clone())
                .await
        }
        Command::Request(request_cmd) => {
            handle_request_command(request_cmd, args, boundless_market, provider.clone()).await
        }
        Command::Proving(proving_cmd) => {
            handle_proving_command(proving_cmd, args, boundless_market, caller, provider.clone())
                .await
        }
        Command::Ops(operation_cmd) => handle_ops_command(operation_cmd, boundless_market).await,
        Command::Config { show_sensitive } => handle_config_command(args, *show_sensitive).await,
    }
}

/// Handle ops-related commands
async fn handle_ops_command<P>(
    cmd: &OpsCommands,
    boundless_market: BoundlessMarketService<P>,
) -> Result<()>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    match cmd {
        OpsCommands::Slash { request_id } => {
            tracing::info!("Slashing prover for request 0x{:x}", request_id);
            boundless_market.slash(*request_id).await?;
            tracing::info!("Successfully slashed prover for request 0x{:x}", request_id);
            Ok(())
        }
    }
}

/// Handle account-related commands
async fn handle_account_command<P>(
    cmd: &AccountCommands,
    boundless_market: BoundlessMarketService<P>,
    private_key: PrivateKeySigner,
) -> Result<()>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    match cmd {
        AccountCommands::Deposit { amount } => {
            tracing::info!("Depositing {} ETH into the market", format_ether(*amount));
            boundless_market.deposit(*amount).await?;
            tracing::info!("Successfully deposited {} ETH into the market", format_ether(*amount));
            Ok(())
        }
        AccountCommands::Withdraw { amount } => {
            tracing::info!("Withdrawing {} ETH from the market", format_ether(*amount));
            boundless_market.withdraw(*amount).await?;
            tracing::info!("Successfully withdrew {} ETH from the market", format_ether(*amount));
            Ok(())
        }
        AccountCommands::Balance { address } => {
            let addr = address.unwrap_or(boundless_market.caller());
            tracing::info!("Checking balance for address {}", addr);
            let balance = boundless_market.balance_of(addr).await?;
            tracing::info!("Balance for address {}: {} ETH", addr, format_ether(balance));
            Ok(())
        }
        AccountCommands::DepositStake { amount } => {
            tracing::info!("Depositing {} HP as stake", format_ether(*amount));
            match boundless_market.deposit_stake_with_permit(*amount, &private_key).await {
                Ok(_) => {
                    tracing::info!("Successfully deposited {} HP as stake", format_ether(*amount));
                    Ok(())
                }
                Err(e) => {
                    if e.to_string().contains("TRANSFER_FROM_FAILED") {
                        let addr = boundless_market.caller();
                        Err(anyhow!(
                            "Failed to deposit stake: Ensure your address ({}) has funds on the HP contract", addr
                        ))
                    } else {
                        Err(anyhow!("Failed to deposit stake: {}", e))
                    }
                }
            }
        }
        AccountCommands::WithdrawStake { amount } => {
            tracing::info!("Withdrawing {} HP from stake", format_ether(*amount));
            boundless_market.withdraw_stake(*amount).await?;
            tracing::info!("Successfully withdrew {} HP from stake", format_ether(*amount));
            Ok(())
        }
        AccountCommands::StakeBalance { address } => {
            let addr = address.unwrap_or(boundless_market.caller());
            tracing::info!("Checking stake balance for address {}", addr);
            let balance = boundless_market.balance_of_stake(addr).await?;
            tracing::info!("Stake balance for address {}: {} HP", addr, format_ether(balance));
            Ok(())
        }
    }
}

/// Handle request-related commands
async fn handle_request_command<P>(
    cmd: &RequestCommands,
    args: &MainArgs,
    boundless_market: BoundlessMarketService<P>,
    provider: impl Provider<Ethereum> + 'static + Clone,
) -> Result<()>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    match cmd {
        RequestCommands::SubmitOffer(offer_args) => {
            tracing::info!("Submitting new proof request with offer");
            let order_stream_url = offer_args
                .offchain
                .then_some(
                    offer_args
                        .order_stream_url
                        .clone()
                        .ok_or(anyhow!("offchain flag set, but order stream URL not provided")),
                )
                .transpose()?;
            let client = ClientBuilder::new()
                .with_private_key(args.config.private_key.clone())
                .with_rpc_url(args.config.rpc_url.clone())
                .with_boundless_market_address(args.config.boundless_market_address)
                .with_set_verifier_address(args.config.set_verifier_address)
                .with_storage_provider_config(offer_args.storage_config.clone())
                .await?
                .with_order_stream_url(order_stream_url)
                .with_timeout(args.config.tx_timeout)
                .build()
                .await?;

            submit_offer(client, &args.config.private_key, offer_args).await
        }
        RequestCommands::Submit {
            storage_config,
            yaml_request,
            id,
            wait,
            offchain,
            order_stream_url,
            no_preflight,
            proof_type,
            callback_address,
            callback_gas_limit,
        } => {
            tracing::info!("Submitting proof request from YAML file");
            let id = match id {
                Some(id) => *id,
                None => boundless_market.index_from_rand().await?,
            };

            let order_stream_url = offchain
                .then_some(
                    order_stream_url
                        .clone()
                        .ok_or(anyhow!("offchain flag set, but order stream URL not provided")),
                )
                .transpose()?;
            let client = ClientBuilder::new()
                .with_private_key(args.config.private_key.clone())
                .with_rpc_url(args.config.rpc_url.clone())
                .with_boundless_market_address(args.config.boundless_market_address)
                .with_set_verifier_address(args.config.set_verifier_address)
                .with_order_stream_url(order_stream_url.clone())
                .with_storage_provider_config(storage_config.clone())
                .await?
                .with_timeout(args.config.tx_timeout)
                .build()
                .await?;

            submit_request(
                id,
                yaml_request,
                client,
                &args.config.private_key,
                SubmitOptions {
                    wait: *wait,
                    offchain: *offchain,
                    preflight: !*no_preflight,
                    proof_type: proof_type.clone(),
                    callback_address: *callback_address,
                    callback_gas_limit: *callback_gas_limit,
                },
            )
            .await
        }
        RequestCommands::Status { request_id, expires_at } => {
            tracing::info!("Checking status for request 0x{:x}", request_id);
            let status = boundless_market.get_status(*request_id, *expires_at).await?;
            tracing::info!("Request 0x{:x} status: {:?}", request_id, status);
            Ok(())
        }
        RequestCommands::GetProof { request_id } => {
            tracing::info!("Fetching proof for request 0x{:x}", request_id);
            let (journal, seal) = boundless_market.get_request_fulfillment(*request_id).await?;
            tracing::info!("Successfully retrieved proof for request 0x{:x}", request_id);
            tracing::info!(
                "Journal: {} - Seal: {}",
                serde_json::to_string_pretty(&journal)?,
                serde_json::to_string_pretty(&seal)?
            );
            Ok(())
        }
        RequestCommands::VerifyProof { request_id, image_id } => {
            tracing::info!("Verifying proof for request 0x{:x}", request_id);
            let (journal, seal) = boundless_market.get_request_fulfillment(*request_id).await?;
            let journal_digest = <[u8; 32]>::from(Journal::new(journal.to_vec()).digest()).into();
            let verifier = IRiscZeroVerifier::new(args.config.verifier_address, provider.clone());

            verifier
                .verify(seal, *image_id, journal_digest)
                .call()
                .await
                .map_err(|_| anyhow::anyhow!("Verification failed"))?;

            tracing::info!("Successfully verified proof for request 0x{:x}", request_id);
            Ok(())
        }
    }
}

/// Handle proving-related commands
async fn handle_proving_command<P>(
    cmd: &ProvingCommands,
    args: &MainArgs,
    boundless_market: BoundlessMarketService<P>,
    caller: Address,
    provider: impl Provider<Ethereum> + 'static + Clone,
) -> Result<()>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    match cmd {
        ProvingCommands::Execute {
            request_path,
            request_id,
            request_digest,
            tx_hash,
            order_stream_url,
        } => {
            tracing::info!("Executing proof request");
            let request: ProofRequest = if let Some(file_path) = request_path {
                tracing::debug!("Loading request from file: {:?}", file_path);
                let file = File::open(file_path).context("failed to open request file")?;
                let reader = BufReader::new(file);
                serde_yaml::from_reader(reader).context("failed to parse request from YAML")?
            } else if let Some(request_id) = request_id {
                tracing::debug!("Loading request from blockchain: 0x{:x}", request_id);
                let client = ClientBuilder::new()
                    .with_private_key(args.config.private_key.clone())
                    .with_rpc_url(args.config.rpc_url.clone())
                    .with_boundless_market_address(args.config.boundless_market_address)
                    .with_set_verifier_address(args.config.set_verifier_address)
                    .with_order_stream_url(order_stream_url.clone())
                    .with_timeout(args.config.tx_timeout)
                    .build()
                    .await?;
                let order = client.fetch_order(*request_id, *tx_hash, *request_digest).await?;
                order.request
            } else {
                bail!("execute requires either a request file path or request ID")
            };

            let session_info = execute(&request).await?;
            let journal = session_info.journal.bytes;

            if !request.requirements.predicate.eval(&journal) {
                tracing::error!("Predicate evaluation failed for request");
                bail!("Predicate evaluation failed");
            }

            tracing::info!("Successfully executed request 0x{:x}", request.id);
            tracing::debug!("Journal: {:?}", journal);
            Ok(())
        }
        ProvingCommands::Fulfill { request_ids, request_digests, tx_hashes, order_stream_url } => {
            if request_digests.is_some()
                && request_ids.len() != request_digests.as_ref().unwrap().len()
            {
                bail!("request_ids and request_digests must have the same length");
            }
            if tx_hashes.is_some() && request_ids.len() != tx_hashes.as_ref().unwrap().len() {
                bail!("request_ids and tx_hashes must have the same length");
            }

            let request_ids_string =
                request_ids.iter().map(|id| format!("0x{:x}", id)).collect::<Vec<_>>().join(", ");
            tracing::info!("Fulfilling proof requests {}", request_ids_string);

            let (_, market_url) = boundless_market.image_info().await?;
            tracing::debug!("Fetching Assessor program from {}", market_url);
            let assessor_elf = fetch_url(&market_url).await?;
            let domain = boundless_market.eip712_domain().await?;

            let mut set_verifier =
                SetVerifierService::new(args.config.set_verifier_address, provider.clone(), caller);

            if let Some(tx_timeout) = args.config.tx_timeout {
                set_verifier = set_verifier.with_timeout(tx_timeout);
            }

            let (_, set_builder_url) = set_verifier.image_info().await?;
            tracing::debug!("Fetching SetBuilder program from {}", set_builder_url);
            let set_builder_elf = fetch_url(&set_builder_url).await?;

            let prover = DefaultProver::new(set_builder_elf, assessor_elf, caller, domain)?;

            let client = ClientBuilder::new()
                .with_private_key(args.config.private_key.clone())
                .with_rpc_url(args.config.rpc_url.clone())
                .with_boundless_market_address(args.config.boundless_market_address)
                .with_set_verifier_address(args.config.set_verifier_address)
                .with_order_stream_url(order_stream_url.clone())
                .with_timeout(args.config.tx_timeout)
                .build()
                .await?;

            let fetch_order_jobs = request_ids.iter().enumerate().map(|(i, request_id)| {
                let client = client.clone();
                let boundless_market = boundless_market.clone();
                async move {
                    let order = client
                        .fetch_order(
                            *request_id,
                            tx_hashes.as_ref().map(|tx_hashes| tx_hashes[i]),
                            request_digests.as_ref().map(|request_digests| request_digests[i]),
                        )
                        .await?;
                    tracing::debug!("Fetched order details: {:?}", order.request);

                    let sig: Bytes = order.signature.as_bytes().into();
                    order.request.verify_signature(
                        &sig,
                        args.config.boundless_market_address,
                        boundless_market.get_chain_id().await?,
                    )?;
                    let is_locked = boundless_market.is_locked(*request_id).await?;
                    Ok::<_, anyhow::Error>((order, sig, is_locked))
                }
            });

            let results = futures::future::join_all(fetch_order_jobs).await;
            let mut orders = Vec::new();
            let mut signatures = Vec::new();
            let mut requests_to_price = Vec::new();

            for result in results {
                let (order, sig, is_locked) = result?;
                // If the request is not locked in, we need to "price" which checks the requirements
                // and assigns a price. Otherwise, we don't. This vec will be a singleton if not locked
                // and empty if the request is locked.
                if !is_locked {
                    requests_to_price.push(order.request.clone());
                }
                orders.push(order);
                signatures.push(sig);
            }

            let (fills, root_receipt, assessor_receipt) = prover.fulfill(&orders).await?;
            let order_fulfilled = OrderFulfilled::new(fills, root_receipt, assessor_receipt)?;
            tracing::debug!("Submitting root {} to SetVerifier", order_fulfilled.root);
            set_verifier.submit_merkle_root(order_fulfilled.root, order_fulfilled.seal).await?;
            tracing::debug!("Successfully submitted root to SetVerifier");

            match boundless_market
                .price_and_fulfill_batch(
                    requests_to_price,
                    signatures,
                    order_fulfilled.fills,
                    order_fulfilled.assessorReceipt,
                    None,
                )
                .await
            {
                Ok(_) => {
                    tracing::info!("Successfully fulfilled requests {}", request_ids_string);
                    Ok(())
                }
                Err(e) => {
                    tracing::error!("Failed to fulfill requests {}: {}", request_ids_string, e);
                    bail!("Failed to fulfill request: {}", e)
                }
            }
        }
        ProvingCommands::Lock { request_id, request_digest, tx_hash, order_stream_url } => {
            tracing::info!("Locking proof request 0x{:x}", request_id);
            let client = ClientBuilder::new()
                .with_private_key(args.config.private_key.clone())
                .with_rpc_url(args.config.rpc_url.clone())
                .with_boundless_market_address(args.config.boundless_market_address)
                .with_set_verifier_address(args.config.set_verifier_address)
                .with_order_stream_url(order_stream_url.clone())
                .with_timeout(args.config.tx_timeout)
                .build()
                .await?;

            let order = client.fetch_order(*request_id, *tx_hash, *request_digest).await?;
            tracing::debug!("Fetched order details: {:?}", order.request);

            let sig: Bytes = order.signature.as_bytes().into();
            order.request.verify_signature(
                &sig,
                args.config.boundless_market_address,
                boundless_market.get_chain_id().await?,
            )?;

            boundless_market.lock_request(&order.request, &sig, None).await?;
            tracing::info!("Successfully locked request 0x{:x}", request_id);
            Ok(())
        }
    }
}

/// Submit an offer and create a proof request
async fn submit_offer<P, S>(
    client: Client<P, S>,
    signer: &impl Signer,
    args: &SubmitOfferArgs,
) -> Result<()>
where
    P: Provider<Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // Read the YAML offer file
    let file = File::open(&args.yaml_offer)
        .context(format!("Failed to open offer file at {:?}", args.yaml_offer))?;
    let reader = BufReader::new(file);
    let mut offer: Offer =
        serde_yaml::from_reader(reader).context("failed to parse offer from YAML")?;

    // If set to 0, override the offer bidding_start field with the current timestamp + 30 seconds.
    if offer.biddingStart == 0 {
        // Adding a delay to bidding start lets provers see and evaluate the request
        // before the price starts to ramp up
        offer = Offer { biddingStart: now_timestamp() + 30, ..offer };
    }

    // Resolve the program and input from command line arguments.
    let program: Cow<'static, [u8]> = std::fs::read(&args.program)
        .context(format!("Failed to read program file at {:?}", args.program))?
        .into();

    // Process input based on provided arguments
    let input: Vec<u8> = match (&args.input.input, &args.input.input_file) {
        (Some(input), None) => input.as_bytes().to_vec(),
        (None, Some(input_file)) => std::fs::read(input_file)
            .context(format!("Failed to read input file at {:?}", input_file))?,
        _ => bail!("Exactly one of input or input-file args must be provided"),
    };

    // Prepare the input environment
    let input_env = InputBuilder::new();
    let encoded_input = if args.encode_input {
        input_env.write(&input)?.build_vec()?
    } else {
        input_env.write_slice(&input).build_vec()?
    };

    // Resolve the predicate from the command line arguments.
    let predicate: Predicate = match (&args.reqs.journal_digest, &args.reqs.journal_prefix) {
        (Some(digest), None) => Predicate {
            predicateType: PredicateType::DigestMatch,
            data: Bytes::copy_from_slice(Digest::from_hex(digest)?.as_bytes()),
        },
        (None, Some(prefix)) => Predicate {
            predicateType: PredicateType::PrefixMatch,
            data: Bytes::copy_from_slice(prefix.as_bytes()),
        },
        _ => bail!("Exactly one of journal-digest or journal-prefix args must be provided"),
    };

    // Configure callback if provided
    let callback = match (&args.reqs.callback_address, &args.reqs.callback_gas_limit) {
        (Some(addr), Some(gas_limit)) => Callback { addr: *addr, gasLimit: U96::from(*gas_limit) },
        _ => Callback::default(),
    };

    // Compute the image_id, then upload the program
    tracing::info!("Uploading image...");
    let elf_url = client.upload_image(&program).await?;
    let image_id = B256::from(<[u8; 32]>::from(risc0_zkvm::compute_image_id(&program)?));

    // Upload the input or prepare inline input
    tracing::info!("Preparing input...");
    let requirements_input = match args.inline_input {
        false => client.upload_input(&encoded_input).await?.into(),
        true => Input::inline(encoded_input),
    };

    // Set request id
    let id = match args.id {
        Some(id) => id,
        None => client.boundless_market.index_from_rand().await?,
    };

    // Construct the request from its individual parts.
    let mut request = ProofRequest::new(
        RequestId::new(client.caller(), id),
        Requirements { imageId: image_id, predicate, callback, selector: UNSPECIFIED_SELECTOR },
        elf_url,
        requirements_input,
        offer.clone(),
    );

    if args.reqs.proof_type == ProofType::Groth16 {
        request.requirements = request.requirements.with_groth16_proof();
    }

    tracing::debug!("Request details: {}", serde_json::to_string_pretty(&request)?);

    // Run preflight check if not disabled
    if !args.no_preflight {
        tracing::info!("Running request preflight check");
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;
        ensure!(
            request.requirements.predicate.eval(&journal),
            "Preflight failed: Predicate evaluation failed. Journal: {}, Predicate type: {:?}, Predicate data: {}",
            hex::encode(&journal),
            request.requirements.predicate.predicateType,
            hex::encode(&request.requirements.predicate.data)
        );
        tracing::info!("Preflight check passed");
    } else {
        tracing::warn!("Skipping preflight check");
    }

    // Submit the request
    let (request_id, expires_at) = if args.offchain {
        tracing::info!("Submitting request offchain");
        client.submit_request_offchain_with_signer(&request, signer).await?
    } else {
        tracing::info!("Submitting request onchain");
        client.submit_request_with_signer(&request, signer).await?
    };

    tracing::info!(
        "Submitted request 0x{request_id:x}, bidding starts at {}",
        convert_timestamp(offer.biddingStart)
    );

    // Wait for fulfillment if requested
    if args.wait {
        tracing::info!("Waiting for request fulfillment...");
        let (journal, seal) = client
            .boundless_market
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
            .await?;

        tracing::info!("Request fulfilled!");
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    }

    Ok(())
}

struct SubmitOptions {
    wait: bool,
    offchain: bool,
    preflight: bool,
    proof_type: ProofType,
    callback_address: Option<Address>,
    callback_gas_limit: Option<u64>,
}

/// Submit a proof request from a YAML file
async fn submit_request<P, S>(
    id: u32,
    request_path: impl AsRef<Path>,
    client: Client<P, S>,
    signer: &impl Signer,
    opts: SubmitOptions,
) -> Result<()>
where
    P: Provider<Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // Read the YAML request file
    let file = File::open(request_path.as_ref())
        .context(format!("Failed to open request file at {:?}", request_path.as_ref()))?;
    let reader = BufReader::new(file);
    let mut request_yaml: ProofRequest =
        serde_yaml::from_reader(reader).context("Failed to parse request from YAML")?;

    // If set to 0, override the offer bidding_start field with the current timestamp + 30s
    if request_yaml.offer.biddingStart == 0 {
        // Adding a delay to bidding start lets provers see and evaluate the request
        // before the price starts to ramp up
        request_yaml.offer = Offer { biddingStart: now_timestamp() + 30, ..request_yaml.offer };
    }

    // Create a new request with the provided ID
    let mut request = ProofRequest::new(
        RequestId::new(client.caller(), id),
        request_yaml.requirements.clone(),
        &request_yaml.imageUrl,
        request_yaml.input,
        request_yaml.offer,
    );

    // Use the original request id if it was set
    if request_yaml.id != U256::ZERO {
        request.id = request_yaml.id;
    }

    if opts.proof_type == ProofType::Groth16 {
        request.requirements = request.requirements.with_groth16_proof();
    }

    // Configure callback if provided
    request.requirements.callback = match (opts.callback_address, opts.callback_gas_limit) {
        (Some(addr), Some(gas_limit)) => Callback { addr, gasLimit: U96::from(gas_limit) },
        _ => Callback::default(),
    };

    // Run preflight check if enabled
    if opts.preflight {
        tracing::info!("Running request preflight check");
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;

        // Verify image ID if available
        if let Some(claim) = session_info.receipt_claim {
            ensure!(
                claim.pre.digest().as_bytes() == request_yaml.requirements.imageId.as_slice(),
                "Image ID mismatch: requirements ({}) do not match the given program ({})",
                hex::encode(request_yaml.requirements.imageId),
                hex::encode(claim.pre.digest().as_bytes())
            );
        } else {
            tracing::debug!("Cannot check image ID; session info doesn't have receipt claim");
        }

        // Verify predicate
        ensure!(
            request.requirements.predicate.eval(&journal),
            "Preflight failed: Predicate evaluation failed. Journal: {}, Predicate type: {:?}, Predicate data: {}",
            hex::encode(&journal),
            request.requirements.predicate.predicateType,
            hex::encode(&request.requirements.predicate.data)
        );

        tracing::info!("Preflight check passed");
    } else {
        tracing::warn!("Skipping preflight check");
    }

    // Submit the request
    let (request_id, expires_at) = if opts.offchain {
        tracing::info!("Submitting request offchain");
        client.submit_request_offchain_with_signer(&request, signer).await?
    } else {
        tracing::info!("Submitting request onchain");
        client.submit_request_with_signer(&request, signer).await?
    };

    tracing::info!(
        "Submitted request 0x{request_id:x}, bidding starts at {}",
        convert_timestamp(request.offer.biddingStart)
    );

    // Wait for fulfillment if requested
    if opts.wait {
        tracing::info!("Waiting for request fulfillment...");
        let (journal, seal) = client
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
            .await?;

        tracing::info!("Request fulfilled!");
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    }

    Ok(())
}

/// Execute a proof request using the RISC Zero zkVM executor
async fn execute(request: &ProofRequest) -> Result<SessionInfo> {
    tracing::info!("Fetching program from {}", request.imageUrl);
    let program = fetch_url(&request.imageUrl).await?;

    tracing::info!("Processing input");
    let input = match request.input.inputType {
        InputType::Inline => GuestEnv::decode(&request.input.data)?.stdin,
        InputType::Url => {
            let input_url =
                std::str::from_utf8(&request.input.data).context("Input URL is not valid UTF-8")?;
            tracing::info!("Fetching input from {}", input_url);
            GuestEnv::decode(&fetch_url(input_url).await?)?.stdin
        }
        _ => bail!("Unsupported input type"),
    };

    tracing::info!("Executing program in zkVM");
    r0vm_is_installed()?;
    let env = ExecutorEnv::builder().write_slice(&input).build()?;
    default_executor().execute(env, &program)
}

fn r0vm_is_installed() -> Result<()> {
    // Try to run the binary with the --version flag
    let result = std::process::Command::new("r0vm").arg("--version").output();

    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(anyhow!("r0vm is not installed or could not be executed. Please check instructions at https://dev.risczero.com/api/zkvm/install")),
    }
}

// Get current timestamp with appropriate error handling
fn now_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("Time went backwards").as_secs()
}

/// Handle config command
async fn handle_config_command(args: &MainArgs, show_sensitive: bool) -> Result<()> {
    tracing::info!("Displaying CLI configuration");
    println!("\n=== Boundless CLI Configuration ===\n");

    // Show configuration
    println!("RPC URL: {}", args.config.rpc_url);
    if show_sensitive {
        println!("Private Key: <available but cannot be displayed>");
    } else {
        println!("Private Key: <hidden> (use --show-sensitive to reveal)");
    }
    println!("Wallet Address: {}", args.config.private_key.address());
    println!("Boundless Market Address: {}", args.config.boundless_market_address);
    println!("Verifier Address: {}", args.config.verifier_address);
    println!("Set Verifier Address: {}", args.config.set_verifier_address);
    if let Some(timeout) = args.config.tx_timeout {
        println!("Transaction Timeout: {} seconds", timeout.as_secs());
    } else {
        println!("Transaction Timeout: <not set>");
    }
    println!("Log Level: {:?}", args.config.log_level);

    // Validate RPC connection
    println!("\n=== Environment Validation ===\n");
    print!("Testing RPC connection... ");
    let wallet = EthereumWallet::from(args.config.private_key.clone());
    let provider = ProviderBuilder::new().wallet(wallet).on_http(args.config.rpc_url.clone());

    let rpc_ok = match provider.get_chain_id().await {
        Ok(chain_id) => {
            println!("✅ Connected to chain ID: {}", chain_id);
            true
        }
        Err(e) => {
            println!("❌ Failed to connect: {}", e);
            false
        }
    };

    // Check market contract
    print!("Testing Boundless Market contract... ");
    let boundless_market = BoundlessMarketService::new(
        args.config.boundless_market_address,
        provider.clone(),
        args.config.private_key.address(),
    );

    let market_ok = match boundless_market.get_chain_id().await {
        Ok(_) => {
            println!("✅ Contract responds");
            true
        }
        Err(e) => {
            println!("❌ Contract error: {}", e);
            false
        }
    };

    // Check set verifier contract
    print!("Testing Set Verifier contract... ");
    let set_verifier = SetVerifierService::new(
        args.config.set_verifier_address,
        provider.clone(),
        args.config.private_key.address(),
    );

    let (image_id, _) = match set_verifier.image_info().await {
        Ok(image_info) => {
            println!("✅ Contract responds");
            image_info
        }
        Err(e) => {
            println!("❌ Contract error: {}", e);
            (B256::default(), String::default())
        }
    };

    let verifier_parameters =
        SetInclusionReceiptVerifierParameters { image_id: Digest::from_bytes(*image_id) };
    let selector: [u8; 4] = verifier_parameters.digest().as_bytes()[0..4].try_into()?;

    // Build the call data:
    // 1. Append the function selector for getVerifier(bytes4) ("3cadf449")
    // 2. Append the ABI encoding for the bytes4 parameter (padded to 32 bytes)
    let mut call_data = Vec::new();
    call_data.extend_from_slice(&hex::decode("3cadf449")?);
    call_data.extend_from_slice(&FixedBytes::from(selector).abi_encode());

    // Create a transaction request with the call data
    let tx = TransactionRequest {
        to: Some(TxKind::Call(args.config.verifier_address)),
        input: TransactionInput::new(call_data.into()),
        ..Default::default()
    };

    // Check verifier contract
    print!("Testing VerifierRouter contract... ");
    let verifier_ok = match provider.call(tx).await {
        Ok(_) => {
            println!("✅ Contract responds");
            true
        }
        Err(e) => {
            println!("❌ Contract error: {}", e);
            false
        }
    };

    println!(
        "\nEnvironment Setup: {}",
        if rpc_ok && market_ok && verifier_ok { "✅ Ready to use" } else { "❌ Issues detected" }
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use super::*;

    use alloy::{
        node_bindings::{Anvil, AnvilInstance},
        primitives::utils::format_units,
        providers::WalletProvider,
    };
    use boundless_market::{
        contracts::{hit_points::default_allowance, RequestStatus},
        selector::is_groth16_selector,
    };
    use boundless_market_test_utils::{
        create_test_ctx, deploy_mock_callback, get_mock_callback_count, TestCtx,
    };
    use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
    use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
    use guest_util::{ECHO_ID, ECHO_PATH};
    use order_stream::{run_from_parts, AppState, ConfigBuilder};
    use sqlx::PgPool;
    use tempfile::tempdir;
    use tokio::task::JoinHandle;
    use tracing_test::traced_test;

    // generate a test request
    fn generate_request(id: u32, addr: &Address) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(*addr, id),
            Requirements::new(
                Digest::from(ECHO_ID),
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            format!("file://{ECHO_PATH}"),
            Input::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                biddingStart: now_timestamp(),
                timeout: 420,
                lockTimeout: 420,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
            },
        )
    }

    enum AccountOwner {
        Customer,
        Prover,
    }

    /// Test setup helper that creates common test infrastructure
    async fn setup_test_env(
        owner: AccountOwner,
    ) -> (TestCtx<impl Provider + WalletProvider + Clone + 'static>, AnvilInstance, GlobalConfig)
    {
        let anvil = Anvil::new().spawn();

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
        .await
        .unwrap();

        let private_key = match owner {
            AccountOwner::Customer => {
                ctx.prover_market
                    .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
                    .await
                    .unwrap();
                ctx.customer_signer.clone()
            }
            AccountOwner::Prover => ctx.prover_signer.clone(),
        };

        let config = GlobalConfig {
            rpc_url: anvil.endpoint_url(),
            private_key,
            boundless_market_address: ctx.boundless_market_address,
            verifier_address: ctx.verifier_address,
            set_verifier_address: ctx.set_verifier_address,
            tx_timeout: None,
            log_level: LevelFilter::INFO,
        };

        (ctx, anvil, config)
    }

    async fn setup_test_env_with_order_stream(
        owner: AccountOwner,
        pool: PgPool,
    ) -> (
        TestCtx<impl Provider + WalletProvider + Clone + 'static>,
        AnvilInstance,
        GlobalConfig,
        Url,
        JoinHandle<()>,
    ) {
        let (ctx, anvil, global_config) = setup_test_env(owner).await;

        // Create listener first
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .await
            .unwrap();
        let order_stream_address = listener.local_addr().unwrap();
        let order_stream_url = Url::parse(&format!("http://{}", order_stream_address)).unwrap();
        let domain = order_stream_address.to_string();

        let config = ConfigBuilder::default()
            .rpc_url(anvil.endpoint_url())
            .market_address(ctx.boundless_market_address)
            .domain(domain)
            .build()
            .unwrap();

        // Start order stream server
        let order_stream = AppState::new(&config, Some(pool)).await.unwrap();
        let order_stream_clone = order_stream.clone();
        let order_stream_handle = tokio::spawn(async move {
            run_from_parts(order_stream_clone, listener).await.unwrap();
        });

        (ctx, anvil, global_config, order_stream_url, order_stream_handle)
    }

    #[tokio::test]
    #[traced_test]
    async fn test_deposit_withdraw() {
        let (ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let mut args = MainArgs {
            config,
            command: Command::Account(Box::new(AccountCommands::Deposit {
                amount: default_allowance(),
            })),
        };

        run(&args).await.unwrap();
        assert!(logs_contain(&format!(
            "Depositing {} ETH",
            format_units(default_allowance(), "ether").unwrap()
        )));
        assert!(logs_contain(&format!(
            "Successfully deposited {} ETH",
            format_units(default_allowance(), "ether").unwrap()
        )));

        let balance = ctx.prover_market.balance_of(ctx.customer_signer.address()).await.unwrap();
        assert_eq!(balance, default_allowance());

        args.command = Command::Account(Box::new(AccountCommands::Balance {
            address: Some(ctx.customer_signer.address()),
        }));
        run(&args).await.unwrap();
        assert!(logs_contain(&format!(
            "Checking balance for address {}",
            ctx.customer_signer.address()
        )));
        assert!(logs_contain(&format!(
            "Balance for address {}: {} ETH",
            ctx.customer_signer.address(),
            format_units(default_allowance(), "ether").unwrap()
        )));

        args.command =
            Command::Account(Box::new(AccountCommands::Withdraw { amount: default_allowance() }));

        run(&args).await.unwrap();
        assert!(logs_contain(&format!(
            "Withdrawing {} ETH",
            format_units(default_allowance(), "ether").unwrap()
        )));
        assert!(logs_contain(&format!(
            "Successfully withdrew {} ETH",
            format_units(default_allowance(), "ether").unwrap()
        )));

        let balance = ctx.prover_market.balance_of(ctx.customer_signer.address()).await.unwrap();
        assert_eq!(balance, U256::from(0));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_fail_deposit_withdraw() {
        let (_ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let amount = U256::from(10000000000000000000000_u128);
        let mut args = MainArgs {
            config,
            command: Command::Account(Box::new(AccountCommands::Deposit { amount })),
        };

        let err = run(&args).await.unwrap_err();
        assert!(err.to_string().contains("Insufficient funds"));

        args.command = Command::Account(Box::new(AccountCommands::Withdraw { amount }));

        let err = run(&args).await.unwrap_err();
        assert!(err.to_string().contains("InsufficientBalance"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_deposit_withdraw_stake() {
        let (ctx, _anvil, config) = setup_test_env(AccountOwner::Prover).await;

        let mut args = MainArgs {
            config,
            command: Command::Account(Box::new(AccountCommands::DepositStake {
                amount: default_allowance(),
            })),
        };

        run(&args).await.unwrap();
        assert!(logs_contain(&format!(
            "Depositing {} HP as stake",
            format_units(default_allowance(), "ether").unwrap()
        )));
        assert!(logs_contain(&format!(
            "Successfully deposited {} HP as stake",
            format_units(default_allowance(), "ether").unwrap()
        )));

        let balance =
            ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap();
        assert_eq!(balance, default_allowance());

        args.command = Command::Account(Box::new(AccountCommands::StakeBalance {
            address: Some(ctx.prover_signer.address()),
        }));
        run(&args).await.unwrap();
        assert!(logs_contain(&format!(
            "Checking stake balance for address {}",
            ctx.prover_signer.address()
        )));
        assert!(logs_contain(&format!(
            "Stake balance for address {}: {} HP",
            ctx.prover_signer.address(),
            format_units(default_allowance(), "ether").unwrap()
        )));

        args.command = Command::Account(Box::new(AccountCommands::WithdrawStake {
            amount: default_allowance(),
        }));

        run(&args).await.unwrap();
        assert!(logs_contain(&format!(
            "Withdrawing {} HP from stake",
            format_units(default_allowance(), "ether").unwrap()
        )));
        assert!(logs_contain(&format!(
            "Successfully withdrew {} HP from stake",
            format_units(default_allowance(), "ether").unwrap()
        )));

        let balance =
            ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap();
        assert_eq!(balance, U256::from(0));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_fail_deposit_withdraw_stake() {
        let (ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let mut args = MainArgs {
            config,
            command: Command::Account(Box::new(AccountCommands::DepositStake {
                amount: default_allowance(),
            })),
        };

        let err = run(&args).await.unwrap_err();
        assert!(err.to_string().contains(&format!(
            "Failed to deposit stake: Ensure your address ({}) has funds on the HP contract",
            ctx.customer_signer.address()
        )));

        args.command = Command::Account(Box::new(AccountCommands::WithdrawStake {
            amount: default_allowance(),
        }));

        let err = run(&args).await.unwrap_err();
        assert!(err.to_string().contains("InsufficientBalance"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_submit_request_onchain() {
        let (_ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        // Submit a request onchain
        let args = MainArgs {
            config,
            command: Command::Request(Box::new(RequestCommands::Submit {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_request: "../../request.yaml".to_string().into(),
                id: None,
                wait: false,
                offchain: false,
                order_stream_url: None,
                no_preflight: false,
                proof_type: ProofType::Any,
                callback_address: None,
                callback_gas_limit: None,
            })),
        };
        run(&args).await.unwrap();
        assert!(logs_contain("Submitting request onchain"));
        assert!(logs_contain("Submitted request"));
    }

    #[sqlx::test]
    #[traced_test]
    async fn test_submit_request_offchain(pool: PgPool) {
        let (ctx, _anvil, config, order_stream_url, order_stream_handle) =
            setup_test_env_with_order_stream(AccountOwner::Customer, pool).await;

        // Deposit funds into the market
        ctx.customer_market.deposit(parse_ether("1").unwrap()).await.unwrap();

        // Submit a request offchain
        let args = MainArgs {
            config,
            command: Command::Request(Box::new(RequestCommands::Submit {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_request: "../../request.yaml".to_string().into(),
                id: None,
                wait: false,
                offchain: true,
                order_stream_url: Some(order_stream_url),
                no_preflight: true,
                proof_type: ProofType::Any,
                callback_address: None,
                callback_gas_limit: None,
            })),
        };
        run(&args).await.unwrap();
        assert!(logs_contain("Submitting request offchain"));
        assert!(logs_contain("Submitted request"));

        // Clean up
        order_stream_handle.abort();
    }

    #[tokio::test]
    #[traced_test]
    async fn test_submit_offer_onchain() {
        let (_ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        // Submit a request onchain
        let args = MainArgs {
            config,
            command: Command::Request(Box::new(RequestCommands::SubmitOffer(SubmitOfferArgs {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_offer: "../../offer.yaml".to_string().into(),
                id: None,
                wait: false,
                offchain: false,
                order_stream_url: None,
                no_preflight: true,
                encode_input: false,
                inline_input: true,
                input: SubmitOfferInput {
                    input: Some(hex::encode([0x41, 0x41, 0x41, 0x41])),
                    input_file: None,
                },
                program: PathBuf::from(ECHO_PATH),
                reqs: SubmitOfferRequirements {
                    journal_digest: None,
                    journal_prefix: Some(String::default()),
                    callback_address: None,
                    callback_gas_limit: None,
                    proof_type: ProofType::Any,
                },
            }))),
        };
        run(&args).await.unwrap();
        assert!(logs_contain("Submitting request onchain"));
        assert!(logs_contain("Submitted request"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_request_status_onchain() {
        let (ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
        );

        // Deposit funds into the market
        ctx.customer_market.deposit(parse_ether("1").unwrap()).await.unwrap();

        // Submit the request onchain
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // Create a new args struct to test the Status command
        let status_args = MainArgs {
            config,
            command: Command::Request(Box::new(RequestCommands::Status {
                request_id: request.id,
                expires_at: None,
            })),
        };

        run(&status_args).await.unwrap();

        assert!(logs_contain(&format!("Request 0x{:x} status: Unknown", request.id)));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_slash() {
        let (ctx, anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let mut request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
        );
        request.offer.timeout = 50;
        request.offer.lockTimeout = 50;

        // Deposit funds into the market
        ctx.customer_market.deposit(parse_ether("1").unwrap()).await.unwrap();

        // Submit the request onchain
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        let client_sig = request
            .sign_request(&ctx.customer_signer, ctx.boundless_market_address, anvil.chain_id())
            .await
            .unwrap();

        // Lock the request
        ctx.prover_market
            .lock_request(&request, &Bytes::copy_from_slice(&client_sig.as_bytes()), None)
            .await
            .unwrap();

        // Create a new args struct to test the Status command
        let status_args = MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::Status {
                request_id: request.id,
                expires_at: None,
            })),
        };
        run(&status_args).await.unwrap();
        assert!(logs_contain(&format!("Request 0x{:x} status: Locked", request.id)));

        loop {
            // Wait for the timeout to expire
            tokio::time::sleep(Duration::from_secs(1)).await;
            let status = ctx
                .customer_market
                .get_status(request.id, Some(request.expires_at()))
                .await
                .unwrap();
            if status == RequestStatus::Expired {
                break;
            }
        }

        // test the Slash command
        run(&MainArgs {
            config,
            command: Command::Ops(Box::new(OpsCommands::Slash { request_id: request.id })),
        })
        .await
        .unwrap();
        assert!(logs_contain(&format!(
            "Successfully slashed prover for request 0x{:x}",
            request.id
        )));
    }

    #[tokio::test]
    #[traced_test]
    #[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
    async fn test_proving_onchain() {
        let (ctx, anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
        );

        let request_id = request.id;

        // Dump the request to a tmp file
        let request_path = tempdir().unwrap().into_path().join("request.yaml");
        let request_file = File::create(&request_path).unwrap();
        serde_yaml::to_writer(request_file, &request).unwrap();

        // send the request onchain
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::Submit {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_request: request_path,
                id: None,
                wait: false,
                offchain: false,
                order_stream_url: None,
                no_preflight: true,
                proof_type: ProofType::Any,
                callback_address: None,
                callback_gas_limit: None,
            })),
        })
        .await
        .unwrap();

        // test the Execute command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Proving(Box::new(ProvingCommands::Execute {
                request_path: None,
                request_id: Some(request_id),
                request_digest: None,
                tx_hash: None,
                order_stream_url: None,
            })),
        })
        .await
        .unwrap();

        assert!(logs_contain(&format!("Successfully executed request 0x{:x}", request.id)));

        let prover_config = GlobalConfig {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.prover_signer.clone(),
            boundless_market_address: ctx.boundless_market_address,
            verifier_address: ctx.verifier_address,
            set_verifier_address: ctx.set_verifier_address,
            tx_timeout: None,
            log_level: LevelFilter::INFO,
        };

        // test the Lock command
        run(&MainArgs {
            config: prover_config,
            command: Command::Proving(Box::new(ProvingCommands::Lock {
                request_id,
                request_digest: None,
                tx_hash: None,
                order_stream_url: None,
            })),
        })
        .await
        .unwrap();
        assert!(logs_contain(&format!("Successfully locked request 0x{:x}", request.id)));

        // test the Status command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::Status {
                request_id,
                expires_at: None,
            })),
        })
        .await
        .unwrap();
        assert!(logs_contain(&format!("Request 0x{:x} status: Locked", request.id)));

        // test the Fulfill command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Proving(Box::new(ProvingCommands::Fulfill {
                request_ids: vec![request_id],
                request_digests: None,
                tx_hashes: None,
                order_stream_url: None,
            })),
        })
        .await
        .unwrap();

        assert!(logs_contain(&format!("Successfully fulfilled requests 0x{:x}", request.id)));

        // test the Status command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::Status {
                request_id,
                expires_at: None,
            })),
        })
        .await
        .unwrap();
        assert!(logs_contain(&format!("Request 0x{:x} status: Fulfilled", request.id)));

        // test the GetProof command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::GetProof { request_id })),
        })
        .await
        .unwrap();
        assert!(logs_contain(&format!(
            "Successfully retrieved proof for request 0x{:x}",
            request.id
        )));

        // test the Verify command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::VerifyProof {
                request_id,
                image_id: request.requirements.imageId,
            })),
        })
        .await
        .unwrap();
        assert!(logs_contain(&format!(
            "Successfully verified proof for request 0x{:x}",
            request.id
        )));
    }

    #[tokio::test]
    #[traced_test]
    #[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
    async fn test_proving_multiple_requests() {
        let (ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let mut request_ids = Vec::new();
        for _ in 0..3 {
            let request = generate_request(
                ctx.customer_market.index_from_nonce().await.unwrap(),
                &ctx.customer_signer.address(),
            );

            ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();
            request_ids.push(request.id);
        }

        // test the Fulfill command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Proving(Box::new(ProvingCommands::Fulfill {
                request_ids: request_ids.clone(),
                request_digests: None,
                tx_hashes: None,
                order_stream_url: None,
            })),
        })
        .await
        .unwrap();

        let request_ids_str =
            request_ids.iter().map(|id| format!("0x{:x}", id)).collect::<Vec<_>>().join(", ");
        assert!(logs_contain(&format!("Successfully fulfilled requests {request_ids_str}")));

        for request_id in request_ids {
            // test the Status command
            run(&MainArgs {
                config: config.clone(),
                command: Command::Request(Box::new(RequestCommands::Status {
                    request_id,
                    expires_at: None,
                })),
            })
            .await
            .unwrap();
            assert!(logs_contain(&format!("Request 0x{:x} status: Fulfilled", request_id)));
        }
    }

    #[tokio::test]
    #[traced_test]
    #[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
    async fn test_callback() {
        let (ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
        );

        // Dump the request to a tmp file
        let request_path = tempdir().unwrap().into_path().join("request.yaml");
        let request_file = File::create(&request_path).unwrap();
        serde_yaml::to_writer(request_file, &request).unwrap();

        // Deploy MockCallback contract
        let callback_address = deploy_mock_callback(
            &ctx.prover_provider,
            ctx.verifier_address,
            ctx.boundless_market_address,
            ECHO_ID,
            U256::ZERO,
        )
        .await
        .unwrap();

        // send the request onchain
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::Submit {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_request: request_path,
                id: None,
                wait: false,
                offchain: false,
                order_stream_url: None,
                no_preflight: true,
                proof_type: ProofType::Any,
                callback_address: Some(callback_address),
                callback_gas_limit: Some(100000),
            })),
        })
        .await
        .unwrap();

        // fulfill the request
        run(&MainArgs {
            config,
            command: Command::Proving(Box::new(ProvingCommands::Fulfill {
                request_ids: vec![request.id],
                request_digests: None,
                tx_hashes: None,
                order_stream_url: None,
            })),
        })
        .await
        .unwrap();

        // check the callback was called
        let count =
            get_mock_callback_count(&ctx.customer_provider, callback_address).await.unwrap();
        assert!(count == U256::from(1));
    }

    #[tokio::test]
    #[traced_test]
    #[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
    async fn test_selector() {
        let (ctx, _anvil, config) = setup_test_env(AccountOwner::Customer).await;

        let request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
        );

        // Dump the request to a tmp file
        let request_path = tempdir().unwrap().into_path().join("request.yaml");
        let request_file = File::create(&request_path).unwrap();
        serde_yaml::to_writer(request_file, &request).unwrap();

        // send the request onchain
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::Submit {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_request: request_path,
                id: None,
                wait: false,
                offchain: false,
                order_stream_url: None,
                no_preflight: true,
                proof_type: ProofType::Groth16,
                callback_address: None,
                callback_gas_limit: None,
            })),
        })
        .await
        .unwrap();

        // fulfill the request
        run(&MainArgs {
            config,
            command: Command::Proving(Box::new(ProvingCommands::Fulfill {
                request_ids: vec![request.id],
                request_digests: None,
                tx_hashes: None,
                order_stream_url: None,
            })),
        })
        .await
        .unwrap();

        // check the seal is aggregated
        let (_journal, seal) =
            ctx.customer_market.get_request_fulfillment(request.id).await.unwrap();
        let selector: FixedBytes<4> = seal[0..4].try_into().unwrap();
        assert!(is_groth16_selector(selector))
    }

    #[sqlx::test]
    #[traced_test]
    #[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
    async fn test_proving_offchain(pool: PgPool) {
        let (ctx, anvil, config, order_stream_url, order_stream_handle) =
            setup_test_env_with_order_stream(AccountOwner::Customer, pool).await;

        // Deposit funds into the market
        ctx.customer_market.deposit(parse_ether("1").unwrap()).await.unwrap();

        let request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
        );

        let request_id = request.id;

        // Dump the request to a tmp file
        let request_path = tempdir().unwrap().into_path().join("request.yaml");
        let request_file = File::create(&request_path).unwrap();
        serde_yaml::to_writer(request_file, &request).unwrap();

        // send the request offchain
        run(&MainArgs {
            config: config.clone(),
            command: Command::Request(Box::new(RequestCommands::Submit {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_request: request_path,
                id: None,
                wait: false,
                offchain: true,
                order_stream_url: Some(order_stream_url.clone()),
                no_preflight: true,
                proof_type: ProofType::Any,
                callback_address: None,
                callback_gas_limit: None,
            })),
        })
        .await
        .unwrap();

        // test the Execute command
        run(&MainArgs {
            config: config.clone(),
            command: Command::Proving(Box::new(ProvingCommands::Execute {
                request_path: None,
                request_id: Some(request_id),
                request_digest: None,
                tx_hash: None,
                order_stream_url: Some(order_stream_url.clone()),
            })),
        })
        .await
        .unwrap();

        assert!(logs_contain(&format!("Successfully executed request 0x{:x}", request.id)));

        let prover_config = GlobalConfig {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.prover_signer.clone(),
            boundless_market_address: ctx.boundless_market_address,
            verifier_address: ctx.verifier_address,
            set_verifier_address: ctx.set_verifier_address,
            tx_timeout: None,
            log_level: LevelFilter::INFO,
        };

        // test the Lock command
        run(&MainArgs {
            config: prover_config,
            command: Command::Proving(Box::new(ProvingCommands::Lock {
                request_id,
                request_digest: None,
                tx_hash: None,
                order_stream_url: Some(order_stream_url.clone()),
            })),
        })
        .await
        .unwrap();
        assert!(logs_contain(&format!("Successfully locked request 0x{:x}", request.id)));

        // test the Fulfill command
        run(&MainArgs {
            config,
            command: Command::Proving(Box::new(ProvingCommands::Fulfill {
                request_ids: vec![request_id],
                request_digests: None,
                tx_hashes: None,
                order_stream_url: Some(order_stream_url),
            })),
        })
        .await
        .unwrap();

        assert!(logs_contain(&format!("Successfully fulfilled requests 0x{:x}", request.id)));

        // Clean up
        order_stream_handle.abort();
    }
}
