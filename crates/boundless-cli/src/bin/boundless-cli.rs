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
        Address, Bytes, FixedBytes, B256, U256,
    },
    providers::{network::EthereumWallet, Provider, ProviderBuilder},
    signers::{local::PrivateKeySigner, Signer},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use boundless_cli::{DefaultProver, OrderFulfilled};
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
        PredicateType, ProofRequest, Requirements,
    },
    input::{GuestEnv, InputBuilder},
    storage::{StorageProvider, StorageProviderConfig},
};

// TODO(victor): Make it possible to specify global args (e.g. RPC URL) before or after the
// command.
#[derive(Subcommand, Clone, Debug)]
enum Command {
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
    BalanceOfStake {
        /// Address to check the balance of;
        /// if not provided, defaults to the wallet address
        address: Option<Address>,
    },
    /// Submit a proof request, constructed with the given offer, input, and image.
    SubmitOffer(SubmitOfferArgs),
    /// Submit a fully specified proof request
    SubmitRequest {
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
        /// Submit the request offchain via the provided order stream service url.
        #[clap(short, long, requires = "order_stream_url")]
        offchain: bool,
        /// Offchain order stream service URL to submit offchain requests to.
        #[clap(long, env)]
        order_stream_url: Option<Url>,
        /// Preflight uses the RISC Zero zkvm executor to run the program
        /// before submitting the request. Set no-preflight to skip.
        #[clap(long, default_value = "false")]
        no_preflight: bool,
    },
    /// Slash a prover for a given request
    Slash {
        /// The proof request identifier
        request_id: U256,
    },
    /// Get the journal and seal for a given request
    GetProof {
        /// The proof request identifier
        request_id: U256,
    },
    /// Verify the proof of the given request against
    /// the SetVerifier contract.
    VerifyProof {
        /// The proof request identifier
        request_id: U256,
        /// The image id of the original request
        image_id: B256,
    },
    GetSetInclusionReceipt {
        /// The proof request identifier
        request_id: U256,
        /// The image id of the request
        image_id: B256,
    },
    /// Get the status of a given request
    Status {
        /// The proof request identifier
        request_id: U256,
        /// The time at which the request expires, in seconds since the UNIX epoch.
        expires_at: Option<u64>,
    },
    /// Execute a proof request using the RISC Zero zkVM executor.
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
        /// Whether to revert the fulfill transaction if payment conditions are not met (e.g. the
        /// request is locked to another prover).
        #[arg(long, default_value = "false")]
        require_payment: bool,
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
    /// Submit the request offchain via the provided order stream service url.
    #[clap(short, long, requires = "order_stream_url")]
    offchain: bool,
    /// Offchain order stream service URL to submit offchain requests to.
    #[clap(long, env, default_value = "https://order-stream.beboundless.xyz")]
    order_stream_url: Option<Url>,
    /// Preflight uses the RISC Zero zkvm executor to run the program
    /// before submitting the request. Set no-preflight to skip.
    #[clap(long, default_value = "false")]
    no_preflight: bool,
    /// Use risc0_zkvm::serde to encode the input as a `Vec<u8>`
    #[clap(short, long)]
    encode_input: bool,
    /// Send the input inline (i.e. in the transaction calldata) rather than uploading it.
    #[clap(long)]
    inline_input: bool,
    /// Elf file to use as the guest image, given as a path.
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
    callback_addr: Option<Address>,
    /// Gas limit of the callback to use in the requirements.
    #[clap(long, requires = "callback_addr")]
    callback_gas_limit: Option<u64>,
}

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct MainArgs {
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
    /// Subcommand to run
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).from_env_lossy(),
        )
        .init();

    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => tracing::debug!("No .env file found"),
        Err(e) => bail!("failed to load .env file: {}", e),
    }

    let args = MainArgs::parse();
    run(&args).await.unwrap();

    Ok(())
}

