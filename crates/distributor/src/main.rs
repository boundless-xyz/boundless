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

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{
        utils::{format_units, parse_ether, parse_units},
        Address, U256,
    },
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol,
};
use anyhow::{Context, Result};
use boundless_market::{
    client::Client,
    deployments::collateral_token_supports_permit,
    indexer_client::{IndexerClient, ProverLeaderboardEntry},
    BoundlessMarketService, Deployment,
};
use chrono::Utc;
use clap::Parser;
use serde::{Deserialize, Serialize};
use url::Url;

const TX_TIMEOUT: Duration = Duration::from_secs(180);
const CHAINALYSIS_API_URL: &str = "https://public.chainalysis.com";

sol! {
    #[sol(rpc)]
    contract IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function balanceOf(address owner) external view returns (uint256);
    }
}

/// Chainalysis Sanctions API response.
#[derive(Deserialize, Debug)]
struct SanctionsResponse {
    identifications: Vec<SanctionsIdentification>,
}

#[derive(Deserialize, Debug)]
struct SanctionsIdentification {
    #[allow(dead_code)]
    category: Option<String>,
}

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
    /// List of prover private keys
    #[clap(long, env, value_delimiter = ',')]
    prover_keys: Vec<PrivateKeySigner>,
    /// List of order generator private keys
    #[clap(long, env, value_delimiter = ',')]
    order_generator_keys: Vec<PrivateKeySigner>,
    /// List of offchain requestor addresses (these will have ETH deposited to market)
    #[clap(long, env, value_delimiter = ',')]
    offchain_requestor_addresses: Vec<Address>,
    /// Slasher private key
    #[clap(long, env)]
    slasher_key: PrivateKeySigner,
    /// If prover ETH balance is above this threshold, transfer 80% of the ETH to distributor
    #[clap(long, env, default_value = "1.0")]
    prover_eth_donate_threshold: String,
    /// If prover collateral balance is above this threshold, transfer 60% of the collateral to distributor
    #[clap(long, env, default_value = "100.0")]
    prover_stake_donate_threshold: String,
    /// If ETH balance is below this threshold, transfer ETH to address
    #[clap(long, env, default_value = "0.1")]
    eth_threshold: String,
    /// If collateral balance is below this threshold, transfer collateral to address
    #[clap(long, env, default_value = "1.0")]
    stake_threshold: String,
    /// Amount of ETH to transfer from distributor to account during top up
    #[clap(long, env, default_value = "0.1")]
    eth_top_up_amount: String,
    /// Amount of collateral to transfer from distributor to prover during top up
    #[clap(long, env, default_value = "10")]
    stake_top_up_amount: String,
    /// If distributor ETH balance is below this threshold, emit alert log
    #[clap(long, env, default_value = "0.5")]
    distributor_eth_alert_threshold: String,
    /// If distributor collateral balance is below this threshold, emit alert log
    #[clap(long, env, default_value = "200.0")]
    distributor_stake_alert_threshold: String,
    /// Deployment to use
    #[clap(flatten, next_help_heading = "Boundless Market Deployment")]
    deployment: Option<Deployment>,

    // --- External prover top-up args ---
    /// Enable external prover collateral top-ups. Requires --indexer-api-url.
    #[clap(long, env)]
    enable_external_topup: bool,
    /// Indexer API base URL (required when --enable-external-topup is set).
    #[clap(long, env)]
    indexer_api_url: Option<Url>,
    /// Minimum 7D billion-cycles for a prover to qualify for external top-up
    #[clap(long, env, default_value = "1")]
    min_bcycles_threshold: u64,
    /// Chainalysis Sanctions API key (leave empty to skip OFAC screening, e.g. on testnets)
    #[clap(long, env = "CHAINALYSIS_API_KEY", default_value = "")]
    chainalysis_api_key: String,
    /// Chainalysis Sanctions API base URL override (test only; defaults to the public endpoint)
    #[clap(skip = CHAINALYSIS_API_URL.to_string())]
    chainalysis_api_url: String,
    /// ZKC balance below which an external prover gets topped up (human-readable, e.g. "5")
    #[clap(long, env, default_value = "5")]
    external_collateral_threshold: String,
    /// ZKC per top-up for external provers (human-readable, e.g. "10")
    #[clap(long, env, default_value = "10")]
    external_per_top_up_amount: String,
    /// Total lifetime ZKC allowance per external prover (human-readable, e.g. "100")
    #[clap(long, env, default_value = "100")]
    external_lifetime_allowance: String,
    /// Path to JSON state file tracking per-prover lifetime allowances
    #[clap(long, env, default_value = "./topup-state.json")]
    allowance_state_file: PathBuf,
}

/// Per-prover lifetime allowance tracking.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
struct ProverAllowance {
    /// Total ZKC sent to this prover (raw U256 string).
    total_sent: String,
    /// Last top-up timestamp (ISO 8601).
    last_top_up: Option<String>,
    /// Whether the address was flagged as sanctioned.
    sanctioned: bool,
}

/// State file tracking all external prover allowances.
#[derive(Serialize, Deserialize, Default, Debug)]
struct AllowanceState {
    provers: HashMap<String, ProverAllowance>,
}

fn load_state(path: &std::path::Path) -> Result<AllowanceState> {
    if path.exists() {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read state file {}", path.display()))?;
        serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse state file {}", path.display()))
    } else {
        Ok(AllowanceState::default())
    }
}

fn save_state(path: &std::path::Path, state: &AllowanceState) -> Result<()> {
    let data = serde_json::to_string_pretty(state).context("Failed to serialize state")?;
    std::fs::write(path, data)
        .with_context(|| format!("Failed to write state file {}", path.display()))
}

