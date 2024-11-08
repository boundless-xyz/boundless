// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.
#[cfg(feature = "cli")]
use std::{borrow::Cow, fs::File, io::BufReader, path::PathBuf, time::Duration};

use alloy::{
    network::Ethereum,
    primitives::{
        aliases::U192,
        utils::{format_ether, parse_ether},
        Address, Bytes, B256, U256,
    },
    providers::{network::EthereumWallet, Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    transports::Transport,
};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use guest_util::ECHO_ELF;
use hex::FromHex;
use risc0_ethereum_contracts::IRiscZeroVerifier;
use risc0_zkvm::{
    default_executor,
    sha::{Digest, Digestible},
    ExecutorEnv, Journal, SessionInfo,
};
use url::Url;

use boundless_market::{
    client::Client,
    contracts::{
        proof_market::ProofMarketService, Input, InputType, Offer, Predicate, PredicateType,
        ProvingRequest, Requirements,
    },
    storage::{
        storage_provider_from_env, BuiltinStorageProvider, StorageProvider, TempFileStorageProvider,
    },
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
        /// Submit the request offchain via the order stream service
        #[clap(short, long, default_value = "false")]
        offchain: bool,
        /// Use the RISC Zero zkvm executor to run the program
        /// without submitting the request
        #[clap(long, default_value = "false")]
        dry_run: bool,
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
    /// Get the status of a given request
    Status {
        /// The proof request identifier
        request_id: U256,
        /// The block number at which the request expires
        expires_at: Option<u64>,
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
    /// Submit the request offchain via the order stream service
    #[clap(short, long, default_value = "false")]
    offchain: bool,
    /// Use the RISC Zero zkvm executor to run the program
    /// without submitting the request
    #[clap(long, default_value = "false")]
    dry_run: bool,
    /// Use risc0_zkvm::serde to encode the input as a `Vec<u8>`
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
    /// URL of the Ethereum RPC endpoint
    #[clap(short, long, env, default_value = "http://localhost:8545")]
    rpc_url: Url,
    /// URL of the order stream service
    #[clap(long, env, default_value = "http://localhost:8585")]
    order_stream_url: Url,
    /// Private key of the wallet
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the proof market contract
    #[clap(short, long, env)]
    proof_market_address: Address,
    /// Address of the SetVerifier contract
    #[clap(short, long, env)]
    set_verifier_address: Address,
    /// Tx timeout in seconds
    #[clap(long, env)]
    tx_timeout: Option<u64>,
    /// Subcommand to run
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => tracing::debug!("No .env file found"),
        Err(e) => bail!("failed to load .env file: {}", e),
    }

    let args = MainArgs::try_parse()?;
    run(&args).await.unwrap();

    Ok(())
}

pub(crate) async fn run(args: &MainArgs) -> Result<Option<U256>> {
    let caller = args.private_key.address();
    let signer = args.private_key.clone();
    let wallet = EthereumWallet::from(args.private_key.clone());
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_http(args.rpc_url.clone());
    let mut proof_market =
        ProofMarketService::new(args.proof_market_address, provider.clone(), caller);
    if let Some(tx_timeout) = args.tx_timeout {
        proof_market = proof_market.with_timeout(Duration::from_secs(tx_timeout));
    }

    let command = args.command.clone();

    let mut request_id = None;
    match command {
        Command::Deposit { amount } => {
            proof_market.deposit(amount).await?;
            tracing::info!("Deposited: {}", format_ether(amount));
        }
        Command::Withdraw { amount } => {
            proof_market.withdraw(amount).await?;
            tracing::info!("Withdrew: {}", format_ether(amount));
        }
        Command::Balance { address } => {
            let addr = address.unwrap_or(caller);
            let balance = proof_market.balance_of(addr).await?;
            tracing::info!("Balance of {addr}: {}", format_ether(balance));
        }
        Command::SubmitOffer(offer_args) => {
            let storage_provider = if cfg!(test) {
                BuiltinStorageProvider::File(TempFileStorageProvider::new().await?)
            } else {
                storage_provider_from_env().await?
            };
            let mut client = Client::from_parts(
                signer.clone(),
                args.rpc_url.clone(),
                args.proof_market_address,
                args.set_verifier_address,
                args.order_stream_url.clone(),
                storage_provider,
            )
            .await?;
            if let Some(tx_timeout) = args.tx_timeout {
                client = client.with_timeout(Duration::from_secs(tx_timeout));
            }

            request_id = submit_offer(client, &offer_args).await?;
        }
        Command::SubmitRequest { yaml_request, id, wait, offchain, dry_run } => {
            let id = match id {
                Some(id) => id,
                None => proof_market.index_from_rand().await?,
            };

            let storage_provider = if cfg!(test) {
                BuiltinStorageProvider::File(TempFileStorageProvider::new().await?)
            } else {
                storage_provider_from_env().await?
            };
            let mut client = Client::from_parts(
                signer.clone(),
                args.rpc_url.clone(),
                args.proof_market_address,
                args.set_verifier_address,
                args.order_stream_url.clone(),
                storage_provider,
            )
            .await?;
            if let Some(tx_timeout) = args.tx_timeout {
                client = client.with_timeout(Duration::from_secs(tx_timeout));
            }

            request_id = submit_request(id, yaml_request, client, wait, offchain, dry_run).await?;
        }
        Command::Slash { request_id } => {
            proof_market.slash(request_id).await?;
            tracing::info!("Request slashed: 0x{request_id:x}");
        }
        Command::GetProof { request_id } => {
            let (journal, seal) = proof_market.get_request_fulfillment(request_id).await?;
            tracing::info!(
                "Journal: {} - Seal: {}",
                serde_json::to_string_pretty(&journal)?,
                serde_json::to_string_pretty(&seal)?
            );
        }
        Command::VerifyProof { request_id, image_id } => {
            let (journal, seal) = proof_market.get_request_fulfillment(request_id).await?;
            let journal_digest = <[u8; 32]>::from(Journal::new(journal.to_vec()).digest()).into();
            let set_verifier = IRiscZeroVerifier::new(args.set_verifier_address, provider.clone());
            set_verifier
                .verify(seal, image_id, journal_digest)
                .call()
                .await
                .map_err(|_| anyhow::anyhow!("Verification failed"))?;
            tracing::info!("Proof for request id 0x{request_id:x} verified successfully.");
        }
        Command::Status { request_id, expires_at } => {
            let status = proof_market.get_status(request_id, expires_at).await?;
            tracing::info!("Status: {:?}", status);
        }
    };

    Ok(request_id)
}

async fn submit_offer<T, P, S>(
    client: Client<T, P, S>,
    args: &SubmitOfferArgs,
) -> Result<Option<U256>>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // Read the YAML offer file
    let file = File::open(&args.yaml_offer)?;
    let reader = BufReader::new(file);
    let mut offer: Offer =
        serde_yaml::from_reader(reader).context("failed to parse offer from YAML")?;

    // If set to 0, override the offer bidding_start field with the current block number.
    if offer.biddingStart == 0 {
        let latest_block = client
            .proof_market
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

    // Compute the image_id, then upload the ELF.
    let elf_url = client.upload_image(&elf).await?;
    let image_id = B256::from(<[u8; 32]>::from(risc0_zkvm::compute_image_id(&elf)?));

    // Upload the input.
    let requirements_input = match args.inline_input {
        false => {
            let input_url = client.upload_input(&encoded_input).await?;
            Input { inputType: InputType::Url, data: input_url.into() }
        }
        true => Input { inputType: InputType::Inline, data: encoded_input.into() },
    };

    // Set request id
    let id = match args.id {
        Some(id) => id,
        None => client.proof_market.index_from_rand().await?,
    };

    // Construct the request from its individual parts.
    let request = ProvingRequest::new(
        id,
        &client.signer.address(),
        Requirements { imageId: image_id, predicate },
        &elf_url,
        requirements_input,
        offer.clone(),
    );

    tracing::debug!("Request: {}", serde_json::to_string_pretty(&request)?);

    if args.dry_run {
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;
        if !request.requirements.predicate.eval(&journal) {
            bail!("Predicate evaluation failed");
        }
        tracing::info!("Dry-run succeeded.");
        return Ok(None);
    }

    let request_id = if args.offchain {
        client.submit_request_offchain(&request).await?
    } else {
        client.submit_request(&request).await?
    };
    tracing::info!(
        "Submitted request ID 0x{request_id:x}, bidding start at block number {}",
        offer.biddingStart
    );

    if args.wait {
        let (journal, seal) = client
            .proof_market
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), request.expires_at())
            .await?;
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    };
    Ok(Some(request_id))
}