pub(crate) async fn run(args: &MainArgs) -> Result<Option<U256>> {
    let caller = args.private_key.address();
    let wallet = EthereumWallet::from(args.private_key.clone());
    let provider = ProviderBuilder::new().wallet(wallet).on_http(args.rpc_url.clone());
    let mut boundless_market =
        BoundlessMarketService::new(args.boundless_market_address, provider.clone(), caller);
    if let Some(tx_timeout) = args.tx_timeout {
        boundless_market = boundless_market.with_timeout(tx_timeout);
    }

    let command = args.command.clone();

    let mut request_id = None;
    match command {
        Command::Deposit { amount } => {
            boundless_market.deposit(amount).await?;
            tracing::info!("Deposited: {}", format_ether(amount));
        }
        Command::Withdraw { amount } => {
            boundless_market.withdraw(amount).await?;
            tracing::info!("Withdrew: {}", format_ether(amount));
        }
        Command::Balance { address } => {
            let addr = address.unwrap_or(caller);
            let balance = boundless_market.balance_of(addr).await?;
            tracing::info!("Balance of {addr}: {}", format_ether(balance));
        }
        Command::DepositStake { amount } => {
            boundless_market.deposit_stake_with_permit(amount, &args.private_key).await?;
            tracing::info!("Deposited stake: {}", amount);
        }
        Command::WithdrawStake { amount } => {
            boundless_market.withdraw_stake(amount).await?;
            tracing::info!("Withdrew stake: {}", amount);
        }
        Command::BalanceOfStake { address } => {
            let addr = address.unwrap_or(caller);
            let balance = boundless_market.balance_of_stake(addr).await?;
            tracing::info!("Stake balance of {addr}: {}", balance);
        }
        Command::SubmitOffer(offer_args) => {
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
                .with_private_key(args.private_key.clone())
                .with_rpc_url(args.rpc_url.clone())
                .with_boundless_market_address(args.boundless_market_address)
                .with_set_verifier_address(args.set_verifier_address)
                .with_storage_provider_config(offer_args.storage_config.clone())
                .with_order_stream_url(order_stream_url)
                .with_timeout(args.tx_timeout)
                .build()
                .await?;

            request_id = submit_offer(client, &args.private_key, &offer_args).await?;
        }
        Command::SubmitRequest {
            storage_config,
            yaml_request,
            id,
            wait,
            offchain,
            order_stream_url,
            no_preflight,
        } => {
            let id = match id {
                Some(id) => id,
                None => boundless_market.index_from_rand().await?,
            };
            let order_stream_url = offchain
                .then_some(
                    order_stream_url
                        .ok_or(anyhow!("offchain flag set, but order stream URL not provided")),
                )
                .transpose()?;
            let client = ClientBuilder::default()
                .with_private_key(args.private_key.clone())
                .with_rpc_url(args.rpc_url.clone())
                .with_boundless_market_address(args.boundless_market_address)
                .with_set_verifier_address(args.set_verifier_address)
                .with_order_stream_url(order_stream_url.clone())
                .with_storage_provider_config(storage_config)
                .with_timeout(args.tx_timeout)
                .build()
                .await?;

            request_id = submit_request(
                id,
                yaml_request,
                client,
                &args.private_key,
                wait,
                offchain,
                !no_preflight,
            )
            .await?;
        }
        Command::Slash { request_id } => {
            boundless_market.slash(request_id).await?;
            tracing::info!("Request slashed: 0x{request_id:x}");
        }
        Command::GetProof { request_id } => {
            let (journal, seal) = boundless_market.get_request_fulfillment(request_id).await?;
            tracing::info!(
                "Journal: {} - Seal: {}",
                serde_json::to_string_pretty(&journal)?,
                serde_json::to_string_pretty(&seal)?
            );
        }
        Command::VerifyProof { request_id, image_id } => {
            let (journal, seal) = boundless_market.get_request_fulfillment(request_id).await?;
            let journal_digest = <[u8; 32]>::from(Journal::new(journal.to_vec()).digest()).into();
            let set_verifier = IRiscZeroVerifier::new(args.set_verifier_address, provider.clone());
            set_verifier
                .verify(seal, image_id, journal_digest)
                .call()
                .await
                .map_err(|_| anyhow::anyhow!("Verification failed"))?;
            tracing::info!("Proof for request id 0x{request_id:x} verified successfully.");
        }
        Command::GetSetInclusionReceipt { request_id, image_id } => {
            let client = ClientBuilder::default()
                .with_private_key(args.private_key.clone())
                .with_rpc_url(args.rpc_url.clone())
                .with_boundless_market_address(args.boundless_market_address)
                .with_set_verifier_address(args.set_verifier_address)
                .with_timeout(args.tx_timeout)
                .build()
                .await?;
            let (journal, receipt) =
                client.fetch_set_inclusion_receipt(request_id, image_id).await?;
            tracing::info!(
                "Journal: {} - Receipt: {}",
                serde_json::to_string_pretty(&journal)?,
                serde_json::to_string_pretty(&receipt)?
            );
        }
        Command::Status { request_id, expires_at } => {
            let status = boundless_market.get_status(request_id, expires_at).await?;
            tracing::info!("Status: {:?}", status);
        }
        Command::Execute {
            request_id,
            request_digest,
            request_path,
            tx_hash,
            order_stream_url,
        } => {
            let request: ProofRequest = if let Some(file_path) = request_path {
                let file = File::open(file_path).context("failed to open request file")?;
                let reader = BufReader::new(file);
                serde_yaml::from_reader(reader).context("failed to parse request from YAML")?
            } else if let Some(request_id) = request_id {
                let client = ClientBuilder::default()
                    .with_private_key(args.private_key.clone())
                    .with_rpc_url(args.rpc_url.clone())
                    .with_boundless_market_address(args.boundless_market_address)
                    .with_set_verifier_address(args.set_verifier_address)
                    .with_order_stream_url(order_stream_url.clone())
                    .with_timeout(args.tx_timeout)
                    .build()
                    .await?;
                let order = client.fetch_order(request_id, tx_hash, request_digest).await?;
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
        }
        Command::Fulfill {
            request_id,
            request_digest,
            tx_hash,
            order_stream_url,
            require_payment,
        } => {
            let (_, market_url) = boundless_market.image_info().await?;
            tracing::debug!("Fetching Assessor ELF from {}", market_url);
            let assessor_elf = fetch_url(&market_url).await?;
            let domain = boundless_market.eip712_domain().await?;

            let mut set_verifier =
                SetVerifierService::new(args.set_verifier_address, provider.clone(), caller);
            if let Some(tx_timeout) = args.tx_timeout {
                set_verifier = set_verifier.with_timeout(tx_timeout);
            }
            let (_, set_builder_url) = set_verifier.image_info().await?;
            tracing::debug!("Fetching SetBuilder ELF from {}", set_builder_url);
            let set_builder_elf = fetch_url(&set_builder_url).await?;

            let prover = DefaultProver::new(set_builder_elf, assessor_elf, caller, domain)?;

            let client = ClientBuilder::default()
                .with_private_key(args.private_key.clone())
                .with_rpc_url(args.rpc_url.clone())
                .with_boundless_market_address(args.boundless_market_address)
                .with_set_verifier_address(args.set_verifier_address)
                .with_order_stream_url(order_stream_url.clone())
                .with_timeout(args.tx_timeout)
                .build()
                .await?;

            let order = client.fetch_order(request_id, tx_hash, request_digest).await?;
            tracing::debug!("Fulfilling request {:?}", order.request);
            let sig: Bytes = order.signature.as_bytes().into();
            order.request.verify_signature(
                &sig,
                args.boundless_market_address,
                boundless_market.get_chain_id().await?,
            )?;

            let (fill, root_receipt, _, assessor_receipt) =
                prover.fulfill(order.clone(), require_payment).await?;
            let order_fulfilled =
                OrderFulfilled::new(fill, root_receipt, assessor_receipt, caller)?;
            set_verifier.submit_merkle_root(order_fulfilled.root, order_fulfilled.seal).await?;

            // If the request is not locked in, we need to "price" which checks the requirements
            // and assigns a price. Otherwise, we don't. This vec will be a singleton if not locked
            // and empty if the request is locked.
            let requests_to_price: Vec<ProofRequest> =
                (!boundless_market.is_locked(request_id).await?)
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
                }
                Err(e) => {
                    tracing::error!("Failed to fulfill request 0x{:x}: {}", request_id, e);
                }
            }
        }
    };

    Ok(request_id)
}