fn parse_total_sent(s: &str) -> U256 {
    if s.is_empty() {
        U256::ZERO
    } else {
        U256::from_str_radix(s, 10).unwrap_or_else(|e| {
            tracing::warn!("Corrupt total_sent value '{}': {}. Treating as zero.", s, e);
            U256::ZERO
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .with_target(false)
        .with_ansi(false)
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
    let distributor_wallet = EthereumWallet::from(args.private_key.clone());
    let distributor_address = distributor_wallet.default_signer().address();
    let distributor_provider =
        ProviderBuilder::new().wallet(distributor_wallet).connect_http(args.rpc_url.clone());

    tracing::info!("Using deployment: {:?}", args.deployment);
    let distributor_client = Client::builder()
        .with_rpc_url(args.rpc_url.clone())
        .with_private_key(args.private_key.clone())
        .with_deployment(args.deployment.clone())
        .with_timeout(Some(TX_TIMEOUT))
        .build()
        .await?;

    // Parse thresholds
    let prover_eth_donate_threshold = parse_ether(&args.prover_eth_donate_threshold)?;
    let collateral_token_decimals =
        distributor_client.boundless_market.collateral_token_decimals().await?;
    let prover_collateral_donate_threshold: U256 =
        parse_units(&args.prover_stake_donate_threshold, collateral_token_decimals)?.into();
    let eth_threshold = parse_ether(&args.eth_threshold)?;
    let collateral_threshold: U256 =
        parse_units(&args.stake_threshold, collateral_token_decimals)?.into();
    let eth_top_up_amount = parse_ether(&args.eth_top_up_amount)?;
    let collateral_top_up_amount: U256 =
        parse_units(&args.stake_top_up_amount, collateral_token_decimals)?.into();
    let distributor_eth_alert_threshold = parse_ether(&args.distributor_eth_alert_threshold)?;
    let distributor_collateral_alert_threshold: U256 =
        parse_units(&args.distributor_stake_alert_threshold, collateral_token_decimals)?.into();

    // check top up amounts are greater than thresholds
    if eth_top_up_amount < eth_threshold {
        tracing::error!("ETH top up amount is less than threshold");
        return Err(anyhow::anyhow!(
            "ETH top up amount is less than threshold [top up amount: {}, threshold: {}]",
            format_units(eth_top_up_amount, "ether")?,
            format_units(eth_threshold, "ether")?
        ));
    }
    if collateral_top_up_amount < collateral_threshold {
        tracing::error!("Collateral top up amount is less than threshold");
        return Err(anyhow::anyhow!(
            "Collateral top up amount is less than threshold [top up amount: {}, threshold: {}]",
            format_units(collateral_top_up_amount, collateral_token_decimals)?,
            format_units(collateral_threshold, collateral_token_decimals)?
        ));
    }
    let collateral_token = distributor_client.boundless_market.collateral_token_address().await?;

    tracing::info!("Distributor address: {}", distributor_address);
    tracing::info!("Collateral token address: {}", collateral_token);
    tracing::info!("Collateral token decimals: {}", collateral_token_decimals);

    // Check distributor's own balances and emit early warning alerts if low
    let distributor_eth_balance =
        distributor_client.provider().get_balance(distributor_address).await?;
    let collateral_token_contract = IERC20::new(collateral_token, distributor_provider.clone());
    let distributor_collateral_balance =
        collateral_token_contract.balanceOf(distributor_address).call().await?;

    if distributor_eth_balance < distributor_eth_alert_threshold {
        tracing::warn!(
            "[B-DIST-ETH-LOW]: Distributor {} ETH balance {} is below alert threshold {}",
            distributor_address,
            format_units(distributor_eth_balance, "ether")?,
            format_units(distributor_eth_alert_threshold, "ether")?
        );
    } else {
        tracing::info!(
            "Distributor ETH balance: {}. Alert threshold: {}",
            format_units(distributor_eth_balance, "ether")?,
            format_units(distributor_eth_alert_threshold, "ether")?
        );
    }

    if distributor_collateral_balance < distributor_collateral_alert_threshold {
        tracing::warn!(
            "[B-DIST-STK-LOW]: Distributor {} collateral balance {} is below alert threshold {}",
            distributor_address,
            format_units(distributor_collateral_balance, collateral_token_decimals)?,
            format_units(distributor_collateral_alert_threshold, collateral_token_decimals)?
        );
    } else {
        tracing::info!(
            "Distributor collateral balance: {}. Alert threshold: {}",
            format_units(distributor_collateral_balance, collateral_token_decimals)?,
            format_units(distributor_collateral_alert_threshold, collateral_token_decimals)?
        );
    }

    // Transfer ETH from provers to the distributor from provers if above threshold
    for prover_key in &args.prover_keys {
        let prover_wallet = EthereumWallet::from(prover_key.clone());
        let prover_provider =
            ProviderBuilder::new().wallet(prover_wallet.clone()).connect_http(args.rpc_url.clone());
        let prover_address = prover_wallet.default_signer().address();

        let prover_eth_balance = distributor_client.provider().get_balance(prover_address).await?;

        tracing::info!(
            "Prover {} has {} ETH balance. Threshold for donation to distributor is {}.",
            prover_address,
            format_units(prover_eth_balance, "ether")?,
            format_units(prover_eth_donate_threshold, "ether")?
        );

        if prover_eth_balance > prover_eth_donate_threshold {
            // Transfer 80% of the balance to the distributor (leave 20% for future gas)
            let transfer_amount =
                prover_eth_balance.saturating_mul(U256::from(8)).div_ceil(U256::from(10)); // Leave some for gas

            tracing::info!(
                "Transferring {} ETH from prover {} to distributor",
                format_units(transfer_amount, "ether")?,
                prover_address
            );

            let tx = TransactionRequest::default()
                .with_from(prover_address)
                .with_to(distributor_address)
                .with_value(transfer_amount);

            let pending_tx = match prover_provider.send_transaction(tx).await {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!(
                        "Failed to send ETH transfer transaction from prover {} to distributor: {:?}. Skipping.",
                        prover_address, e
                    );
                    continue;
                }
            };

            // Wait for the transaction to be confirmed
            let receipt = match pending_tx.with_timeout(Some(TX_TIMEOUT)).watch().await {
                Ok(receipt) => receipt,
                Err(e) => {
                    tracing::error!(
                        "Failed to watch ETH transfer transaction from prover {} to distributor: {:?}. Skipping.",
                        prover_address, e
                    );
                    continue;
                }
            };

            tracing::info!(
                "Transfer completed: {:x} from prover {} for {} ETH to distributor",
                receipt,
                prover_address,
                format_units(transfer_amount, "ether")?
            );
        }

        let prover_collateral_balance =
            distributor_client.boundless_market.balance_of_collateral(prover_address).await?;

        tracing::info!(
            "Prover {} has {} collateral balance on market. Threshold for donation to distributor is {}.",
            prover_address,
            format_units(prover_collateral_balance, collateral_token_decimals)?,
            format_units(prover_collateral_donate_threshold, collateral_token_decimals)?
        );

        if prover_collateral_balance > prover_collateral_donate_threshold {
            // Withdraw 60% of the collateral balance to the distributor (leave 40% for future lock collateral)
            let withdraw_amount =
                prover_collateral_balance.saturating_mul(U256::from(6)).div_ceil(U256::from(10));

            tracing::info!(
                "Withdrawing {} collateral from prover {} to distributor",
                format_units(withdraw_amount, collateral_token_decimals)?,
                prover_address
            );

            // Create prover client to withdraw collateral
            let prover_client = Client::builder()
                .with_rpc_url(args.rpc_url.clone())
                .with_private_key(prover_key.clone())
                .with_timeout(Some(TX_TIMEOUT))
                .with_deployment(args.deployment.clone())
                .build()
                .await?;

            // Withdraw collateral from market to prover
            if let Err(e) =
                prover_client.boundless_market.withdraw_collateral(withdraw_amount).await
            {
                tracing::error!(
                    "Failed to withdraw collateral from boundless market for prover {}: {:?}. Skipping.",
                    prover_address,
                    e
                );
                continue;
            }

            tracing::info!(
                "Withdrawn {} collateral from market for prover {}. Now transferring to distributor",
                format_units(withdraw_amount, collateral_token_decimals)?,
                prover_address
            );

            // Transfer the withdrawn collateral to distributor
            let collateral_token_contract = IERC20::new(collateral_token, prover_provider.clone());

            let pending_tx = match collateral_token_contract
                .transfer(distributor_address, withdraw_amount)
                .send()
                .await
            {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!(
                        "Failed to send collateral transfer transaction from prover {} to distributor: {:?}. Skipping.",
                        prover_address, e
                    );
                    continue;
                }
            };

            if let Err(e) = pending_tx.with_timeout(Some(TX_TIMEOUT)).watch().await {
                tracing::error!(
                    "Failed to watch collateral transfer transaction from prover {} to distributor: {:?}. Skipping.",
                    prover_address, e
                );
                continue;
            }

            tracing::info!(
                "Collateral transfer completed from prover {} for {} collateral to distributor",
                prover_address,
                format_units(withdraw_amount, collateral_token_decimals)?
            );
        }
    }

    tracing::info!("Topping up collateral for provers if below threshold");

    // Top up collateral for provers if below threshold
    for prover_key in &args.prover_keys {
        let prover_wallet = EthereumWallet::from(prover_key.clone());
        let prover_address = prover_wallet.default_signer().address();

        let collateral_token =
            distributor_client.boundless_market.collateral_token_address().await?;
        let collateral_token_contract = IERC20::new(collateral_token, distributor_provider.clone());

        let distributor_collateral_balance =
            collateral_token_contract.balanceOf(distributor_address).call().await?;
        let prover_collateral_balance_market =
            distributor_client.boundless_market.balance_of_collateral(prover_address).await?;

        tracing::info!("Account {} has {} collateral balance deposited to market. Threshold for top up is {}. Distributor has {} collateral balance (Collateral token: 0x{:x}). ", prover_address, format_units(prover_collateral_balance_market, collateral_token_decimals)?, format_units(collateral_threshold, collateral_token_decimals)?, format_units(distributor_collateral_balance, collateral_token_decimals)?, collateral_token);

        if prover_collateral_balance_market < collateral_threshold {
            let mut prover_collateral_balance_contract =
                collateral_token_contract.balanceOf(prover_address).call().await?;

            let transfer_amount =
                collateral_top_up_amount.saturating_sub(prover_collateral_balance_market);

            if transfer_amount > distributor_collateral_balance {
                tracing::error!("[B-DIST-STK]: Distributor {} has insufficient collateral balance to top up prover {} with {} collateral", distributor_address, prover_address, format_units(transfer_amount, collateral_token_decimals)?);
                continue;
            }

            if transfer_amount == U256::ZERO {
                tracing::error!(
                    "Misconfiguration: collateral top up amount too low, or threshold too high"
                );
                continue;
            }

            tracing::info!(
                "Transferring {} collateral from distributor to prover {} [collateral top up amount: {}, balance on market: {}, balance on contract: {}]",
                format_units(transfer_amount, collateral_token_decimals)?,
                prover_address,
                format_units(collateral_top_up_amount, collateral_token_decimals)?,
                format_units(prover_collateral_balance_market, collateral_token_decimals)?,
                format_units(prover_collateral_balance_contract, collateral_token_decimals)?
            );
            let pending_tx = match collateral_token_contract
                .transfer(prover_address, transfer_amount)
                .send()
                .await
            {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!(
                        "Failed to send collateral transfer transaction from distributor to prover {}: {:?}. Skipping.",
                        prover_address, e
                    );
                    continue;
                }
            };

            let receipt = match pending_tx.with_timeout(Some(TX_TIMEOUT)).watch().await {
                Ok(receipt) => receipt,
                Err(e) => {
                    tracing::error!(
                        "Failed to watch collateral transfer transaction from distributor to prover {}: {:?}. Skipping.",
                        prover_address, e
                    );
                    continue;
                }
            };

            tracing::info!("Collateral transfer completed: Tx hash: 0x{:x}. Amount: {} from distributor to prover {}. About to deposit collateral", receipt, format_units(transfer_amount, collateral_token_decimals)?, prover_address);

            // Then have the prover deposit the collateral
            let prover_client = Client::builder()
                .with_rpc_url(args.rpc_url.clone())
                .with_private_key(prover_key.clone())
                .with_timeout(Some(TX_TIMEOUT))
                .with_deployment(args.deployment.clone())
                .build()
                .await?;

            prover_collateral_balance_contract =
                collateral_token_contract.balanceOf(prover_address).call().await?;

            prover_client.boundless_market.approve_deposit_collateral(U256::MAX).await?;
            tracing::info!(
                "Approved {} collateral to deposit for prover {}. About to deposit collateral",
                format_units(prover_collateral_balance_contract, collateral_token_decimals)?,
                prover_address
            );
            if let Err(e) = prover_client
                .boundless_market
                .deposit_collateral(prover_collateral_balance_contract)
                .await
            {
                tracing::error!(
                    "Failed to deposit collateral to boundless market for prover {}: {:?}. Skipping.",
                    prover_address,
                    e
                );
                continue;
            }
            tracing::info!(
                "Collateral deposit of {} completed for prover {}",
                format_units(prover_collateral_balance_contract, collateral_token_decimals)?,
                prover_address
            );
        }
    }

    // Top up ETH for all accounts if below threshold
    let all_accounts = [
        args.prover_keys.iter().collect::<Vec<_>>(),
        args.order_generator_keys.iter().collect::<Vec<_>>(),
        vec![&args.slasher_key],
    ]
    .concat();

    let offchain_requestor_addresses: std::collections::HashSet<_> =
        args.offchain_requestor_addresses.iter().cloned().collect();

    for key in all_accounts {
        let wallet = EthereumWallet::from(key.clone());
        let address = wallet.default_signer().address();

        let is_offchain_requestor = offchain_requestor_addresses.contains(&address);

        // For offchain requestors, check market balance; for others, check wallet balance
        let (account_eth_balance, balance_location) = if is_offchain_requestor {
            let market_balance = distributor_client.boundless_market.balance_of(address).await?;
            (market_balance, "market")
        } else {
            let wallet_balance = distributor_client.provider().get_balance(address).await?;
            (wallet_balance, "wallet")
        };

        let distributor_eth_balance =
            distributor_client.provider().get_balance(distributor_address).await?;

        tracing::info!("Account {} has {} ETH balance in {}. Threshold for top up is {}. Distributor has {} ETH balance. ", address, format_units(account_eth_balance, "ether")?, balance_location, format_units(eth_threshold, "ether")?, format_units(distributor_eth_balance, "ether")?);

        if account_eth_balance < eth_threshold {
            let transfer_amount = eth_top_up_amount.saturating_sub(account_eth_balance);

            if transfer_amount > distributor_eth_balance {
                tracing::error!("[B-DIST-ETH]: Distributor {} has insufficient ETH balance to top up {} with {} ETH.", distributor_address, address, format_units(transfer_amount, "ether")?);
                continue;
            }

            if transfer_amount == U256::ZERO {
                tracing::error!("Misconfiguration: ETH top up amount too low, or threshold too high [top up amount: {}, address 0x{:x} balance: {}, distributor balance: {}]", format_units(eth_top_up_amount, "ether")?, address, format_units(account_eth_balance, "ether")?, format_units(distributor_eth_balance, "ether")?);
                continue;
            }

            tracing::info!(
                "Transferring {} ETH from distributor to {}",
                format_units(transfer_amount, "ether")?,
                address
            );

            let eth_amount = if is_offchain_requestor
                && distributor_client.provider().get_balance(address).await? < parse_ether("0.01")?
            {
                // If offchain requestor, add some ETH for gas
                transfer_amount.saturating_add(parse_ether("0.01")?)
            } else {
                transfer_amount
            };

            // Transfer ETH for gas
            let tx = TransactionRequest::default()
                .with_from(distributor_address)
                .with_to(address)
                .with_value(eth_amount);

            let pending_tx = match distributor_client.provider().send_transaction(tx).await {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!(
                        "Failed to send ETH transfer transaction from distributor to {}: {:?}. Skipping.",
                        address, e
                    );
                    continue;
                }
            };

            let receipt = match pending_tx.with_timeout(Some(TX_TIMEOUT)).watch().await {
                Ok(receipt) => receipt,
                Err(e) => {
                    tracing::error!(
                        "Failed to watch ETH transfer transaction from distributor to {}: {:?}. Skipping.",
                        address, e
                    );
                    continue;
                }
            };

            tracing::info!(
                "ETH transfer completed: {:x}. {} ETH from distributor to {}",
                receipt,
                format_units(transfer_amount, "ether")?,
                address
            );

            // Only deposit to market for offchain requestors
            if is_offchain_requestor {
                tracing::info!("Depositing ETH to market for offchain requestor {}", address);

                let account_client = Client::builder()
                    .with_rpc_url(args.rpc_url.clone())
                    .with_private_key(key.clone())
                    .with_deployment(args.deployment.clone())
                    .with_timeout(Some(TX_TIMEOUT))
                    .build()
                    .await?;

                if let Err(e) = account_client.boundless_market.deposit(transfer_amount).await {
                    tracing::error!(
                            "Failed to deposit ETH to boundless market for offchain requestor {}: {:?}. Skipping.",
                            address,
                            e
                        );
                    continue;
                }
                tracing::info!(
                    "ETH deposit completed for offchain requestor {} with {} ETH",
                    address,
                    format_units(transfer_amount, "ether")?
                );
            }
        }
    }

    // --- External prover collateral top-ups ---
    if args.enable_external_topup {
        match args.indexer_api_url {
            Some(ref indexer_api_url) => {
                if let Err(e) = top_up_external_provers(
                    args,
                    &distributor_client.boundless_market,
                    collateral_token_decimals,
                    indexer_api_url,
                )
                .await
                {
                    tracing::error!("External prover top-up flow failed: {:?}", e);
                }
            }
            None => {
                tracing::error!("--enable-external-topup is set but --indexer-api-url is missing");
            }
        }
    } else {
        tracing::info!("External prover top-ups disabled");
    }

    Ok(())
}

