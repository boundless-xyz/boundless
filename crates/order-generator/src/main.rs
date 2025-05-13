// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{path::PathBuf, time::Duration};

use alloy::{
    network::EthereumWallet,
    primitives::{
        utils::{format_units, parse_ether},
        Address, U256,
    },
    providers::Provider,
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Result};
use boundless_market::{
    balance_alerts_layer::BalanceAlertConfig,
    client::ClientBuilder,
    contracts::{Input, Offer, Predicate, ProofRequest, Requirements},
    input::InputBuilder,
    storage::{
        storage_provider_from_config, storage_provider_from_env, BuiltinStorageProvider,
        StorageProviderConfig,
    },
};
use clap::Parser;
use rand::Rng;
use risc0_zkvm::{compute_image_id, default_executor, sha::Digestible};
use url::Url;

/// Arguments of the order generator.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Optional URL of the offchain order stream endpoint.
    ///
    /// If set, the order-generator will submit requests off-chain.
    #[clap(short, long, env)]
    order_stream_url: Option<Url>,
    /// Private key used to sign and submit requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the SetVerifier contract.
    #[clap(short, long, env)]
    set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// Interval in seconds between requests.
    #[clap(short, long, default_value = "60")]
    interval: u64,
    // Storage provider to use.
    #[clap(flatten)]
    storage_config: Option<StorageProviderConfig>,
    /// Optional number of requests to submit.
    ///
    /// If unspecified, the loop will run indefinitely.
    #[clap(short, long)]
    count: Option<u64>,
    /// Minimum price per mcycle in ether.
    #[clap(long = "min", value_parser = parse_ether, default_value = "0.001")]
    min_price_per_mcycle: U256,
    /// Maximum price per mcycle in ether.
    #[clap(long = "max", value_parser = parse_ether, default_value = "0.002")]
    max_price_per_mcycle: U256,
    /// Lockin stake amount in ether.
    #[clap(short, long, value_parser = parse_ether, default_value = "0.0")]
    lockin_stake: U256,
    /// Number of seconds, from the current time, before the auction period starts.
    #[clap(long, default_value = "30")]
    bidding_start_delay: u64,
    /// Ramp-up period in seconds.
    ///
    /// The bid price will increase linearly from `min_price` to `max_price` over this period.
    #[clap(long, default_value = "240")] // 240s = ~20 Sepolia blocks
    ramp_up: u32,
    /// Number of seconds before the request lock-in expires.
    #[clap(long, default_value = "900")]
    lock_timeout: u32,
    /// Number of seconds before the request expires.
    #[clap(long, default_value = "1800")]
    timeout: u32,
    /// Additional time in seconds to add to the timeout for each 1M cycles.
    #[clap(long, default_value = "60")]
    seconds_per_mcycle: u32,
    /// Program binary file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included echo guest.
    #[clap(long)]
    program: Option<PathBuf>,
    /// The cycle count to drive the loop.
    ///
    /// If unspecified, defaults to a random value.
    #[clap(long)]
    input: Option<u32>,
    /// Balance threshold at which to log a warning.
    #[clap(long, value_parser = parse_ether, default_value = "1")]
    warn_balance_below: Option<U256>,
    /// Balance threshold at which to log an error.
    #[clap(long, value_parser = parse_ether, default_value = "0.1")]
    error_balance_below: Option<U256>,
    /// When submitting offchain, auto-deposits an amount in ETH when market balance is below this value.
    ///
    /// This parameter can only be set if order_stream_url is provided.
    #[clap(long, value_parser = parse_ether, requires = "order_stream_url")]
    auto_deposit: Option<U256>,
}

/// An estimated upper bound on the cost of locking and fulfilling a request.
/// TODO: Make this configurable.
const LOCK_FULFILL_GAS_UPPER_BOUND: u128 = 1_000_000;

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

    let args = MainArgs::parse();

    // NOTE: Using a separate `run` function to facilitate testing below.
    run(&args).await?;

    Ok(())
}

