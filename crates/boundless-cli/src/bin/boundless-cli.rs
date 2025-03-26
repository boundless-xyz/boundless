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
        Address, Bytes, B256, U256,
    },
    providers::{network::EthereumWallet, Provider, ProviderBuilder},
    signers::{local::PrivateKeySigner, Signer},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use boundless_cli::{fetch_url, DefaultProver, OrderFulfilled};
use clap::{Args, Parser, Subcommand};
use hex::FromHex;
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
        PredicateType, ProofRequest, Requirements, UNSPECIFIED_SELECTOR,
    },
    input::{GuestEnv, InputBuilder},
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
        #[clap(long, env, default_value = "https://order-stream.beboundless.xyz")]
        order_stream_url: Option<Url>,

        /// Skip preflight check (not recommended)
        #[clap(long, default_value = "false")]
        no_preflight: bool,

        /// Request an unaggregated proof (i.e., a Groth16).
        #[clap(long)]
        unaggregated: bool,
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
        #[arg(long, conflicts_with_all = ["request_path", "tx_hash"])]
        order_stream_url: Option<Url>,
    },

    /// Fulfill a proof request using the RISC Zero zkVM default prover
    Fulfill {
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
        #[arg(long, conflicts_with_all = ["tx_hash"])]
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
    #[clap(long, env, default_value = "https://order-stream.beboundless.xyz")]
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

    /// Elf file to use as the guest image, given as a path
    #[clap(long)]
    elf: PathBuf,

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
    #[clap(long, requires = "callback_addr")]
    callback_gas_limit: Option<u64>,
    /// Request an unaggregated proof (i.e., a Groth16).
    #[clap(long)]
    unaggregated: bool,
}

/// Common configuration options for all commands
#[derive(Args, Debug, Clone)]
struct GlobalConfig {
    /// URL of the Ethereum RPC endpoint
    #[clap(short, long, env, default_value = "http://localhost:8545")]
    rpc_url: Url,

    /// Private key of the wallet
    #[clap(long, env)]
    private_key: PrivateKeySigner,

    /// Address of the market contract
    #[clap(short, long, env)]
    boundless_market_address: Address,

    /// Address of the SetVerifier contract
    #[clap(short, long, env)]
    set_verifier_address: Address,

    /// Tx timeout in seconds
    #[clap(long, env, value_parser = |arg: &str| -> Result<Duration, ParseIntError> {Ok(Duration::from_secs(arg.parse()?))})]
    tx_timeout: Option<Duration>,

    /// Log level (error, warn, info, debug, trace)
    #[clap(long, env, default_value = "info")]
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
    // Load environment variables from .env file if present
    match dotenvy::dotenv() {
        Ok(path) => println!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => (),
        Err(e) => {
            eprintln!("Warning: failed to load .env file: {}", e);
        }
    }

    // Parse command line arguments
    let args = MainArgs::parse();

