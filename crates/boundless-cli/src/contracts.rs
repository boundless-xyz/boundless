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

//! Contract interaction helpers for common patterns

use alloy::{
    network::Ethereum,
    primitives::{Address, B256, U256},
    providers::{PendingTransactionBuilder, Provider},
    rpc::types::TransactionReceipt,
};
use anyhow::{ensure, Context, Result};
use boundless_market::contracts::token::IERC20;
use std::sync::Arc;
use std::time::Duration;

/// Standard token information
#[derive(Debug, Clone)]
pub struct TokenInfo {
    /// Token contract address
    pub address: Address,
    /// Token symbol (e.g., "USDC", "DAI")
    pub symbol: String,
    /// Number of decimal places
    pub decimals: u8,
    /// Optional token name
    pub name: Option<String>,
}

/// Get ERC20 token information
pub async fn get_token_info<P>(provider: P, token_address: Address) -> Result<TokenInfo>
where
    P: Provider<Ethereum> + Clone + 'static,
{
    let token = IERC20::new(token_address, provider);

    let symbol = token
        .symbol()
        .call()
        .await
        .context("Failed to get token symbol")?;

    let decimals = token
        .decimals()
        .call()
        .await
        .context("Failed to get token decimals")?;

    // Name is optional - not all tokens have it in the IERC20 interface
    // We'll set it to None for now
    let name = None;

    Ok(TokenInfo {
        address: token_address,
        symbol,
        decimals,
        name,
    })
}

/// Get balance of an ERC20 token
pub async fn get_token_balance<P>(
    provider: P,
    token_address: Address,
    account: Address,
) -> Result<U256>
where
    P: Provider<Ethereum> + Clone + 'static,
{
    let token = IERC20::new(token_address, provider);
    token
        .balanceOf(account)
        .call()
        .await
        .context("Failed to get token balance")
}

/// Get both native ETH and token balances
pub async fn get_balances<P>(
    provider: P,
    account: Address,
    token_address: Option<Address>,
) -> Result<(U256, Option<U256>)>
where
    P: Provider<Ethereum> + Clone + 'static,
{
    let eth_balance = provider
        .get_balance(account)
        .await
        .context("Failed to get ETH balance")?;

    let token_balance = if let Some(addr) = token_address {
        Some(get_token_balance(provider, addr, account).await?)
    } else {
        None
    };

    Ok((eth_balance, token_balance))
}

/// Standard transaction confirmation helper
pub async fn confirm_transaction(
    pending: PendingTransactionBuilder<Ethereum>,
    timeout: Option<Duration>,
    confirmations: u64,
) -> Result<TransactionReceipt> {
    let tx_hash = *pending.tx_hash();

    let mut pending_with_config = pending.with_required_confirmations(confirmations);

    if let Some(timeout_duration) = timeout {
        pending_with_config = pending_with_config.with_timeout(Some(timeout_duration));
    }

    let receipt = pending_with_config
        .get_receipt()
        .await
        .with_context(|| format!("Failed to get receipt for transaction {:#x}", tx_hash))?;

    ensure!(
        receipt.status(),
        "Transaction failed: {:#x}",
        receipt.transaction_hash
    );

    Ok(receipt)
}

/// Extract a specific event from a transaction receipt
pub fn extract_event<E>(receipt: &TransactionReceipt) -> Result<E>
where
    E: alloy::sol_types::SolEvent,
{
    receipt
        .logs()
        .iter()
        .filter_map(|log| log.log_decode::<E>().ok())
        .next()
        .with_context(|| format!("Event {} not found in transaction receipt", E::SIGNATURE))
        .map(|log| log.inner.data)
}

/// Check if an address has sufficient balance for a transaction
pub async fn check_sufficient_balance<P>(
    provider: P,
    account: Address,
    required: U256,
    token_address: Option<Address>,
) -> Result<bool>
where
    P: Provider<Ethereum> + Clone + 'static,
{
    let balance = if let Some(token) = token_address {
        get_token_balance(provider, token, account).await?
    } else {
        provider.get_balance(account).await?
    };

    Ok(balance >= required)
}