async fn run(args: &MainArgs) -> Result<()> {
    let wallet = EthereumWallet::from(args.private_key.clone());
    let balance_alerts = BalanceAlertConfig {
        watch_address: wallet.default_signer().address(),
        warn_threshold: args.warn_balance_below,
        error_threshold: args.error_balance_below,
    };

    let storage_provider = match &args.storage_config {
        Some(storage_config) => storage_provider_from_config(storage_config).await?,
        None => storage_provider_from_env().await?,
    };

    let boundless_client = ClientBuilder::<BuiltinStorageProvider>::new()
        .with_rpc_url(args.rpc_url.clone())
        .with_storage_provider(Some(storage_provider))
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.set_verifier_address)
        .with_order_stream_url(args.order_stream_url.clone())
        .with_private_key(args.private_key.clone())
        .with_bidding_start_delay(args.bidding_start_delay)
        .with_balance_alerts(balance_alerts)
        .build()
        .await?;

    let program = match &args.program {
        Some(path) => std::fs::read(path)?,
        None => {
            // A build of the loop guest, which simply loop until reaching the cycle count it reads from inputs and commits to it.
            let url = "https://gateway.pinata.cloud/ipfs/bafkreifpofz3uvc3vq7t2k2o66b2duwvmfab32js7dvn7jldpypwjqisyq";
            fetch_http(&Url::parse(url)?).await?
        }
    };
    let image_id = compute_image_id(&program)?;

    let program_url = boundless_client.upload_program(&program).await?;
    tracing::info!("Uploaded program to {}", program_url);

    let mut i = 0u64;
    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }

        let input = match args.input {
            Some(input) => input,
            None => {
                // Generate a random input.
                let mut rng = rand::rng();
                let input: u32 = rng.random_range(1_000_000..=1_000_000_000);
                tracing::debug!("Generated random input: {}", input);
                input
            }
        };

        let env = InputBuilder::new().write(&(input as u64))?.build_env()?;
        let session_info = default_executor().execute(env.clone().try_into()?, &program)?;
        let journal = session_info.journal;

        let cycles_count =
            session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
        let min_price = args
            .min_price_per_mcycle
            .checked_mul(U256::from(cycles_count))
            .unwrap()
            .div_ceil(U256::from(1_000_000));
        let mcycle_max_price = args
            .max_price_per_mcycle
            .checked_mul(U256::from(cycles_count))
            .unwrap()
            .div_ceil(U256::from(1_000_000));
        let m_cycles = input.div_ceil(1_000_000);
        // add 1 minute for each 1M cycles to the original timeout
        let timeout = args.timeout + args.seconds_per_mcycle.checked_mul(m_cycles).unwrap();
        let lock_timeout =
            args.lock_timeout + args.seconds_per_mcycle.checked_mul(m_cycles).unwrap();

        // Add to the max price an estimated upper bound on the gas costs.
        // Note that the auction will allow us to pay the lowest price a prover will accept.
        // Add a 10% buffer to the gas costs to account for flucuations after submission.
        let gas_price: u128 = boundless_client.provider().get_gas_price().await?;
        let gas_cost_estimate = (gas_price + (gas_price / 10)) * LOCK_FULFILL_GAS_UPPER_BOUND;
        let max_price = mcycle_max_price + U256::from(gas_cost_estimate);
        tracing::info!(
            "Setting a max price of {} ether: {} mcycle_price + {} gas_cost_estimate",
            format_units(max_price, "ether")?,
            format_units(mcycle_max_price, "ether")?,
            format_units(gas_cost_estimate, "ether")?,
        );

        tracing::info!(
            "{} cycles count {} min_price in ether {} max_price in ether",
            cycles_count,
            format_units(min_price, "ether")?,
            format_units(max_price, "ether")?
        );

        let request = ProofRequest::builder()
            .with_image_url(program_url.clone())
            .with_input(Input::inline(env.encode()?))
            .with_requirements(Requirements::new(
                image_id,
                Predicate::digest_match(journal.digest()),
            ))
            .with_offer(
                Offer::default()
                    .with_min_price(min_price)
                    .with_max_price(max_price)
                    .with_lock_stake(args.lockin_stake)
                    .with_ramp_up_period(args.ramp_up)
                    .with_timeout(timeout)
                    .with_lock_timeout(lock_timeout),
            )
            .build()?;

        tracing::info!("Request: {:?}", request);

        let submit_offchain = args.order_stream_url.is_some();

        // Check balance and auto-deposit if needed. Only necessary if submitting offchain, since onchain submission automatically deposits
        // in the submitRequest call.
        if submit_offchain {
            if let Some(auto_deposit) = args.auto_deposit {
                let market = boundless_client.boundless_market.clone();
                let caller = boundless_client.caller();
                let balance = market.balance_of(caller).await?;
                tracing::info!(
                    "Caller {} has balance {} ETH on market {}",
                    caller,
                    format_units(balance, "ether")?,
                    args.boundless_market_address
                );
                if balance < auto_deposit {
                    tracing::info!(
                        "Balance {} ETH is below auto-deposit threshold {} ETH, depositing...",
                        format_units(balance, "ether")?,
                        format_units(auto_deposit, "ether")?
                    );
                    market.deposit(auto_deposit).await?;
                    tracing::info!(
                        "Successfully deposited {} ETH",
                        format_units(auto_deposit, "ether")?
                    );
                }
            }
        }

        let (request_id, _) = if submit_offchain {
            boundless_client.submit_request_offchain(&request).await?
        } else {
            boundless_client.submit_request(&request).await?
        };

        if submit_offchain {
            tracing::info!(
                "Request 0x{request_id:x} submitted offchain to {}",
                args.order_stream_url.clone().unwrap()
            );
        } else {
            tracing::info!(
                "Request 0x{request_id:x} submitted onchain to {}",
                args.boundless_market_address
            );
        }

        i += 1;
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }

    Ok(())
}

