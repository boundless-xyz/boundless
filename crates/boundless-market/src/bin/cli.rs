// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.
#[cfg(feature = "cli")]
use std::{borrow::Cow, fs::File, io::BufReader, path::PathBuf, time::Duration};

use alloy::{
    network::Ethereum,
    primitives::{utils::parse_ether, Address, Bytes, B256, U256},
    providers::{network::EthereumWallet, Provider, ProviderBuilder},
    signers::{local::PrivateKeySigner, Signer, SignerSync},
    transports::Transport,
};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use guest_util::ECHO_ELF;
use hex::FromHex;
use risc0_zkvm::sha::Digest;
use url::Url;

use boundless_market::{
    contracts::{
        proof_market::ProofMarketService, Input, InputType, Offer, Predicate, PredicateType,
        ProvingRequest, Requirements,
    },
    storage::{storage_provider_from_env, StorageProvider},
};

// TODO(victor): Update corresponding docs
#[derive(Subcommand, Clone, Debug)]
enum Command {
    /// Deposit funds into the proof market
    Deposit {
        /// Amount in ether to deposit
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// Withdraw funds from the proof market
    Withdraw {
        /// Amount in ether to withdraw
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// Check the balance of an account in the proof market
    Balance {
        /// Address to check the balance of;
        /// if not provided, defaults to the wallet address
        address: Option<Address>,
    },
    /// Submit a proving request, constructed with the given offer, input, and image.
    SubmitOffer(SubmitOfferArgs),
    /// Submit a fully specified proving request
    SubmitRequest {
        /// Path to a YAML file containing the request
        yaml_request: String,
        /// Optional identifier for the request
        id: Option<u32>,
        /// Wait until the request is fulfilled
        #[clap(short, long, default_value = "false")]
        wait: bool,
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
        /// Wait until the request is fulfilled
        #[clap(short, long, default_value = "false")]
        wait: bool,
    },
    /// Get the status of a given request
    Status {
        /// The proof request identifier
        request_id: U256,
    },
}

#[derive(Args, Clone, Debug)]
struct SubmitOfferArgs {
    /// Path to a YAML file containing the offer
    yaml_offer: String,
    /// Optional identifier for the request
    id: Option<u32>,
    /// Wait until the request is fulfilled
    #[clap(short, long, default_value = "false")]
    wait: bool,
    /// Use risc0_zkvm::serde to encode the input as a Vec<u8>
    #[clap(short, long)]
    encode_input: bool,
    /// Send the input inline (i.e. in the transaction calldata) rather than uploading it.
    #[clap(long)]
    inline_input: bool,
    /// Elf file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included echo guest.
    #[clap(long)]
    elf: Option<PathBuf>,

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
}

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct MainArgs {
    #[clap(short, long, env, default_value = "http://localhost:8545")]
    rpc_url: Url,
    #[clap(short, long, env)]
    wallet_private_key: PrivateKeySigner,
    #[clap(short, long, env)]
    proof_market_address: Address,
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    dotenvy::dotenv()?;
    let args = MainArgs::try_parse()?;

    let caller = args.wallet_private_key.address();
    let signer = args.wallet_private_key.clone();
    let wallet = EthereumWallet::from(args.wallet_private_key.clone());
    let provider =
        ProviderBuilder::new().with_recommended_fillers().wallet(wallet).on_http(args.rpc_url);
    let market = ProofMarketService::new(args.proof_market_address, provider.clone(), caller);

    let command = args.command.clone();
    match command {
        Command::Deposit { amount } => {
            market.deposit(amount).await?;
            tracing::info!("Deposited: {}", amount);
        }
        Command::Withdraw { amount } => {
            market.withdraw(amount).await?;
            tracing::info!("Withdrew: {}", amount);
        }
        Command::Balance { address } => {
            let addr = address.unwrap_or(caller);
            let balance = market.balance_of(addr).await?;
            tracing::info!("Balance of {addr}: {balance}");
        }
        Command::SubmitOffer(args) => submit_offer(market, &args, signer).await?,
        Command::SubmitRequest { yaml_request, id, wait } => {
            let id = match id {
                Some(id) => id,
                None => market.gen_random_id().await?,
            };
            submit_request(id, market, yaml_request, signer, wait).await?
        }
        Command::Slash { request_id } => {
            market.slash(request_id).await?;
            tracing::info!("Request slashed: {}", request_id);
        }
        Command::GetProof { request_id, wait } => {
            let (journal, seal) = if wait {
                market
                    .wait_for_request_fulfillment(request_id, Duration::from_secs(5), None)
                    .await?
            } else {
                market.get_request_fulfillment(request_id).await?
            };
            tracing::info!(
                "Journal: {} - Seal: {}",
                serde_json::to_string_pretty(&journal)?,
                serde_json::to_string_pretty(&seal)?
            );
        }
        Command::Status { request_id } => {
            let status = market.get_status(request_id).await?;
            tracing::info!("Status: {:?}", status);
        }
    };

    Ok(())
}

async fn submit_offer<T, P>(
    market: ProofMarketService<T, P>,
    args: &SubmitOfferArgs,
    signer: impl Signer + SignerSync,
) -> Result<()>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    // Read the YAML offer file
    let file = File::open(&args.yaml_offer)?;
    let reader = BufReader::new(file);
    let mut offer: Offer =
        serde_yaml::from_reader(reader).context("failed to parse offer from YAML")?;

    // If set to 0, override the offer bidding_start field with the current block number.
    if offer.biddingStart == 0 {
        let latest_block = market
            .instance()
            .provider()
            .get_block_number()
            .await
            .context("Failed to get block number")?;
        offer = Offer { biddingStart: latest_block, ..offer };
    }

    // Resolve the ELF and input from command line arguments.
    let elf: Cow<'static, [u8]> = args
        .elf
        .as_ref()
        .map(|path| std::fs::read(path).map(Into::into))
        .unwrap_or(Ok(ECHO_ELF.into()))?;
    let input: Vec<u8> = match (&args.input.input, &args.input.input_file) {
        (Some(input), None) => input.as_bytes().to_vec(),
        (None, Some(input_file)) => std::fs::read(input_file)?,
        _ => bail!("exactly one of input or input-file args must be provided"),
    };
    let encoded_input = if args.encode_input {
        bytemuck::pod_collect_to_vec(&risc0_zkvm::serde::to_vec(&input)?)
    } else {
        input
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

    let storage_provider = storage_provider_from_env().await?;

    // Compute the image_id, then upload the ELF.
    let elf_url = storage_provider.upload_image(&elf).await?;
    let image_id = B256::from(<[u8; 32]>::from(risc0_zkvm::compute_image_id(&elf)?));

    // Upload the input.
    let requirements_input = match args.inline_input {
        false => {
            let input_url = storage_provider.upload_input(&encoded_input).await?;
            Input { inputType: InputType::Url, data: input_url.into() }
        }
        true => Input { inputType: InputType::Inline, data: encoded_input.into() },
    };

    // Set request id
    let id = match args.id {
        Some(id) => id,
        None => market.gen_random_id().await?,
    };

    // Construct the request from its individual parts.
    let request = ProvingRequest::new(
        id,
        &market.caller(),
        Requirements { imageId: image_id, predicate },
        &elf_url,
        requirements_input,
        offer.clone(),
    );

    tracing::debug!("Request: {}", serde_json::to_string_pretty(&request)?);

    let request_id = market.submit_request(&request, &signer).await?;
    tracing::info!(
        "Submitted request ID {}, bidding start at block number {}",
        request_id,
        offer.biddingStart
    );

    if args.wait {
        let (journal, seal) =
            market.wait_for_request_fulfillment(request_id, Duration::from_secs(5), None).await?;
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    };
    Ok(())
}

async fn submit_request<T, P>(
    id: u32,
    market: ProofMarketService<T, P>,
    request_path: String,
    signer: impl Signer + SignerSync,
    wait: bool,
) -> Result<()>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    // Read the YAML request file
    let file = File::open(request_path).context("failed to open request file")?;
    let reader = BufReader::new(file);
    let mut request_yaml: ProvingRequest =
        serde_yaml::from_reader(reader).context("failed to parse request from YAML")?;

    // If set to 0, override the offer bidding_start field with the current block number.
    if request_yaml.offer.biddingStart == 0 {
        let latest_block = market
            .instance()
            .provider()
            .get_block_number()
            .await
            .context("Failed to get block number")?;
        request_yaml.offer = Offer { biddingStart: latest_block, ..request_yaml.offer };
    }

    tracing::info!("Client addr: {}", signer.address());
    let mut request = ProvingRequest::new(
        id,
        &signer.address(),
        request_yaml.requirements,
        &request_yaml.imageUrl,
        request_yaml.input,
        request_yaml.offer,
    );

    // Use the original request id if it was set
    if request_yaml.id != U256::ZERO {
        request.id = request_yaml.id;
    }

    market.deposit(U256::from(request.offer.maxPrice)).await?;

    let request_id = market.submit_request(&request, &signer).await?;
    tracing::info!(
        "Proving request ID {}, bidding start at block number {}",
        request_id,
        request.offer.biddingStart
    );

    if wait {
        let (journal, seal) =
            market.wait_for_request_fulfillment(request_id, Duration::from_secs(5), None).await?;
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    };
    Ok(())
}