async fn submit_offer<P, S>(
    client: Client<P, S>,
    signer: &impl Signer,
    args: &SubmitOfferArgs,
) -> Result<Option<U256>>
where
    P: Provider<Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // TODO(victor): Execute the request before sending it.
    // Read the YAML offer file
    let file = File::open(&args.yaml_offer)?;
    let reader = BufReader::new(file);
    let mut offer: Offer =
        serde_yaml::from_reader(reader).context("failed to parse offer from YAML")?;

    // If set to 0, override the offer bidding_start field with the current timestamp + 30 seconds.
    if offer.biddingStart == 0 {
        // NOTE: Adding a bit of a delay to bidding start lets provers see and evaluate the request
        // before the price starts to ramp up. 30s is an arbitrary value.
        offer = Offer { biddingStart: now_timestamp() + 30, ..offer };
    }

    // Resolve the ELF and input from command line arguments.
    let elf: Cow<'static, [u8]> = std::fs::read(&args.elf).map(Into::into)?;
    let input: Vec<u8> = match (&args.input.input, &args.input.input_file) {
        (Some(input), None) => input.as_bytes().to_vec(),
        (None, Some(input_file)) => std::fs::read(input_file)?,
        _ => bail!("exactly one of input or input-file args must be provided"),
    };
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
        _ => bail!("exactly one of journal-digest or journal-prefix args must be provided"),
    };

    let callback = match (&args.reqs.callback_addr, &args.reqs.callback_gas_limit) {
        (Some(addr), Some(gas_limit)) => Callback { addr: *addr, gasLimit: U96::from(*gas_limit) },
        _ => Callback::default(),
    };

    // Compute the image_id, then upload the ELF.
    let elf_url = client.upload_image(&elf).await?;
    let image_id = B256::from(<[u8; 32]>::from(risc0_zkvm::compute_image_id(&elf)?));

    // Upload the input.
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
    let request = ProofRequest::new(
        id,
        &client.caller(),
        Requirements { imageId: image_id, predicate, callback, selector: FixedBytes::<4>([0; 4]) },
        elf_url,
        requirements_input,
        offer.clone(),
    );

    tracing::debug!("Request: {}", serde_json::to_string_pretty(&request)?);

    if !args.no_preflight {
        tracing::info!("Running request preflight");
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;
        ensure!(
            request.requirements.predicate.eval(&journal),
            "Predicate evaluation failed; journal does not match requirements"
        );
        tracing::debug!("Preflight succeeded");
    }

    let (request_id, expires_at) = if args.offchain {
        client.submit_request_offchain_with_signer(&request, signer).await?
    } else {
        client.submit_request_with_signer(&request, signer).await?
    };
    tracing::info!(
        "Submitted request ID 0x{request_id:x}, bidding start at timestamp {}",
        offer.biddingStart
    );

    if args.wait {
        let (journal, seal) = client
            .boundless_market
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
            .await?;
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    };
    Ok(Some(request_id))
}

