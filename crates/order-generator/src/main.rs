// Copyright 2026 Boundless Foundation, Inc.
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

use std::{path::PathBuf, time::Duration};

use alloy::{
    network::EthereumWallet,
    primitives::{
        utils::{format_units, parse_ether},
        U256,
    },
    providers::DynProvider,
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use boundless_market::{
    balance_alerts_layer::BalanceAlertConfig,
    client::{Client, FundingMode},
    deployments::Deployment,
    input::GuestEnv,
    request_builder::StandardRequestBuilder,
    storage::{HttpDownloader, StandardDownloader, StorageDownloader},
    StandardUploader, StorageUploaderConfig,
};
use clap::Parser;
use rand::Rng;
use risc0_zkvm::Journal;
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;

/// Arguments of the order generator.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Option<Url>,
    /// Additional RPC URLs for automatic failover.
    #[clap(long, env = "RPC_URLS", value_delimiter = ',')]
    rpc_urls: Option<Vec<Url>>,
    /// Private key used to sign and submit requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Transaction timeout in seconds.
    #[clap(long, default_value = "45")]
    tx_timeout: u64,
    /// When submitting offchain, auto-deposits an amount in ETH when market balance is below this value.
    ///
    /// This parameter can only be set if order_stream_url is provided.
    #[clap(long, env, value_parser = parse_ether)]
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
    lock_collateral_raw: U256,
    /// Number of seconds, from the current time, before the auction period starts.
    /// If not provided, will be calculated based on cycle count assuming 5 MHz prove rate.
    #[clap(long)]
    bidding_start_delay: Option<u64>,
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
    /// Additional time in seconds to add to the ramp-up period for each 1M cycles.
    #[clap(long, default_value = "20")]
    ramp_up_seconds_per_mcycle: u32,
    /// Execution rate in kHz for calculating bidding start delays.
    /// Default is 2000 kHz (2 MHz).
    #[clap(long, default_value = "2000", env)]
    exec_rate_khz: u64,
    /// Program binary file to use as the guest image, given as a path.
    ///
    /// If unspecified, defaults to the included loop guest.
    #[clap(long)]
    program: Option<PathBuf>,
    /// The cycle count to drive the loop.
    ///
    /// If unspecified, defaults to a random value between 10_000_000 and 1_000_000_000
    /// with a step of 1_000_000.
    #[clap(long, env = "CYCLE_COUNT")]
    input: Option<u64>,
    /// The maximum cycle count to drive the loop.
    #[clap(long, env = "CYCLE_COUNT_MAX", conflicts_with_all = ["input", "program"])]
    input_max_mcycles: Option<u64>,
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

    /// Configuration for the uploader used for programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Uploader")]
    storage_config: StorageUploaderConfig,

    /// Whether to use the Zeth guest.
    #[clap(long, default_value = "false")]
    use_zeth: bool,
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

    let mut client = Client::builder();
    if let Some(rpc_url) = &args.rpc_url {
        client = client.with_rpc_url(rpc_url.clone());
    }
    if let Some(rpc_urls) = &args.rpc_urls {
        client = client.with_rpc_urls(rpc_urls.clone());
    }

    let client = client
        .with_uploader_config(&args.storage_config)
        .await?
        .with_deployment(args.deployment.clone())
        .with_private_key(args.private_key.clone())
        .with_balance_alerts(balance_alerts)
        .with_timeout(Some(Duration::from_secs(args.tx_timeout)))
        .with_funding_mode(FundingMode::BelowThreshold(parse_ether("0.01").unwrap()))
        .build()
        .await?;

    let ipfs_gateway = args
        .storage_config
        .ipfs_gateway_url
        .clone()
        .unwrap_or(Url::parse("https://gateway.pinata.cloud").unwrap());
    // Ensure we have both a program and a program URL.
    let (program, program_url, input) = match args
        .program
        .as_ref()
        .map(std::fs::read)
        .transpose()?
    {
        Some(program) => {
            let program_url = client.upload_program(&program).await?;
            tracing::info!("Uploaded program to {}", program_url);
            (program, program_url, None)
        }
        None => {
            let (program_url, input) = match args.use_zeth {
                true => {
                    let program_url = ipfs_gateway
                        // A build of the zeth guest, which simply iterate block validation until the iterations are reached.
                        .join("/ipfs/bafybeidxu26oi63himx2hn5vbhmwc3t6rabqmkpl4i7nl7daucvcvq6cge")
                        .unwrap();
                    let input = HttpDownloader::default().download_url(ipfs_gateway.join("/ipfs/bafybeiac26qnu67cqlklukxdtr5cvovbdss4dx64ywi2yx53mw3ykwmufe").unwrap()).await?;
                    (program_url, Some(input))
                }
                false => (
                    ipfs_gateway
                        // A build of the loop guest, which simply loop until reaching the cycle count it reads from inputs and commits to it.
                        .join("/ipfs/bafkreicmwk3xlxbozbp5h63xyywocc7dltt376hn4mnmhk7ojqdcbrkqzi")
                        .unwrap(),
                    None,
                ),
            };
            let program = HttpDownloader::default().download_url(program_url.clone()).await?;
            (program, program_url, input)
        }
    };

    let mut i = 0u64;
    loop {
        if let Some(count) = args.count {
            if i >= count {
                break;
            }
        }
        if let Err(e) = match args.use_zeth {
            true => {
                if let Some(input) = input.as_ref() {
                    handle_zeth_request(args, &client, &program, &program_url, input).await
                } else {
                    let error_msg = "Zeth input is not provided".to_string();
                    tracing::error!("Request failed: {error_msg}");
                    return Err(anyhow::anyhow!(error_msg));
                }
            }
            false => handle_loop_request(args, &client, &program, &program_url).await,
        } {
            let error_msg = format!("{e:?}");
            if error_msg.contains("Transaction confirmation error") {
                tracing::error!("[B-OG-CONF] Transaction confirmation error: {e:?}");
            } else {
                tracing::error!("Request failed: {e:?}");
            }
        }
        i += 1;
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }

    Ok(())
}

