// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{path::PathBuf, time::Duration};

use alloy::{
    network::EthereumWallet,
    primitives::{
        utils::{format_units, parse_ether},
        U256,
    },
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use boundless_market::{
    balance_alerts_layer::BalanceAlertConfig, client::Client, deployments::Deployment,
    input::GuestEnv, request_builder::OfferParams, storage::fetch_url,
    storage::StorageProviderConfig,
};
use clap::Parser;
use rand::Rng;
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

/// Arguments of the order generator.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to sign and submit requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Transaction timeout in seconds.
    #[clap(long, default_value = "45")]
    tx_timeout: u64,
    /// When submitting offchain, auto-deposits an amount in ETH when market balance is below this value.
    ///
    /// This parameter can only be set if order_stream_url is provided.
    #[clap(long, env, value_parser = parse_ether, requires = "submit_offchain")]
    auto_deposit: Option<U256>,
    /// Interval in seconds between requests.
    #[clap(short, long, default_value = "60")]
    interval: u64,
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
    #[clap(short, long, default_value = "0")]
    lock_stake_raw: U256,
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
    #[clap(long, default_value = "20")]
    seconds_per_mcycle: u32,
    /// Program binary file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included echo guest.
    #[clap(long)]
    program: Option<PathBuf>,
    /// The cycle count to drive the loop.
    ///
    /// If unspecified, defaults to a random value between 1_000_000 and 1_000_000_000
    /// with a step of 1_000_000.
    #[clap(long, env = "CYCLE_COUNT")]
    input: Option<u64>,
    /// Balance threshold at which to log a warning.
    #[clap(long, value_parser = parse_ether, default_value = "1")]
    warn_balance_below: Option<U256>,
    /// Balance threshold at which to log an error.
    #[clap(long, value_parser = parse_ether, default_value = "0.1")]
    error_balance_below: Option<U256>,

    /// Boundless Market deployment configuration
    #[clap(flatten, next_help_heading = "Boundless Market Deployment")]
    deployment: Option<Deployment>,

    /// Submit requests offchain.
    #[clap(long, default_value = "false")]
    submit_offchain: bool,

    /// Storage provider to use.
    #[clap(flatten, next_help_heading = "Storage Provider")]
    storage_config: StorageProviderConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(false)
        .with_span_events(FmtSpan::CLOSE)
        .json()
        .init();

    let args = MainArgs::parse();

    // NOTE: Using a separate `run` function to facilitate testing below.
    let result = run(&args).await;
    if let Err(e) = result {
        tracing::error!("FATAL: {:?}", e);
    }

    Ok(())
}