async fn submit_request<T, P, S>(
    id: u32,
    request_path: String,
    client: Client<T, P, S>,
    wait: bool,
    offchain: bool,
    dry_run: bool,
) -> Result<Option<U256>>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    // Read the YAML request file
    let file = File::open(request_path).context("failed to open request file")?;
    let reader = BufReader::new(file);
    let mut request_yaml: ProvingRequest =
        serde_yaml::from_reader(reader).context("failed to parse request from YAML")?;

    // If set to 0, override the offer bidding_start field with the current block number.
    if request_yaml.offer.biddingStart == 0 {
        let latest_block = client
            .proof_market
            .instance()
            .provider()
            .get_block_number()
            .await
            .context("Failed to get block number")?;
        request_yaml.offer = Offer { biddingStart: latest_block, ..request_yaml.offer };
    }

    let mut request = ProvingRequest::new(
        id,
        &client.signer.address(),
        request_yaml.requirements,
        &request_yaml.imageUrl,
        request_yaml.input,
        request_yaml.offer,
    );

    // Use the original request id if it was set
    if request_yaml.id != U192::ZERO {
        request.id = request_yaml.id;
    }

    if dry_run {
        let session_info = execute(&request).await?;
        let journal = session_info.journal.bytes;
        if !request.requirements.predicate.eval(&journal) {
            bail!("Predicate evaluation failed");
        }
        tracing::info!("Dry-run succeeded.");
        return Ok(None);
    }

    let request_id = if offchain {
        client.submit_request_offchain(&request).await?
    } else {
        client.submit_request(&request).await?
    };
    tracing::info!(
        "Proving request ID 0x{request_id:x}, bidding start at block number {}",
        request.offer.biddingStart
    );

    if wait {
        let (journal, seal) = client
            .proof_market
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), request.expires_at())
            .await?;
        tracing::info!(
            "Journal: {} - Seal: {}",
            serde_json::to_string_pretty(&journal)?,
            serde_json::to_string_pretty(&seal)?
        );
    };
    Ok(Some(request_id))
}