async fn handle_loop_request(
    args: &MainArgs,
    client: &Client<
        DynProvider,
        StandardUploader,
        StandardDownloader,
        StandardRequestBuilder<DynProvider, StandardUploader, StandardDownloader>,
        PrivateKeySigner,
    >,
    program: &[u8],
    program_url: &url::Url,
) -> Result<()> {
    let mut rng = rand::rng();
    let nonce: u64 = rng.random();
    let input = match args.input {
        Some(input) => input,
        None => {
            // Generate a random input.
            let max = args.input_max_mcycles.unwrap_or(1000);
            let input: u64 = rand::rng().random_range(10..=max) << 20;
            tracing::debug!("Generated random cycle count: {}", input);
            input
        }
    };
    let env = GuestEnv::builder().write(&(input as u64))?.write(&nonce)?.build_env();

    let m_cycles = input >> 20;

    // Provide journal and cycles in order to skip preflighting, allowing us to send requests faster.
    let journal = Journal::new([input.to_le_bytes(), nonce.to_le_bytes()].concat());

    let request = client
        .new_request()
        .with_program(program.to_vec())
        .with_program_url(program_url.clone())?
        .with_env(env)
        .with_cycles(input)
        .with_journal(journal);

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

    // Check balance and auto-deposit if needed for both onchain and offchain submissions
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
    Ok(())
}

async fn handle_zeth_request(
    args: &MainArgs,
    client: &Client<
        DynProvider,
        StandardUploader,
        StandardDownloader,
        StandardRequestBuilder<DynProvider, StandardUploader, StandardDownloader>,
        PrivateKeySigner,
    >,
    program: &[u8],
    program_url: &url::Url,
    input: &[u8],
) -> Result<()> {
    let mut rng = rand::rng();
    let nonce: u32 = rng.random_range(0..=u32::MAX);
    let max_cycles = args.input_max_mcycles.unwrap_or(3000) << 20;
    let max_iterations = max_cycles.div_ceil(1000000000); // ~1B cycles per iteration
    let iterations: u32 = rng.random_range(1..=max_iterations as u32);
    tracing::info!("Iterations: {}", iterations);
    let env = GuestEnv::builder().write_slice(input).write(&iterations)?.write(&nonce)?.build_env();
    let request = client
        .new_request()
        .with_program(program.to_vec())
        .with_program_url(program_url.clone())?
        .with_env(env);

    // Build the request, including preflight, and assigned the remaining fields.
    let request = client.build_request(request).await?;

    tracing::info!("Request: {:?}", request);

    tracing::info!(
        "{} min_price in ether {} max_price in ether",
        format_units(request.offer.minPrice, "ether")?,
        format_units(request.offer.maxPrice, "ether")?
    );

    let submit_offchain = args.submit_offchain;

    // Check balance and auto-deposit if needed for both onchain and offchain submissions
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil, providers::Provider, rpc::types::Filter, sol_types::SolEvent,
    };
    use boundless_market::contracts::IBoundlessMarket;
    use boundless_test_utils::{guests::LOOP_PATH, market::create_test_ctx};
    use tracing_test::traced_test;

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        let args = MainArgs {
            rpc_url: Some(anvil.endpoint_url()),
            rpc_urls: Some(Vec::new()),
            storage_config: StorageUploaderConfig::dev_mode(),
            private_key: ctx.customer_signer,
            deployment: Some(ctx.deployment.clone()),
            interval: 1,
            count: Some(2),
            min_price_per_mcycle: parse_ether("0.001").unwrap(),
            max_price_per_mcycle: parse_ether("0.002").unwrap(),
            lock_collateral_raw: parse_ether("0.0").unwrap(),
            bidding_start_delay: None,
            ramp_up: 0,
            timeout: 1000,
            lock_timeout: 1000,
            seconds_per_mcycle: 60,
            ramp_up_seconds_per_mcycle: 60,
            exec_rate_khz: 5000,
            program: Some(LOOP_PATH.parse().unwrap()),
            input: None,
            input_max_mcycles: None,
            warn_balance_below: None,
            error_balance_below: None,
            auto_deposit: None,
            tx_timeout: 45,
            submit_offchain: false,
            use_zeth: false,
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