async fn run(args: &MainArgs) -> Result<()> {
    let wallet = EthereumWallet::from(args.private_key.clone());
    let balance_alerts = BalanceAlertConfig {
        watch_address: wallet.default_signer().address(),
        warn_threshold: args.warn_balance_below,
        error_threshold: args.error_balance_below,
    };

    let client = Client::builder()
        .with_rpc_url(args.rpc_url.clone())
        .with_storage_provider_config(&args.storage_config)?
        .with_deployment(args.deployment.clone())
        .with_private_key(args.private_key.clone())
        .with_balance_alerts(balance_alerts)
        .with_timeout(Some(Duration::from_secs(args.tx_timeout)))
        .config_offer_layer(|config| {
            config
                .min_price_per_cycle(args.min_price_per_mcycle >> 20)
                .max_price_per_cycle(args.max_price_per_mcycle >> 20)
                .bidding_start_delay(args.bidding_start_delay)
        })
        .build()
        .await?;

    // Ensure we have both a program and a program URL.
    let program = args.program.as_ref().map(std::fs::read).transpose()?;
    let program_url = match program {
        Some(ref program) => {
            let program_url = client.upload_program(program).await?;
            tracing::info!("Uploaded program to {}", program_url);
            program_url
        }
        None => {
            // A build of the loop guest, which simply loop until reaching the cycle count it reads from inputs and commits to it.
            Url::parse("https://gateway.pinata.cloud/ipfs/bafkreicmwk3xlxbozbp5h63xyywocc7dltt376hn4mnmhk7ojqdcbrkqzi").unwrap()
        }
    };
    let program = match program {
        None => fetch_url(&program_url).await?,
        Some(program) => program,
    };

    let mut i = 0u64;
    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }

        let mut rng = rand::rng();
        let nonce: u64 = rng.random();
        let input = match args.input {
            Some(input) => input,
            None => {
                // Generate a random input.
                let input: u64 = rand::rng().random_range(1..=1000) << 20;
                tracing::debug!("Generated random cycle count: {}", input);
                input
            }
        };
        let env = GuestEnv::builder().write(&(input as u64))?.write(&nonce)?.build_env();

        // add 1 minute for each 1M cycles to the original timeout
        // Use the input directly as the estimated cycle count, since we are using a loop program.
        let m_cycles = input >> 20;
        let lock_timeout =
            args.lock_timeout + args.seconds_per_mcycle.checked_mul(m_cycles as u32).unwrap();
        // Give equal time for provers that are fulfilling after lock expiry to prove.
        let timeout: u32 = args.timeout
            + lock_timeout
            + args.seconds_per_mcycle.checked_mul(m_cycles as u32).unwrap();

        let request = client
            .new_request()
            .with_program(program.clone())
            .with_program_url(program_url.clone())?
            .with_env(env)
            .with_offer(
                OfferParams::builder()
                    .ramp_up_period(args.ramp_up)
                    .lock_timeout(lock_timeout)
                    .timeout(timeout)
                    .lock_stake(args.lock_stake_raw),
            );

        // Build the request, including preflight, and assigned the remaining fields.
        let request = client.build_request(request).await?;

        tracing::info!("Request: {:?}", request);

        tracing::info!(
            "{} Mcycles count {} min_price in ether {} max_price in ether",
            m_cycles,
            format_units(request.offer.minPrice, "ether")?,
            format_units(request.offer.maxPrice, "ether")?
        );

        let submit_offchain = args.submit_offchain;

        // Check balance and auto-deposit if needed. Only necessary if submitting offchain, since onchain submission automatically deposits
        // in the submitRequest call.
        if submit_offchain {
            if let Some(auto_deposit) = args.auto_deposit {
                let market = client.boundless_market.clone();
                let caller = client.caller();
                let balance = market.balance_of(caller).await?;
                tracing::info!(
                    "Caller {} has balance {} ETH on market {}. Auto-deposit threshold is {} ETH",
                    caller,
                    format_units(balance, "ether")?,
                    client.deployment.boundless_market_address,
                    format_units(auto_deposit, "ether")?
                );
                if balance < auto_deposit {
                    tracing::info!(
                        "Balance {} ETH is below auto-deposit threshold {} ETH, depositing...",
                        format_units(balance, "ether")?,
                        format_units(auto_deposit, "ether")?
                    );
                    match market.deposit(auto_deposit).await {
                        Ok(_) => {
                            tracing::info!(
                                "Successfully deposited {} ETH",
                                format_units(auto_deposit, "ether")?
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Failed to auto deposit ETH: {e:?}");
                        }
                    }
                }
            }
        }

        let (request_id, _) = if submit_offchain {
            client.submit_request_offchain(&request).await?
        } else {
            client.submit_request_onchain(&request).await?
        };

        if submit_offchain {
            tracing::info!(
                "Request 0x{request_id:x} submitted offchain to {}",
                client.deployment.order_stream_url.clone().unwrap()
            );
        } else {
            tracing::info!(
                "Request 0x{request_id:x} submitted onchain to {}",
                client.deployment.boundless_market_address,
            );
        }

        i += 1;
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }

    Ok(())
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
            storage_config: StorageProviderConfig::dev_mode(),
            private_key: ctx.customer_signer,
            deployment: Some(ctx.deployment.clone()),
            interval: 1,
            count: Some(2),
            min_price_per_mcycle: parse_ether("0.001").unwrap(),
            max_price_per_mcycle: parse_ether("0.002").unwrap(),
            lock_stake_raw: parse_ether("0.0").unwrap(),
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
            tx_timeout: 45,
            submit_offchain: false,
        };

        run(&args).await.unwrap();

        // Check that the requests were submitted
        let filter = Filter::new()
            .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
            .from_block(0)
            .address(ctx.deployment.boundless_market_address);
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