async fn execute(request: &ProvingRequest) -> Result<SessionInfo> {
    let elf = fetch_url(&request.imageUrl.to_string()).await?;
    let input = match request.input.inputType {
        InputType::Inline => request.input.data.clone(),
        InputType::Url => fetch_url(&request.input.data.to_string()).await?.into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    use alloy::node_bindings::Anvil;
    use boundless_market::contracts::test_utils::TestCtx;
    use tokio::time::timeout;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_deposit_withdraw() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = TestCtx::new(&anvil).await.unwrap();

        let mut args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.prover_signer.clone(),
            proof_market_address: ctx.proof_market_addr,
            set_verifier_address: ctx.set_verifier_addr,
            order_stream_url: Url::parse("http://localhost:8080").unwrap(),
            tx_timeout: None,
            command: Command::Deposit { amount: U256::from(100) },
        };

        run(&args).await.unwrap();

        let balance = ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap();
        assert_eq!(balance, U256::from(100));

        args.command = Command::Withdraw { amount: U256::from(100) };
        run(&args).await.unwrap();

        let balance = ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap();
        assert_eq!(balance, U256::from(0));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_submit_request() {
        // Setup anvil
        let anvil = Anvil::new().spawn();

        let ctx = TestCtx::new(&anvil).await.unwrap();
        ctx.prover_market.deposit(parse_ether("2").unwrap()).await.unwrap();

        let mut args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer.clone(),
            proof_market_address: ctx.proof_market_addr,
            set_verifier_address: ctx.set_verifier_addr,
            order_stream_url: Url::parse("http://localhost:8080").unwrap(),
            tx_timeout: None,
            command: Command::SubmitRequest {
                yaml_request: "../../request.yaml".to_string(),
                id: None,
                wait: false,
                offchain: false,
                dry_run: false,
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