async fn top_up_external_provers<P: Provider<alloy::network::Ethereum> + Clone + 'static>(
    args: &MainArgs,
    market: &BoundlessMarketService<P>,
    collateral_token_decimals: u8,
    indexer_api_url: &Url,
) -> Result<()> {
    tracing::info!("Starting external prover collateral top-up flow");

    // Parse external prover thresholds
    let ext_collateral_threshold: U256 =
        parse_units(&args.external_collateral_threshold, collateral_token_decimals)?.into();
    let ext_per_top_up: U256 =
        parse_units(&args.external_per_top_up_amount, collateral_token_decimals)?.into();
    let ext_lifetime_allowance: U256 =
        parse_units(&args.external_lifetime_allowance, collateral_token_decimals)?.into();

    // Hard cap: lifetime allowance must never exceed 100 ZKC
    let max_lifetime: U256 = parse_units("100", collateral_token_decimals)?.into();
    anyhow::ensure!(
        ext_lifetime_allowance <= max_lifetime,
        "EXTERNAL_LIFETIME_ALLOWANCE ({}) exceeds the hard cap of 100 ZKC",
        args.external_lifetime_allowance
    );
    anyhow::ensure!(
        ext_per_top_up <= ext_lifetime_allowance,
        "EXTERNAL_PER_TOP_UP_AMOUNT ({}) exceeds EXTERNAL_LIFETIME_ALLOWANCE ({})",
        args.external_per_top_up_amount,
        args.external_lifetime_allowance
    );

    // 1. Fetch active provers from indexer API (7D period)
    let indexer = IndexerClient::new(indexer_api_url.clone())?;
    let provers_resp = indexer.get_provers("7d").await?;
    tracing::info!("Fetched {} provers from indexer API", provers_resp.data.len());

    // 2. Filter by min cycles
    let eligible: Vec<&ProverLeaderboardEntry> = provers_resp
        .data
        .iter()
        .filter(|p| {
            p.cycles.parse::<u64>().unwrap_or(0) >= args.min_bcycles_threshold * 1_000_000_000
        })
        .collect();
    tracing::info!(
        "{} provers meet the min cycles threshold of {}B",
        eligible.len(),
        args.min_bcycles_threshold
    );

    if eligible.is_empty() {
        return Ok(());
    }

    // 3. Load lifetime allowance state
    let mut state = load_state(&args.allowance_state_file)?;

    // 4. Set up Chainalysis Sanctions API (skip when API key is empty, e.g. testnets)
    let skip_ofac = args.chainalysis_api_key.is_empty();
    if skip_ofac {
        tracing::warn!(
            "Chainalysis API key is empty — OFAC screening is DISABLED. \
             This is expected on testnets but must not happen in production."
        );
    }
    let http_client = reqwest::Client::new();

    // 5. Chain-aware deposit setup
    let chain_id = market.get_chain_id().await?;
    let use_permit = collateral_token_supports_permit(chain_id);

    // Track whether we still need to send the ERC-20 approve tx.
    // For non-permit chains we defer it until we know there is at least one deposit to make.
    let mut needs_approve = !use_permit;

    let signer = &args.private_key;

    let mut topped_up: u32 = 0;
    let mut skipped_exhausted: u32 = 0;
    let mut skipped_sanctioned: u32 = 0;
    let mut skipped_above_threshold: u32 = 0;
    let mut skipped_ofac_err: u32 = 0;
    let mut failed: u32 = 0;

    for prover in &eligible {
        let addr: Address = prover
            .prover_address
            .parse()
            .with_context(|| format!("Invalid prover address: {}", prover.prover_address))?;

        let addr_key = format!("{:?}", addr);
        let entry = state.provers.entry(addr_key.clone()).or_default();

        let total_sent = parse_total_sent(&entry.total_sent);

        // Skip if exhausted
        if total_sent >= ext_lifetime_allowance {
            tracing::info!(
                "[B-TOPUP-EXHAUSTED] Prover {} has reached lifetime allowance of {}",
                addr,
                format_units(ext_lifetime_allowance, collateral_token_decimals)?
            );
            skipped_exhausted += 1;
            continue;
        }

        // Skip if previously sanctioned
        if entry.sanctioned {
            tracing::info!("[B-TOPUP-OFAC] Prover {} previously flagged as sanctioned", addr);
            skipped_sanctioned += 1;
            continue;
        }

        // Use deposited collateral from indexer
        let balance: U256 = if prover.collateral_deposited_zkc.is_empty() {
            U256::ZERO
        } else {
            parse_units(&prover.collateral_deposited_zkc, collateral_token_decimals)
                .map(|v| v.into())
                .unwrap_or(U256::ZERO)
        };

        if balance >= ext_collateral_threshold {
            tracing::info!(
                "Prover {} has {} deposited collateral, above threshold {}. Skipping.",
                addr,
                format_units(balance, collateral_token_decimals)?,
                format_units(ext_collateral_threshold, collateral_token_decimals)?
            );
            skipped_above_threshold += 1;
            continue;
        }

        // OFAC screen via Chainalysis REST API (fail-closed: skip on error)
        if !skip_ofac {
            let url = format!(
                "{}/api/v1/address/{:?}",
                args.chainalysis_api_url.trim_end_matches('/'),
                addr
            );
            match http_client.get(&url).header("X-API-Key", &args.chainalysis_api_key).send().await
            {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<SanctionsResponse>().await {
                        Ok(body) if !body.identifications.is_empty() => {
                            tracing::warn!(
                                "[B-TOPUP-OFAC] Address {} is sanctioned, blocking top-up",
                                addr
                            );
                            entry.sanctioned = true;
                            skipped_sanctioned += 1;
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[B-TOPUP-OFAC-ERR] Failed to parse sanctions response for {}: {}. Skipping (fail-closed).",
                                addr, e
                            );
                            skipped_ofac_err += 1;
                            continue;
                        }
                        _ => {} // not sanctioned
                    }
                }
                Ok(resp) => {
                    tracing::warn!(
                        "[B-TOPUP-OFAC-ERR] Chainalysis API returned status {} for {}. Skipping (fail-closed).",
                        resp.status(), addr
                    );
                    skipped_ofac_err += 1;
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        "[B-TOPUP-OFAC-ERR] Chainalysis API call failed for {}: {}. Skipping (fail-closed).",
                        addr, e
                    );
                    skipped_ofac_err += 1;
                    continue;
                }
            }
        }

        // Compute capped amount
        let remaining = ext_lifetime_allowance.saturating_sub(total_sent);
        let amount = ext_per_top_up.min(remaining);

        if amount == U256::ZERO {
            continue;
        }

        tracing::info!(
            "Depositing {} collateral to external prover {} (lifetime sent: {}, remaining: {})",
            format_units(amount, collateral_token_decimals)?,
            addr,
            format_units(total_sent, collateral_token_decimals)?,
            format_units(remaining, collateral_token_decimals)?
        );

        // Lazily approve on the first deposit for non-permit chains
        if needs_approve {
            tracing::info!(
                "Chain {} does not support permit, using approve+depositCollateralTo",
                chain_id
            );
            market.approve_deposit_collateral(U256::MAX).await?;
            needs_approve = false;
        }

        // Deposit directly to prover's market balance
        let deposit_result = if use_permit {
            market.deposit_collateral_with_permit_to(addr, amount, signer).await
        } else {
            market.deposit_collateral_to(addr, amount).await
        };

        match deposit_result {
            Ok(()) => {
                let new_total = total_sent.saturating_add(amount);
                entry.total_sent = format!("{}", new_total);
                entry.last_top_up = Some(Utc::now().to_rfc3339());
                tracing::info!(
                    "[B-TOPUP-OK] Successfully topped up prover {} with {} collateral",
                    addr,
                    format_units(amount, collateral_token_decimals)?
                );
                topped_up += 1;
            }
            Err(e) => {
                tracing::error!(
                    "Failed to deposit collateral to external prover {}: {:?}. Skipping.",
                    addr,
                    e
                );
                failed += 1;
                continue;
            }
        }
    }

    tracing::info!(
        "External prover top-up summary: {} eligible, {} topped up, {} failed, {} above threshold, {} exhausted, {} sanctioned, {} OFAC errors",
        eligible.len(), topped_up, failed, skipped_above_threshold, skipped_exhausted, skipped_sanctioned, skipped_ofac_err
    );

    // 6. Save state
    save_state(&args.allowance_state_file, &state)?;
    tracing::info!("External prover top-up state saved to {}", args.allowance_state_file.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil,
        providers::{ext::AnvilApi, Provider},
    };
    use boundless_test_utils::market::create_test_ctx;
    use httpmock::prelude::*;
    use tracing_test::traced_test;

    use super::*;

    /// Build a mock indexer server that returns the given prover entries.
    fn mock_indexer_server(provers: &[serde_json::Value]) -> MockServer {
        let server = MockServer::start();
        let response = serde_json::json!({
            "chain_id": 31337,
            "period": "7d",
            "period_start": 0,
            "period_end": 9999999999i64,
            "data": provers,
            "next_cursor": null,
            "has_more": false,
        });
        server.mock(|when, then| {
            when.method(GET).path("/v1/market/provers");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(serde_json::to_string(&response).unwrap());
        });
        server
    }

    /// Build a prover leaderboard entry JSON for a given address.
    fn prover_entry(
        address: Address,
        cycles: u64,
        collateral_deposited_zkc: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "chain_id": 31337,
            "prover_address": format!("{:?}", address),
            "orders_locked": 10,
            "orders_fulfilled": 9,
            "cycles": cycles.to_string(),
            "cycles_formatted": format!("{} cycles", cycles),
            "fees_earned": "1000000000000000000",
            "fees_earned_formatted": "1.0 ETH",
            "collateral_earned": "0",
            "collateral_earned_formatted": "0.0",
            "median_lock_price_per_cycle": null,
            "median_lock_price_per_cycle_formatted": null,
            "best_effective_prove_mhz": 100.0,
            "locked_order_fulfillment_rate": 90.0,
            "collateral_deposited_zkc": collateral_deposited_zkc.to_string(),
            "collateral_deposited_zkc_formatted": format!("{} ZKC", collateral_deposited_zkc),
            "collateral_available_zkc": "0",
            "collateral_available_zkc_formatted": "0.0 ZKC",
            "last_activity_time": 1710000000,
            "last_activity_time_iso": "2025-03-10T00:00:00Z",
        })
    }

    /// Common test fixture for external prover top-up tests.
    ///
    /// Encapsulates anvil, contract deployment, distributor funding, mock Chainalysis
    /// API server, state dir, and a Client for post-run assertions.
    struct ExternalTopupFixture {
        anvil: alloy::node_bindings::AnvilInstance,
        deployment: Deployment,
        distributor_signer: PrivateKeySigner,
        slasher_signer: PrivateKeySigner,
        chainalysis_mock: MockServer,
        state_dir: tempfile::TempDir,
    }

    impl ExternalTopupFixture {
        /// Create a fully-initialized fixture: anvil, contracts, funded distributor,
        /// mock Chainalysis API server, and a temp dir for the state file.
        async fn new() -> Self {
            let anvil = Anvil::new().spawn();
            let ctx = create_test_ctx(&anvil).await.unwrap();

            let distributor_signer = PrivateKeySigner::random();
            let slasher_signer = PrivateKeySigner::random();

            let provider = ProviderBuilder::new().connect(&anvil.endpoint()).await.unwrap();
            provider
                .anvil_set_balance(distributor_signer.address(), parse_ether("10").unwrap())
                .await
                .unwrap();

            ctx.hit_points_service
                .mint(distributor_signer.address(), parse_units("200", 18).unwrap().into())
                .await
                .unwrap();

            let chainalysis_mock = MockServer::start();

            Self {
                anvil,
                deployment: ctx.deployment,
                distributor_signer,
                slasher_signer,
                chainalysis_mock,
                state_dir: tempfile::tempdir().unwrap(),
            }
        }

        fn state_file(&self) -> PathBuf {
            self.state_dir.path().join("topup-state.json")
        }

        fn args(&self, indexer_url: &str) -> MainArgs {
            MainArgs {
                rpc_url: self.anvil.endpoint_url(),
                private_key: self.distributor_signer.clone(),
                prover_keys: vec![],
                prover_eth_donate_threshold: "1.0".to_string(),
                prover_stake_donate_threshold: "20.0".to_string(),
                eth_threshold: "0.1".to_string(),
                stake_threshold: "0.1".to_string(),
                eth_top_up_amount: "0.5".to_string(),
                stake_top_up_amount: "5".to_string(),
                distributor_eth_alert_threshold: "0.5".to_string(),
                distributor_stake_alert_threshold: "50.0".to_string(),
                order_generator_keys: vec![],
                offchain_requestor_addresses: vec![],
                slasher_key: self.slasher_signer.clone(),
                deployment: Some(self.deployment.clone()),
                enable_external_topup: true,
                indexer_api_url: Some(indexer_url.parse().unwrap()),
                min_bcycles_threshold: 1,
                chainalysis_api_key: "test-key".to_string(),
                chainalysis_api_url: self.chainalysis_mock.base_url(),
                external_collateral_threshold: "5".to_string(),
                external_per_top_up_amount: "10".to_string(),
                external_lifetime_allowance: "100".to_string(),
                allowance_state_file: self.state_file(),
            }
        }

        async fn collateral_balance_of(&self, addr: Address) -> U256 {
            let client = Client::builder()
                .with_rpc_url(self.anvil.endpoint_url())
                .with_private_key(self.distributor_signer.clone())
                .with_deployment(self.deployment.clone())
                .build()
                .await
                .unwrap();
            client.boundless_market.balance_of_collateral(addr).await.unwrap()
        }

        fn load_state(&self) -> AllowanceState {
            serde_json::from_str(&std::fs::read_to_string(self.state_file()).unwrap()).unwrap()
        }

        /// Register a catch-all mock returning "not sanctioned" for any address.
        fn allow_all(&self) {
            self.chainalysis_mock.mock(|when, then| {
                when.method(GET).path_contains("/api/v1/address/");
                then.status(200)
                    .header("Content-Type", "application/json")
                    .body(r#"{"identifications":[]}"#);
            });
        }

        /// Register a mock response marking the given address as sanctioned.
        fn sanction(&self, addr: Address) {
            let addr_str = format!("{:?}", addr);
            self.chainalysis_mock.mock(|when, then| {
                when.method(GET).path(format!("/api/v1/address/{}", addr_str));
                then.status(200).header("Content-Type", "application/json").body(
                    serde_json::to_string(&serde_json::json!({
                        "identifications": [{
                            "category": "Sanctions",
                            "name": "OFAC SDN",
                            "description": "Sanctioned address",
                            "url": "https://example.com"
                        }]
                    }))
                    .unwrap(),
                );
            });
        }

        /// Register a mock returning 500 for the given address.
        fn fail_ofac(&self, addr: Address) {
            let addr_str = format!("{:?}", addr);
            self.chainalysis_mock.mock(|when, then| {
                when.method(GET).path(format!("/api/v1/address/{}", addr_str));
                then.status(500).body("Internal Server Error");
            });
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn test_main() {
        let anvil = Anvil::new().spawn();

        let ctx = create_test_ctx(&anvil).await.unwrap();

        let distributor_signer: PrivateKeySigner = PrivateKeySigner::random();
        let slasher_signer: PrivateKeySigner = PrivateKeySigner::random();
        let order_generator_signer: PrivateKeySigner = PrivateKeySigner::random();
        let offchain_requestor_signer: PrivateKeySigner = order_generator_signer.clone(); // Use order generator as offchain requestor for testing
        let prover_signer_1: PrivateKeySigner = PrivateKeySigner::random();
        let prover_signer_2: PrivateKeySigner = PrivateKeySigner::random();

        let distributor_client = Client::builder()
            .with_rpc_url(anvil.endpoint_url())
            .with_private_key(distributor_signer.clone())
            .with_deployment(ctx.deployment.clone())
            .build()
            .await
            .unwrap();

        let provider = ProviderBuilder::new().connect(&anvil.endpoint()).await.unwrap();
        provider
            .anvil_set_balance(distributor_signer.address(), parse_ether("10").unwrap())
            .await
            .unwrap();

        let args = MainArgs {
            rpc_url: anvil.endpoint_url(),
            private_key: distributor_signer.clone(),
            prover_keys: vec![prover_signer_1.clone(), prover_signer_2.clone()],
            prover_eth_donate_threshold: "1.0".to_string(),
            prover_stake_donate_threshold: "20.0".to_string(),
            eth_threshold: "0.1".to_string(),
            stake_threshold: "0.1".to_string(),
            eth_top_up_amount: "0.5".to_string(),
            stake_top_up_amount: "5".to_string(),
            distributor_eth_alert_threshold: "0.5".to_string(),
            distributor_stake_alert_threshold: "50.0".to_string(),
            order_generator_keys: vec![order_generator_signer.clone()],
            offchain_requestor_addresses: vec![offchain_requestor_signer.address()],
            slasher_key: slasher_signer.clone(),
            deployment: Some(ctx.deployment.clone()),
            // External prover args (disabled for this test)
            enable_external_topup: false,
            indexer_api_url: None,
            min_bcycles_threshold: 1,
            chainalysis_api_key: String::new(),
            chainalysis_api_url: String::new(),
            external_collateral_threshold: "5".to_string(),
            external_per_top_up_amount: "10".to_string(),
            external_lifetime_allowance: "100".to_string(),
            allowance_state_file: PathBuf::from("./test-topup-state.json"),
        };

        run(&args).await.unwrap();

        // Check wallet ETH balances after run (for non-offchain requestors)
        let prover_eth_balance =
            distributor_client.provider().get_balance(prover_signer_1.address()).await.unwrap();
        let prover_eth_balance_2 =
            distributor_client.provider().get_balance(prover_signer_2.address()).await.unwrap();
        let slasher_eth_balance =
            distributor_client.provider().get_balance(slasher_signer.address()).await.unwrap();

        // Check market ETH balance for offchain requestor (order generator in this test)
        let offchain_requestor_eth_balance_market = distributor_client
            .boundless_market
            .balance_of(order_generator_signer.address())
            .await
            .unwrap();

        // Check stake balances on the market
        let prover_stake_balance = distributor_client
            .boundless_market
            .balance_of_collateral(prover_signer_1.address())
            .await
            .unwrap();
        let prover_stake_balance_2 = distributor_client
            .boundless_market
            .balance_of_collateral(prover_signer_2.address())
            .await
            .unwrap();

        let eth_top_up_amount = parse_ether(&args.eth_top_up_amount).unwrap();

        assert_eq!(prover_eth_balance, eth_top_up_amount);
        assert_eq!(prover_eth_balance_2, eth_top_up_amount);
        assert_eq!(slasher_eth_balance, eth_top_up_amount);

        assert!(offchain_requestor_eth_balance_market == eth_top_up_amount);

        // Distributor should not have any collateral
        assert_eq!(prover_stake_balance, U256::ZERO);
        assert_eq!(prover_stake_balance_2, U256::ZERO);
        assert!(logs_contain("[B-DIST-STK]"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_below_threshold() {
        let fix = ExternalTopupFixture::new().await;
        fix.allow_all();
        let external_prover = PrivateKeySigner::random();

        let server =
            mock_indexer_server(&[prover_entry(external_prover.address(), 2_000_000_000, "0")]);
        let args = fix.args(&server.base_url());

        run(&args).await.unwrap();

        let expected: U256 = parse_units("10", 18).unwrap().into();
        assert_eq!(
            fix.collateral_balance_of(external_prover.address()).await,
            expected,
            "External prover should have been topped up"
        );
        assert!(logs_contain("[B-TOPUP-OK]"));

        let state = fix.load_state();
        let entry = state.provers.get(&format!("{:?}", external_prover.address())).unwrap();
        assert_eq!(parse_total_sent(&entry.total_sent), expected);
        assert!(!entry.sanctioned);
        assert!(entry.last_top_up.is_some());
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_lifetime_cap() {
        let fix = ExternalTopupFixture::new().await;
        let external_prover = PrivateKeySigner::random();

        let server =
            mock_indexer_server(&[prover_entry(external_prover.address(), 2_000_000_000, "0")]);

        // Pre-seed state: prover already received 100 ZKC (= lifetime cap)
        let lifetime_cap: U256 = parse_units("100", 18).unwrap().into();
        let initial_state = AllowanceState {
            provers: HashMap::from([(
                format!("{:?}", external_prover.address()),
                ProverAllowance {
                    total_sent: format!("{}", lifetime_cap),
                    last_top_up: Some("2026-01-01T00:00:00Z".to_string()),
                    sanctioned: false,
                },
            )]),
        };
        std::fs::write(fix.state_file(), serde_json::to_string(&initial_state).unwrap()).unwrap();

        let args = fix.args(&server.base_url());
        run(&args).await.unwrap();

        assert_eq!(
            fix.collateral_balance_of(external_prover.address()).await,
            U256::ZERO,
            "Capped prover should not get more collateral"
        );
        assert!(logs_contain("[B-TOPUP-EXHAUSTED]"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_sanctioned_address() {
        let fix = ExternalTopupFixture::new().await;
        let sanctioned_prover = PrivateKeySigner::random();

        fix.sanction(sanctioned_prover.address());

        let server =
            mock_indexer_server(&[prover_entry(sanctioned_prover.address(), 2_000_000_000, "0")]);
        let args = fix.args(&server.base_url());

        run(&args).await.unwrap();

        assert_eq!(
            fix.collateral_balance_of(sanctioned_prover.address()).await,
            U256::ZERO,
            "Sanctioned prover should not get collateral"
        );
        assert!(logs_contain("[B-TOPUP-OFAC]"));

        let state = fix.load_state();
        assert!(
            state.provers.get(&format!("{:?}", sanctioned_prover.address())).unwrap().sanctioned
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_api_failure_fail_closed() {
        let fix = ExternalTopupFixture::new().await;
        let external_prover = PrivateKeySigner::random();

        let server =
            mock_indexer_server(&[prover_entry(external_prover.address(), 2_000_000_000, "0")]);

        // Register a mock returning 500 for this address
        fix.fail_ofac(external_prover.address());

        let args = fix.args(&server.base_url());
        run(&args).await.unwrap();

        assert_eq!(
            fix.collateral_balance_of(external_prover.address()).await,
            U256::ZERO,
            "API failure should fail-closed, no top-up"
        );
        assert!(logs_contain("[B-TOPUP-OFAC-ERR]"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_below_min_cycles() {
        let fix = ExternalTopupFixture::new().await;
        let low_cycles_prover = PrivateKeySigner::random();

        let server =
            mock_indexer_server(&[prover_entry(low_cycles_prover.address(), 500_000, "0")]);
        let args = fix.args(&server.base_url());

        run(&args).await.unwrap();

        assert_eq!(
            fix.collateral_balance_of(low_cycles_prover.address()).await,
            U256::ZERO,
            "Low-cycles prover should not get collateral"
        );
        assert!(!logs_contain("[B-TOPUP-OK]"));
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_lifetime_allowance_exceeds_hard_cap() {
        let fix = ExternalTopupFixture::new().await;
        let external_prover = PrivateKeySigner::random();

        let server =
            mock_indexer_server(&[prover_entry(external_prover.address(), 2_000_000_000, "0")]);
        let mut args = fix.args(&server.base_url());
        args.external_lifetime_allowance = "101".to_string();

        run(&args).await.unwrap();

        assert!(logs_contain("exceeds the hard cap of 100 ZKC"));
        assert!(!logs_contain("[B-TOPUP-OK]"));
        assert_eq!(
            fix.collateral_balance_of(external_prover.address()).await,
            U256::ZERO,
            "No top-up should occur when lifetime allowance exceeds hard cap"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_per_amount_exceeds_lifetime_allowance() {
        let fix = ExternalTopupFixture::new().await;
        let external_prover = PrivateKeySigner::random();

        let server =
            mock_indexer_server(&[prover_entry(external_prover.address(), 2_000_000_000, "0")]);
        let mut args = fix.args(&server.base_url());
        args.external_per_top_up_amount = "60".to_string();
        args.external_lifetime_allowance = "50".to_string();

        run(&args).await.unwrap();

        assert!(logs_contain("exceeds EXTERNAL_LIFETIME_ALLOWANCE"));
        assert!(!logs_contain("[B-TOPUP-OK]"));
        assert_eq!(
            fix.collateral_balance_of(external_prover.address()).await,
            U256::ZERO,
            "No top-up should occur when per-top-up exceeds lifetime allowance"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_external_topup_disabled() {
        let fix = ExternalTopupFixture::new().await;

        let mut args = fix.args("http://localhost:1");
        args.enable_external_topup = false;

        run(&args).await.unwrap();

        assert!(logs_contain("External prover top-ups disabled"));
        assert!(!logs_contain("[B-TOPUP-OK]"));
    }
}