    // Configure tracing with appropriate log level
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(args.config.log_level.into())
                .from_env_lossy(),
        )
        .init();

    // Execute the command and handle any errors
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
            boundless_market.slash(*request_id).await?;
            tracing::info!("Slashed prover for request ID 0x{:x}", request_id);
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
            boundless_market.deposit(*amount).await?;
            tracing::info!("Deposited: {}", format_ether(*amount));
            Ok(())
        }
        AccountCommands::Withdraw { amount } => {
            boundless_market.withdraw(*amount).await?;
            tracing::info!("Withdrew: {}", format_ether(*amount));
            Ok(())
        }
        AccountCommands::Balance { address } => {
            let addr = address.unwrap_or(boundless_market.caller());
            let balance = boundless_market.balance_of(addr).await?;
            tracing::info!("Balance of {addr}: {}", format_ether(balance));
            Ok(())
        }
        AccountCommands::DepositStake { amount } => {
            boundless_market.deposit_stake_with_permit(*amount, &private_key).await?;
            tracing::info!("Deposited stake: {}", amount);
            Ok(())
        }
        AccountCommands::WithdrawStake { amount } => {
            boundless_market.withdraw_stake(*amount).await?;
            tracing::info!("Withdrew stake: {}", amount);
            Ok(())
        }
        AccountCommands::StakeBalance { address } => {
            let addr = address.unwrap_or(boundless_market.caller());
            let balance = boundless_market.balance_of_stake(addr).await?;
            tracing::info!("Stake balance of {addr}: {}", balance);
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
            let order_stream_url = offer_args
                .offchain
                .then_some(
                    offer_args
                        .order_stream_url
                        .clone()
                        .ok_or(anyhow!("offchain flag set, but order stream URL not provided")),
                )
                .transpose()?;

            let client = ClientBuilder::default()
                .with_private_key(args.config.private_key.clone())
                .with_rpc_url(args.config.rpc_url.clone())
                .with_boundless_market_address(args.config.boundless_market_address)
                .with_set_verifier_address(args.config.set_verifier_address)
                .with_storage_provider_config(offer_args.storage_config.clone())
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
            unaggregated,
        } => {
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

            let client = ClientBuilder::default()
                .with_private_key(args.config.private_key.clone())
                .with_rpc_url(args.config.rpc_url.clone())
                .with_boundless_market_address(args.config.boundless_market_address)
                .with_set_verifier_address(args.config.set_verifier_address)
                .with_order_stream_url(order_stream_url.clone())
                .with_storage_provider_config(storage_config.clone())
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
                    unaggredated: *unaggregated,
                },
            )
            .await
        }
        RequestCommands::Status { request_id, expires_at } => {
            let status = boundless_market.get_status(*request_id, *expires_at).await?;
            tracing::info!("RequestStatus {{ id: 0x{:x}, status: {:?}}}", request_id, status);
            Ok(())
        }
        RequestCommands::GetProof { request_id } => {
            let (journal, seal) = boundless_market.get_request_fulfillment(*request_id).await?;
            tracing::info!(
                "Journal: {} - Seal: {}",
                serde_json::to_string_pretty(&journal)?,
                serde_json::to_string_pretty(&seal)?
            );
            Ok(())
        }
        RequestCommands::VerifyProof { request_id, image_id } => {
            let (journal, seal) = boundless_market.get_request_fulfillment(*request_id).await?;
            let journal_digest = <[u8; 32]>::from(Journal::new(journal.to_vec()).digest()).into();
            let set_verifier =
                IRiscZeroVerifier::new(args.config.set_verifier_address, provider.clone());

            set_verifier
                .verify(seal, *image_id, journal_digest)
                .call()
                .await
                .map_err(|_| anyhow::anyhow!("Verification failed"))?;

            tracing::info!("Proof for request id 0x{request_id:x} verified successfully.");
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
            let request: ProofRequest = if let Some(file_path) = request_path {
                let file = File::open(file_path).context("failed to open request file")?;
                let reader = BufReader::new(file);
                serde_yaml::from_reader(reader).context("failed to parse request from YAML")?
            } else if let Some(request_id) = request_id {
                let client = ClientBuilder::default()
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
                bail!("Predicate evaluation failed");
            }

            tracing::info!("Execution succeeded.");
            tracing::debug!("Journal: {}", serde_json::to_string_pretty(&journal)?);
            Ok(())
        }
        ProvingCommands::Fulfill { request_id, request_digest, tx_hash, order_stream_url } => {
            let (_, market_url) = boundless_market.image_info().await?;
            tracing::debug!("Fetching Assessor ELF from {}", market_url);
            let assessor_elf = fetch_url(&market_url).await?;
            let domain = boundless_market.eip712_domain().await?;

            let mut set_verifier =
                SetVerifierService::new(args.config.set_verifier_address, provider.clone(), caller);

            if let Some(tx_timeout) = args.config.tx_timeout {
                set_verifier = set_verifier.with_timeout(tx_timeout);
            }

            let (_, set_builder_url) = set_verifier.image_info().await?;
            tracing::debug!("Fetching SetBuilder ELF from {}", set_builder_url);
            let set_builder_elf = fetch_url(&set_builder_url).await?;

            let prover = DefaultProver::new(set_builder_elf, assessor_elf, caller, domain)?;

            let client = ClientBuilder::default()
                .with_private_key(args.config.private_key.clone())
                .with_rpc_url(args.config.rpc_url.clone())
                .with_boundless_market_address(args.config.boundless_market_address)
                .with_set_verifier_address(args.config.set_verifier_address)
                .with_order_stream_url(order_stream_url.clone())
                .with_timeout(args.config.tx_timeout)
                .build()
                .await?;

            let order = client.fetch_order(*request_id, *tx_hash, *request_digest).await?;
            tracing::debug!("Fulfilling request {:?}", order.request);

            let sig: Bytes = order.signature.as_bytes().into();
            order.request.verify_signature(
                &sig,
                args.config.boundless_market_address,
                boundless_market.get_chain_id().await?,
            )?;

            let (fill, root_receipt, assessor_receipt) = prover.fulfill(order.clone()).await?;
            let order_fulfilled = OrderFulfilled::new(fill, root_receipt, assessor_receipt)?;
            set_verifier.submit_merkle_root(order_fulfilled.root, order_fulfilled.seal).await?;

            // If the request is not locked in, we need to "price" which checks the requirements
            // and assigns a price. Otherwise, we don't. This vec will be a singleton if not locked
            // and empty if the request is locked.
            let requests_to_price: Vec<ProofRequest> =
                (!boundless_market.is_locked(*request_id).await?)
                    .then_some(order.request)
                    .into_iter()
                    .collect();

            match boundless_market
                .price_and_fulfill_batch(
                    requests_to_price,
                    vec![sig],
                    order_fulfilled.fills,
                    order_fulfilled.assessorReceipt,
                    None,
                )
                .await
            {
                Ok(_) => {
                    tracing::info!("Fulfilled request 0x{:x}", request_id);
                    Ok(())
                }
                Err(e) => {
                    tracing::error!("Failed to fulfill request 0x{:x}: {}", request_id, e);
                    bail!("Failed to fulfill request: {}", e)
                }
            }
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

    // Resolve the ELF and input from command line arguments.
    let elf: Cow<'static, [u8]> = std::fs::read(&args.elf)
        .context(format!("Failed to read ELF file at {:?}", args.elf))?
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

    // Compute the image_id, then upload the ELF
    tracing::info!("Uploading image...");
    let elf_url = client.upload_image(&elf).await?;
    let image_id = B256::from(<[u8; 32]>::from(risc0_zkvm::compute_image_id(&elf)?));

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

    // Construct the request from its individual parts
    let mut request = ProofRequest::new(
        id,
        &client.caller(),
        Requirements { imageId: image_id, predicate, callback, selector: UNSPECIFIED_SELECTOR },
        elf_url,
        requirements_input,
        offer.clone(),
    );

    if args.reqs.unaggregated {
        request.requirements = request.requirements.with_unaggregated_proof();
    }

    tracing::debug!("Request details: {}", serde_json::to_string_pretty(&request)?);

    // Run preflight check if not disabled
    if !args.no_preflight {
        tracing::info!("Running request preflight check");
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;
        ensure!(
            request.requirements.predicate.eval(&journal),
            "Preflight failed: Predicate evaluation failed; journal does not match requirements"
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
        tracing::info!("Submitting request on-chain");
        client.submit_request_with_signer(&request, signer).await?
    };

    tracing::info!(
        "Submitted request ID 0x{request_id:x}, bidding starts at timestamp {}",
        offer.biddingStart
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
    unaggredated: bool,
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
        id,
        &client.caller(),
        request_yaml.requirements.clone(),
        &request_yaml.imageUrl,
        request_yaml.input,
        request_yaml.offer,
    );

    // Use the original request id if it was set
    if request_yaml.id != U256::ZERO {
        request.id = request_yaml.id;
    }

    if opts.unaggredated {
        request.requirements = request.requirements.with_unaggregated_proof();
    }

    // Run preflight check if enabled
    if opts.preflight {
        tracing::info!("Running request preflight check");
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;

        // Verify image ID if available
        if let Some(claim) = session_info.receipt_claim {
            ensure!(
                claim.pre.digest().as_bytes() == request_yaml.requirements.imageId.as_slice(),
                "Image ID mismatch: requirements ({}) do not match the given ELF ({})",
                hex::encode(request_yaml.requirements.imageId),
                hex::encode(claim.pre.digest().as_bytes())
            );
        } else {
            tracing::debug!("Cannot check image ID; session info doesn't have receipt claim");
        }

        // Verify predicate
        ensure!(
            request.requirements.predicate.eval(&journal),
            "Preflight failed: Predicate evaluation failed; journal does not match requirements"
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
        tracing::info!("Submitting request on-chain");
        client.submit_request_with_signer(&request, signer).await?
    };

    tracing::info!(
        "Submitted request ID 0x{request_id:x}, bidding starts at timestamp {}",
        request.offer.biddingStart
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
    tracing::info!("Fetching ELF from {}", request.imageUrl);
    let elf = fetch_url(&request.imageUrl).await?;

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
    let env = ExecutorEnv::builder().write_slice(&input).build()?;
    default_executor().execute(env, &elf)
}

// Get current timestamp with appropriate error handling
fn now_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("Time went backwards").as_secs()
}
#[cfg(test)]
mod tests {
    use super::*;

    use alloy::node_bindings::Anvil;
    use boundless_market::contracts::{hit_points::default_allowance, test_utils::create_test_ctx};
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_set_builder::SET_BUILDER_ID;
    use guest_util::{ECHO_ID, ECHO_PATH};
    use tokio::time::timeout;
    use tracing_test::traced_test;

    fn generate_request(
        id: u32,
        addr: &Address,
        unaggregated: bool,
        callback: Option<Callback>,
    ) -> ProofRequest {
        let mut requirements = Requirements::new(
            Digest::from(ECHO_ID),
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        );
        if unaggregated {
            requirements = requirements.with_unaggregated_proof();
        }
        if let Some(callback) = callback {
            requirements = requirements.with_callback(callback);
        }
        ProofRequest::new(
            id,
            addr,
            requirements,
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

    #[tokio::test]
    #[traced_test]
    async fn test_deposit_withdraw() {
        // Setup anvil
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();

        let config = GlobalConfig {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.prover_signer.clone(),
            boundless_market_address: ctx.boundless_market_address,
            set_verifier_address: ctx.set_verifier_address,
            tx_timeout: None,
            log_level: LevelFilter::INFO,
        };

        let mut args = MainArgs {
            config: config.clone(),
            command: Command::Account(Box::new(AccountCommands::Deposit {
                amount: default_allowance(),
            })),
        };

        run(&args).await.unwrap();

        let balance = ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap();
        assert_eq!(balance, default_allowance());

        args.command =
            Command::Account(Box::new(AccountCommands::Withdraw { amount: default_allowance() }));

        run(&args).await.unwrap();

        let balance = ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap();
        assert_eq!(balance, U256::from(0));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_submit_request() {
        // Setup anvil
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();
        ctx.prover_market
            .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();

        let config = GlobalConfig {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer.clone(),
            boundless_market_address: ctx.boundless_market_address,
            set_verifier_address: ctx.set_verifier_address,
            tx_timeout: None,
            log_level: LevelFilter::INFO,
        };

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
                unaggregated: false,
            })),
        };

        let result = timeout(Duration::from_secs(60), run(&args)).await;

        match result {
            Ok(run_result) => match run_result {
                Ok(_) => {}
                Err(e) => {
                    panic!("`run` returned an error: {:?}", e);
                }
            },
            Err(_) => {
                panic!("Test timed out after 1 minute");
            }
        };
    }

    #[tokio::test]
    #[traced_test]
    async fn test_request_status() {
        // Setup anvil
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();
        ctx.prover_market
            .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();

        let config = GlobalConfig {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer.clone(),
            boundless_market_address: ctx.boundless_market_address,
            set_verifier_address: ctx.set_verifier_address,
            tx_timeout: None,
            log_level: LevelFilter::INFO,
        };

        let request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
            false,
            None,
        );

        // Create a new args struct to test the Status command
        let status_args = MainArgs {
            config,
            command: Command::Request(Box::new(RequestCommands::Status {
                request_id: request.id,
                expires_at: None,
            })),
        };

        run(&status_args).await.unwrap();

        assert!(logs_contain(&format!(
            "RequestStatus {{ id: 0x{:x}, status: Unknown}}",
            request.id
        )));
    }
}
