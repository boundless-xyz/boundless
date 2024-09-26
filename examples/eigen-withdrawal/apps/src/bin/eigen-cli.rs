// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::time::Duration;

use alloy::{
    hex::FromHex,
    network::Ethereum,
    primitives::{utils::parse_ether, Address, Bytes, B256, U256},
    providers::{network::EthereumWallet, Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    transports::Transport,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use eigen::withdrawal::mock_proof;
use methods::EIGEN_WITHDRAWAL_ELF;
use risc0_zkvm::{compute_image_id, serde::to_vec};
use tracing::info;
use tracing_subscriber::EnvFilter;
use url::Url;

use boundless_market::{
    contracts::{
        proof_market::ProofMarketService, Input, InputType, Offer, Predicate, PredicateType,
        ProofStatus, ProvingRequest, Requirements,
    },
    storage::{storage_provider_from_env, StorageProvider},
};

use example_eigen::EigenPodService;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(long, env)]
    rpc_url: Url,
    /// Private key used to interact with the Counter contract.
    #[clap(long, env)]
    wallet_private_key: PrivateKeySigner,
    /// Address of the ProofMarket contract.
    #[clap(long, env)]
    proof_market_address: Address,
    /// Address of the EigenPod contract.
    #[clap(long, env)]
    eigen_pod_address: Address,
    /// IPFS Gateway URL.
    /// This is used to upload the guest code and input data.
    #[clap(long, env)]
    ipfs_gateway_url: Option<Url>,
    /// Subcommand to execute
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    /// Create a new validator with the given stake amount
    Stake {
        /// The stake amount in Ether
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// Update the stake of an existing validator
    UpdateStake {
        /// The index of the validator
        validator_index: u64,
        /// The new stake amount in Ether
        #[clap(value_parser = parse_ether)]
        amount: U256,
    },
    /// View the stake of a validator
    ViewStake { validator_index: u64 },
    /// Redeem the yield of a validator (partial withdrawal)
    Redeem {
        /// The index of the validator
        validator_index: u64,
        /// Wait until the request is fulfilled
        #[clap(short, long, default_value = "false")]
        wait: bool,
    },
    /// Redeem the yield of a validator (partial withdrawal) with a proof request id
    RedeemWithId {
        /// The proving request id
        request_id: U256,
    },
    /// View the current yield of a validator
    ViewYield { validator_index: u64 },
    /// Full withdrawal of a validator
    FullWithdraw {
        /// The index of the validator
        validator_index: u64,
        /// Wait until the request is fulfilled
        #[clap(short, long, default_value = "false")]
        wait: bool,
    },
    /// Full withdrawal of a validator with a proof request id
    FullWithdrawWithId {
        /// The proving request id
        request_id: U256,
    },
    /// Get the status for a proving request
    ProofStatus {
        /// The proving request id
        request_id: U256,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up tracing with a default log level of "info"
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info")) // Set default log level to "info"
        .init();
    dotenvy::dotenv()?;
    let args = Args::try_parse()?;

    let caller = args.wallet_private_key.address();
    let signer = args.wallet_private_key.clone();
    let wallet = EthereumWallet::from(args.wallet_private_key);
    let provider =
        ProviderBuilder::new().with_recommended_fillers().wallet(wallet).on_http(args.rpc_url);

    let market =
        ProofMarketService::new(args.proof_market_address, provider.clone(), caller.clone());
    let eigen_pod = EigenPodService::new(args.eigen_pod_address, provider.clone(), caller);

    let command = args.command.clone();
    match command {
        Command::Stake { amount } => {
            let (validator_index, stake) = eigen_pod.new_validator(amount).await?;
            info!("Validator created with index: {} and stake: {}Gwei", validator_index, stake);
        }
        Command::UpdateStake { validator_index, amount } => {
            let (validator_index, stake) = eigen_pod.update_stake(validator_index, amount).await?;
            info!("Validator updated with index: {} and stake: {}Gwei", validator_index, stake);
        }
        Command::ViewStake { validator_index } => {
            let stake = eigen_pod.view_stake(validator_index).await?;
            info!("Current stake: {}Gwei", stake);
        }
        Command::Redeem { validator_index, wait } => {
            let timestamp = eigen_pod.instance().provider().get_block_number().await?;
            let request = make_request(&market, validator_index, timestamp, false).await?;
            if let Some((journal, seal)) = submit_request(&market, &request, &signer, wait).await? {
                let amount = eigen_pod.partial_withdrawal(journal, seal).await?;
                info!("Partial withdrawal amount: {}", amount);
            }
        }
        Command::RedeemWithId { request_id } => {
            let (journal, seal) = market
                .wait_for_request_fulfillment(request_id, Duration::from_secs(5), None)
                .await?;
            let amount = eigen_pod.partial_withdrawal(journal, seal).await?;
            info!("Partial withdrawal amount: {}", amount);
        }
        Command::ViewYield { validator_index } => {
            let amount = eigen_pod.view_yield(validator_index).await?;
            info!("Current yield: {}", amount);
        }
        Command::FullWithdraw { validator_index, wait } => {
            let timestamp = eigen_pod.instance().provider().get_block_number().await?;
            let request = make_request(&market, validator_index, timestamp, true).await?;
            if let Some((journal, seal)) = submit_request(&market, &request, &signer, wait).await? {
                let (index, amount) = eigen_pod.full_withdrawal(journal, seal).await?;
                info!("Full withdrawal for validator {} with amount: {:?}", index, amount);
            }
        }
        Command::FullWithdrawWithId { request_id } => {
            let (journal, seal) = market
                .wait_for_request_fulfillment(request_id, Duration::from_secs(5), None)
                .await?;
            let (index, amount) = eigen_pod.full_withdrawal(journal, seal).await?;
            info!("Full withdrawal for validator {} with amount: {:?}", index, amount);
        }
        Command::ProofStatus { request_id } => {
            let status = get_status(market, request_id).await?;
            info!("Status: {:?}", status);
        }
    }

    Ok(())
}

async fn submit_request<T, P>(
    market: &ProofMarketService<T, P>,
    request: &ProvingRequest,
    signer: &PrivateKeySigner,
    wait: bool,
) -> Result<Option<(Bytes, Bytes)>>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    let request_id = market.submit_request(request, signer).await?;
    info!("Proving request ID: {}", request_id);

    if wait {
        let (journal, seal) =
            market.wait_for_request_fulfillment(request_id, Duration::from_secs(5), None).await?;
        return Ok(Some((journal, seal)));
    }
    Ok(None)
}

async fn make_request<T, P>(
    market: &ProofMarketService<T, P>,
    validator_index: u64,
    timestamp: u64,
    full_withdrawal: bool,
) -> Result<ProvingRequest>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    let current_block = market
        .instance()
        .provider()
        .get_block_number()
        .await
        .context("Failed to get block number")?;

    let offer = Offer {
        minPrice: parse_ether("0.001").unwrap().try_into()?,
        maxPrice: parse_ether("0.001").unwrap().try_into()?,
        biddingStart: current_block,
        rampUpPeriod: 0,
        timeout: 1000,
        lockinStake: parse_ether("0.001").unwrap().try_into()?,
    };

    // Upload the guest code to the default storage provider.
    // It Uses a temporary file storage provider if `RISC0_DEV_MODE` is set;
    // or if you'd like to use Pinata or S3 instead, you can set the appropriate env variables.
    let storage_provider = storage_provider_from_env().await?;
    let elf_url = storage_provider.upload_image(EIGEN_WITHDRAWAL_ELF).await?;
    let image_id = hex::encode(compute_image_id(EIGEN_WITHDRAWAL_ELF)?);
    info!("Image ID: {}", image_id);

    let mut request = ProvingRequest::new(
        market.gen_random_id().await?,
        &market.caller(),
        Requirements {
            imageId: B256::from_hex(image_id).expect("Invalid image_id hex"),
            predicate: Predicate {
                predicateType: PredicateType::PrefixMatch,
                data: Bytes::default(),
            },
        },
        &elf_url,
        Input { inputType: InputType::Inline, data: Bytes::default() },
        offer,
    );

    let proof = mock_proof(validator_index, B256::default(), 0, timestamp, full_withdrawal);

    // Prepare input data and upload it.
    let input_data = to_vec(&proof).unwrap();
    let encoded_input = bytemuck::cast_slice(&input_data).to_vec();
    let input_url = storage_provider.upload_input(&encoded_input).await?;
    info!("Input URL: {}", input_url);
    request.input = Input { inputType: InputType::Url, data: input_url.into() };

    Ok(request)
}

async fn get_status<T, P>(market: ProofMarketService<T, P>, request_id: U256) -> Result<ProofStatus>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    Ok(market.get_status(request_id).await?)
}
