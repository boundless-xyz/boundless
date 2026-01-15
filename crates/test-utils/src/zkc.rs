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

// Some of this code is used by the log_updater test and some by mint_calculator test. Each does
// its own dead code analysis and so will report code used only by the other as dead.
#![allow(dead_code)]

use std::sync::Arc;

use alloy::{
    network::EthereumWallet,
    node_bindings::{Anvil, AnvilInstance},
    primitives::{utils::Unit, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol,
    sol_types::SolCall,
};
use anyhow::Context;
use boundless_market::contracts::bytecode::ERC1967Proxy;
use boundless_zkc::deployments::Deployment;
use tokio::sync::Mutex;

use crate::zkc::{StakingRewards::StakingRewardsInstance, VeZKC::VeZKCInstance, ZKC::ZKCInstance};

// Import the Solidity contracts using alloy's sol! macro
// Use the compiled contracts output to allow for deploying the contracts.
// NOTE: This requires running `forge build` before running this test.
// TODO(povw): Work on making this more robust. If the requirement to run forge build before this
// test is removed, then make sure to remove that step from CI.
sol!(
    #[allow(clippy::too_many_arguments)]
    #[sol(rpc)]
    ZKC,
    "../../out/ZKC.sol/ZKC.json"
);

sol!(
    #[allow(clippy::too_many_arguments)]
    #[sol(rpc)]
    VeZKC,
    "../../out/veZKC.sol/veZKC.json"
);

sol!(
    #[sol(rpc)]
    StakingRewards,
    "../../out/StakingRewards.sol/StakingRewards.json"
);

#[derive(Clone)]
pub struct TestCtx {
    pub anvil: Arc<Mutex<AnvilInstance>>,
    pub provider: DynProvider,
    pub zkc: ZKC::ZKCInstance<DynProvider>,
    pub vezkc: VeZKC::VeZKCInstance<DynProvider>,
    pub staking_rewards: StakingRewards::StakingRewardsInstance<DynProvider>,
    pub deployment: Deployment,
}

/// Creates a new [TestCtx] with all the setup needed to test ZKC.
pub async fn test_ctx() -> anyhow::Result<TestCtx> {
    let anvil = Anvil::new().spawn();
    test_ctx_with(Mutex::new(anvil).into(), 0).await
}

pub async fn test_ctx_with(
    anvil: Arc<Mutex<AnvilInstance>>,
    signer_index: usize,
) -> anyhow::Result<TestCtx> {
    let rpc_url = anvil.lock().await.endpoint_url();

    // Create wallet and provider
    let signer: PrivateKeySigner = anvil.lock().await.keys()[signer_index].clone().into();
    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url).erased();

    // Setup the minter wallet.
    let minter = PrivateKeySigner::random();
    let tx_fund_minter = TransactionRequest::default()
        .from(signer.address())
        .to(minter.address())
        .value(Unit::ETHER.wei());
    provider.send_transaction(tx_fund_minter).await?.watch().await?;

    // Deploy ZKC
    let zkc_contract = ZKC::deploy(provider.clone()).await?;
    println!("ZKC deployed at: {:?}", zkc_contract.address());
    let supply = zkc_contract.totalSupply().call().await?;

    let proxy_instance = ERC1967Proxy::deploy(
        &provider,
        *zkc_contract.address(),
        ZKC::initializeCall {
            _initialMinter1: signer.address(),
            _initialMinter2: signer.address(),
            _initialMinter1Amount: supply.div_ceil(U256::from(2)),
            _initialMinter2Amount: supply.div_ceil(U256::from(2)),
            _owner: signer.address(),
        }
        .abi_encode()
        .into(),
    )
    .await
    .context("failed to deploy ZKC proxy")?;
    let zkc_proxy = *proxy_instance.address();
    println!("ZKC proxy deployed at: {:?}", zkc_proxy);

    // InitializeV2 ZKC
    let tx = TransactionRequest::default()
        .from(signer.address())
        .to(zkc_proxy)
        .input(ZKC::initializeV2Call.abi_encode().into())
        .value(U256::ZERO);
    provider.send_transaction(tx).await?.watch().await?;

    // InitializeV3 ZKC
    let tx = TransactionRequest::default()
        .from(signer.address())
        .to(zkc_proxy)
        .input(ZKC::initializeV3Call.abi_encode().into())
        .value(U256::ZERO);
    provider.send_transaction(tx).await?.watch().await?;

    // Deploy veZKC
    let vezkc_contract = VeZKC::deploy(provider.clone()).await?;
    println!("veZKC deployed at: {:?}", vezkc_contract.address());

    let proxy_instance = ERC1967Proxy::deploy(
        &provider,
        *vezkc_contract.address(),
        VeZKC::initializeCall { zkcTokenAddress: zkc_proxy, _admin: signer.address() }
            .abi_encode()
            .into(),
    )
    .await
    .context("failed to deploy veZKC proxy")?;
    let vezkc_proxy = *proxy_instance.address();
    println!("veZKC proxy deployed at: {:?}", vezkc_proxy);

    // Deploy StakingRewards
    let staking_rewards_contract = StakingRewards::deploy(provider.clone()).await?;
    println!("StakingRewards contract deployed at: {:?}", staking_rewards_contract.address());
    let proxy_instance = ERC1967Proxy::deploy(
        provider.clone(),
        *staking_rewards_contract.address(),
        StakingRewards::initializeCall {
            _zkc: zkc_proxy,
            _veZKC: vezkc_proxy,
            _admin: signer.address(),
        }
        .abi_encode()
        .into(),
    )
    .await
    .context("failed to deploy StakingRewards proxy")?;
    let staking_rewards_proxy = *proxy_instance.address();
    println!("StakingRewards proxy deployed at: {:?}", staking_rewards_proxy);

    let zkc_instance = ZKCInstance::new(zkc_proxy, provider.clone());
    let minter_role = zkc_instance.STAKING_MINTER_ROLE().call().await?;
    zkc_instance.grantRole(minter_role, staking_rewards_proxy).send().await?.watch().await?;
    zkc_instance.approve(signer.address(), U256::MAX).send().await?.watch().await?;
    zkc_instance.approve(signer.address(), U256::MAX).send().await?.watch().await?;

    let vezkc_instance = VeZKCInstance::new(vezkc_proxy, provider.clone());
    let staking_rewards_instance =
        StakingRewardsInstance::new(staking_rewards_proxy, provider.clone());

    let chain_id = anvil.lock().await.chain_id();
    let deployment = Deployment::builder()
        .chain_id(chain_id)
        .zkc_address(zkc_proxy)
        .vezkc_address(vezkc_proxy)
        .staking_rewards_address(staking_rewards_proxy)
        .build()?;

    Ok(TestCtx {
        anvil,
        provider,
        zkc: zkc_instance,
        vezkc: vezkc_instance,
        staking_rewards: staking_rewards_instance,
        deployment,
    })
}