async fn submit_request<P, S>(
    id: u32,
    request_path: impl AsRef<Path>,
    client: Client<P, S>,
    signer: &impl Signer,
    wait: bool,
    offchain: bool,
    preflight: bool,
) -> Result<Option<U256>>
where
    P: Provider<Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // TODO(victor): Execute the request before sending it.
    // Read the YAML request file
    let file = File::open(request_path.as_ref()).context("failed to open request file")?;
    let reader = BufReader::new(file);
    let mut request_yaml: ProofRequest =
        serde_yaml::from_reader(reader).context("failed to parse request from YAML")?;

    // If set to 0, override the offer bidding_start field with the current timestamp + 30s.
    if request_yaml.offer.biddingStart == 0 {
        // NOTE: Adding a bit of a delay to bidding start lets provers see and evaluate the request
        // before the price starts to ramp up. 30s is an arbitrary value.
        request_yaml.offer = Offer { biddingStart: now_timestamp() + 30, ..request_yaml.offer };
    }

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

    if preflight {
        tracing::info!("Running request preflight");
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;
        if let Some(claim) = session_info.receipt_claim {
            ensure!(
                claim.pre.digest().as_bytes() == request_yaml.requirements.imageId.as_slice(),
                "image ID in requirements does not match the given ELF: {} != {}",
                claim.pre.digest(),
                request_yaml.requirements.imageId
            );
        } else {
            tracing::debug!("cannot check image id; session info doesn't have receipt claim");
        }
        ensure!(
            request.requirements.predicate.eval(&journal),
            "Predicate evaluation failed; journal does not match requirements"
        );
        tracing::debug!("Preflight succeeded");
    }

    let (request_id, expires_at) = if offchain {
        client.submit_request_offchain_with_signer(&request, signer).await?
    } else {
        client.submit_request_with_signer(&request, signer).await?
    };
    tracing::info!(
        "Request ID 0x{request_id:x}, bidding start at timestamp {}",
        request.offer.biddingStart
    );

    if wait {
        let (journal, seal) = client
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
            .await?;
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    };
    Ok(Some(request_id))
}

