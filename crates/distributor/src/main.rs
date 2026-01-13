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
use anyhow::Result;
use boundless_market::{client::Client, Deployment};
use clap::Parser;
use url::Url;

const TX_TIMEOUT: Duration = Duration::from_secs(180);

sol! {
    #[sol(rpc)]
    contract IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function balanceOf(address owner) external view returns (uint256);
    }
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy::{
        node_bindings::Anvil,
        providers::{ext::AnvilApi, Provider},
    };
    use boundless_test_utils::market::create_test_ctx;
    use tracing_test::traced_test;

    use super::*;

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
}