async fn fetch_http(url: &Url) -> Result<Vec<u8>> {
    let response = reqwest::get(url.as_str()).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP request failed with status: {}", status);
    }

    Ok(response.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil, providers::Provider, rpc::types::Filter, sol_types::SolEvent,
    };
    use boundless_market::{contracts::IBoundlessMarket, storage::StorageProviderConfig};
    use boundless_market_test_utils::create_test_ctx;
    use tracing_test::traced_test;

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        let args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            order_stream_url: None,
            storage_config: Some(StorageProviderConfig::dev_mode()),
            private_key: ctx.customer_signer,
            set_verifier_address: ctx.set_verifier_address,
            boundless_market_address: ctx.boundless_market_address,
            interval: 1,
            count: Some(2),
            min_price_per_mcycle: parse_ether("0.001").unwrap(),
            max_price_per_mcycle: parse_ether("0.002").unwrap(),
            lockin_stake: parse_ether("0.0").unwrap(),
            bidding_start_delay: 30,
            ramp_up: 0,
            timeout: 1000,
            lock_timeout: 1000,
            seconds_per_mcycle: 60,
            program: None,
            input: None,
            warn_balance_below: None,
            error_balance_below: None,
            auto_deposit: None,
        };

        run(&args).await.unwrap();

        // Check that the requests were submitted
        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(0)
            .address(ctx.boundless_market_address);
        let logs = ctx.customer_provider.get_logs(&filter).await.unwrap();
        let decoded_logs = logs.iter().filter_map(|log| {
            match log.log_decode::<IBoundlessMarket::RequestSubmitted>() {
                Ok(res) => Some(res),
                Err(err) => {
                    tracing::error!("Failed to decode RequestSubmitted log: {err:?}");
                    None
                }
            }
        });
        assert!(decoded_logs.count() == 2);
    }
}