async fn execute(request: &ProofRequest) -> Result<SessionInfo> {
    let elf = fetch_url(&request.imageUrl).await?;
    let input = match request.input.inputType {
        InputType::Inline => GuestEnv::decode(&request.input.data)?.stdin,
        InputType::Url => {
            GuestEnv::decode(
                &fetch_url(
                    std::str::from_utf8(&request.input.data).context("input url is not utf8")?,
                )
                .await?,
            )?
            .stdin
        }
        _ => bail!("Unsupported input type"),
    };
    let env = ExecutorEnv::builder().write_slice(&input).build()?;
    default_executor().execute(env, &elf)
}

async fn fetch_url(url_str: &str) -> Result<Vec<u8>> {
    let url = Url::parse(url_str)?;

    match url.scheme() {
        "http" | "https" => fetch_http(&url).await,
        "file" => fetch_file(&url).await,
        _ => bail!("unsupported URL scheme: {}", url.scheme()),
    }
}

async fn fetch_http(url: &Url) -> Result<Vec<u8>> {
    let response = reqwest::get(url.as_str()).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP request failed with status: {}", status);
    }

    Ok(response.bytes().await?.to_vec())
}

async fn fetch_file(url: &Url) -> Result<Vec<u8>> {
    let path = std::path::Path::new(url.path());
    let data = tokio::fs::read(path).await?;
    Ok(data)
}

// TODO(#379): Avoid drift relative to the chain's timestamps.
fn now_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloy::node_bindings::Anvil;
    use boundless_market::contracts::{hit_points::default_allowance, test_utils::create_test_ctx};
    use guest_assessor::ASSESSOR_GUEST_ID;
    use guest_set_builder::SET_BUILDER_ID;
    use tokio::time::timeout;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_deposit_withdraw() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();

        let mut args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.prover_signer.clone(),
            boundless_market_address: ctx.boundless_market_addr,
            set_verifier_address: ctx.set_verifier_addr,
            tx_timeout: None,
            command: Command::Deposit { amount: default_allowance() },
        };

        run(&args).await.unwrap();

        let balance = ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap();
        assert_eq!(balance, default_allowance());

        args.command = Command::Withdraw { amount: default_allowance() };
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

        let mut args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer.clone(),
            boundless_market_address: ctx.boundless_market_addr,
            set_verifier_address: ctx.set_verifier_addr,
            tx_timeout: None,
            command: Command::SubmitRequest {
                storage_config: Some(StorageProviderConfig::dev_mode()),
                yaml_request: "../../request.yaml".to_string().into(),
                id: None,
                wait: false,
                offchain: false,
                order_stream_url: None,
                no_preflight: false,
            },
        };

        let result = timeout(Duration::from_secs(60), run(&args)).await;

        let request_id = match result {
            Ok(run_result) => match run_result {
                Ok(value) => value.unwrap(),
                Err(e) => {
                    panic!("`run` returned an error: {:?}", e);
                }
            },
            Err(_) => {
                panic!("Test timed out after 1 minute");
            }
        };

        // GetStatus
        args.command = Command::Status { request_id, expires_at: None };
        run(&args).await.unwrap();
    }
}