/// Batch balance queries for multiple addresses
pub async fn get_multiple_balances<P>(
    provider: P,
    addresses: Vec<Address>,
    token_address: Option<Address>,
) -> Result<Vec<U256>>
where
    P: Provider<Ethereum> + Clone + 'static,
{
    use futures::future::try_join_all;

    if let Some(token) = token_address {
        let token_contract = IERC20::new(token, provider.clone());
        let futures = addresses
            .into_iter()
            .map(move |addr| {
                let token_contract = token_contract.clone();
                async move {
                    token_contract.balanceOf(addr).call().await
                }
            })
            .collect::<Vec<_>>();

        try_join_all(futures)
            .await
            .context("Failed to get token balances")
    } else {
        let futures = addresses
            .into_iter()
            .map(move |addr| {
                let provider = provider.clone();
                async move {
                    provider.get_balance(addr).await
                }
            })
            .collect::<Vec<_>>();

        try_join_all(futures)
            .await
            .context("Failed to get ETH balances")
    }
}

/// Common contract error decoder
pub trait DecodeContractError {
    /// Decode the error into a human-readable string
    fn decode_error(&self) -> String;
}

/// Helper for approve and call pattern
pub async fn approve_and_call<P>(
    provider: P,
    token_address: Address,
    spender: Address,
    amount: U256,
    call_fn: impl std::future::Future<Output = Result<PendingTransactionBuilder<Ethereum>>>,
) -> Result<B256>
where
    P: Provider<Ethereum> + Clone + 'static,
{
    // First approve
    let token = IERC20::new(token_address, provider);
    let approve_tx = token
        .approve(spender, amount)
        .send()
        .await
        .context("Failed to send approval transaction")?;

    let approve_receipt = confirm_transaction(approve_tx, None, 1).await?;
    ensure!(approve_receipt.status(), "Approval transaction failed");

    // Then call the actual function
    let call_tx = call_fn.await?;
    let tx_hash = *call_tx.tx_hash();

    // Confirm the transaction
    confirm_transaction(call_tx, None, 1).await?;

    Ok(tx_hash)
}

/// Get current block timestamp
pub async fn get_block_timestamp<P>(provider: P) -> Result<u64>
where
    P: Provider<Ethereum> + 'static,
{
    let block_number = provider
        .get_block_number()
        .await
        .context("Failed to get block number")?;

    let block = provider
        .get_block_by_number(block_number.into())
        .await
        .context("Failed to get block")?
        .context("Block not found")?;

    Ok(block.header.inner.timestamp)
}

/// Estimate gas with a safety margin
pub async fn estimate_gas_with_margin<P>(
    provider: P,
    from: Address,
    to: Address,
    data: Vec<u8>,
    value: U256,
    margin: f64,
) -> Result<U256>
where
    P: Provider<Ethereum> + 'static,
{
    use alloy::rpc::types::TransactionRequest;

    let tx = TransactionRequest::default()
        .from(from)
        .to(to)
        .input(data.into())
        .value(value);

    let estimated = provider
        .estimate_gas(tx)
        .await
        .context("Failed to estimate gas")?;

    // Add safety margin (e.g., 20% more)
    // Convert u64 to u128 for calculation
    let estimated_u128: u128 = estimated.into();
    let with_margin = U256::from(
        (estimated_u128 as f64 * (1.0 + margin)) as u128
    );

    Ok(with_margin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_info_struct() {
        let info = TokenInfo {
            address: Address::ZERO,
            symbol: "TEST".to_string(),
            decimals: 18,
            name: Some("Test Token".to_string()),
        };

        assert_eq!(info.symbol, "TEST");
        assert_eq!(info.decimals, 18);
    }

    #[tokio::test]
    async fn test_estimate_gas_margin() {
        // This would need a mock provider to test properly
        // Just testing the calculation logic
        let base_gas = U256::from(100_000);
        let margin = 0.2; // 20%
        let expected = U256::from(120_000);

        let base_gas_u128: u128 = if base_gas > U256::from(u128::MAX) {
            u128::MAX
        } else {
            base_gas.try_into().unwrap_or(u128::MAX)
        };
        let with_margin = U256::from(
            (base_gas_u128 as f64 * (1.0 + margin)) as u128
        );

        assert_eq!(with_margin, expected);
    }
}