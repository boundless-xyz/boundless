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

use crate::contracts::{
    boundless_market::BoundlessMarketService,
    bytecode::*,
    hit_points::{default_allowance, HitPointsService},
};
use alloy::{
    network::EthereumWallet,
    node_bindings::AnvilInstance,
    primitives::{Address, FixedBytes},
    providers::{ext::AnvilApi, Provider, ProviderBuilder, WalletProvider},
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use alloy_primitives::B256;
use anyhow::{Context, Ok, Result};
use risc0_aggregation::SetInclusionReceiptVerifierParameters;
use risc0_circuit_recursion::control_id::{ALLOWED_CONTROL_ROOT, BN254_IDENTITY_CONTROL_ID};
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::{
    is_dev_mode,
    sha::{Digest, Digestible},
    Groth16ReceiptVerifierParameters,
};

pub struct TestCtx<P> {
    pub verifier_address: Address,
    pub set_verifier_address: Address,
    pub hit_points_address: Address,
    pub boundless_market_address: Address,
    pub prover_signer: PrivateKeySigner,
    pub customer_signer: PrivateKeySigner,
    pub prover_provider: P,
    pub prover_market: BoundlessMarketService<P>,
    pub customer_provider: P,
    pub customer_market: BoundlessMarketService<P>,
    pub set_verifier: SetVerifierService<P>,
    pub hit_points_service: HitPointsService<P>,
}

pub async fn deploy_verifier_router<P: Provider>(
    deployer_provider: P,
    owner: Address,
) -> Result<Address> {
    let instance = RiscZeroVerifierRouter::deploy(deployer_provider, owner)
        .await
        .context("failed to deploy RiscZeroVerifierRouter")?;
    Ok(*instance.address())
}

pub async fn deploy_groth16_verifier<P: Provider>(
    deployer_provider: P,
    control_root: B256,
    bn254_control_id: B256,
) -> Result<Address> {
    let instance =
        RiscZeroGroth16Verifier::deploy(deployer_provider, control_root, bn254_control_id)
            .await
            .context("failed to deploy RiscZeroGroth16Verifier")?;
    Ok(*instance.address())
}

pub async fn deploy_mock_verifier<P: Provider>(deployer_provider: P) -> Result<Address> {
    let instance = RiscZeroMockVerifier::deploy(deployer_provider, FixedBytes::ZERO)
        .await
        .context("failed to deploy RiscZeroMockVerifier")?;
    Ok(*instance.address())
}

pub async fn deploy_set_verifier<P: Provider>(
    deployer_provider: P,
    verifier_address: Address,
    set_builder_id: Digest,
) -> Result<Address> {
    let instance = RiscZeroSetVerifier::deploy(
        deployer_provider,
        verifier_address,
        <[u8; 32]>::from(set_builder_id).into(),
        String::default(),
    )
    .await
    .context("failed to deploy RiscZeroSetVerifier")?;
    Ok(*instance.address())
}

pub async fn deploy_hit_points<P: Provider>(
    owner_address: Address,
    deployer_provider: P,
) -> Result<Address> {
    let instance = HitPoints::deploy(deployer_provider, owner_address)
        .await
        .context("failed to deploy HitPoints contract")?;
    Ok(*instance.address())
}

pub async fn deploy_boundless_market<P: Provider>(
    owner_address: Address,
    deployer_provider: P,
    verifier: Address,
    hit_points: Address,
    assessor_guest_id: Digest,
    allowed_prover: Option<Address>,
) -> Result<Address> {
    let market_instance = BoundlessMarket::deploy(
        &deployer_provider,
        verifier,
        <[u8; 32]>::from(assessor_guest_id).into(),
        hit_points,
    )
    .await
    .context("failed to deploy BoundlessMarket implementation")?;

    let proxy_instance = ERC1967Proxy::deploy(
        &deployer_provider,
        *market_instance.address(),
        BoundlessMarket::initializeCall { initialOwner: owner_address, imageUrl: "".to_string() }
            .abi_encode()
            .into(),
    )
    .await
    .context("failed to deploy BoundlessMarket proxy")?;
    let proxy = *proxy_instance.address();

    if hit_points != Address::ZERO {
        let hit_points_service =
            HitPointsService::new(hit_points, &deployer_provider, owner_address);
        hit_points_service.grant_minter_role(hit_points_service.caller()).await?;
        hit_points_service.grant_authorized_transfer_role(proxy).await?;
        if let Some(prover) = allowed_prover {
            hit_points_service.mint(prover, default_allowance()).await?;
        }
    }

    Ok(proxy)
}

async fn deploy_contracts(
    anvil: &AnvilInstance,
    set_builder_id: Digest,
    assessor_guest_id: Digest,
) -> Result<(Address, Address, Address, Address)> {
    let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let deployer_address = deployer_signer.address();
    let deployer_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(deployer_signer.clone()))
        .on_builtin(&anvil.endpoint())
        .await?;

    // Deploy contracts
    let verifier_router = deploy_verifier_router(&deployer_provider, deployer_address).await?;
    let verifier = match is_dev_mode() {
        true => deploy_mock_verifier(&deployer_provider).await?,
        false => {
            let control_root = ALLOWED_CONTROL_ROOT;
            let mut bn254_control_id = BN254_IDENTITY_CONTROL_ID;
            bn254_control_id.as_mut_bytes().reverse();
            deploy_groth16_verifier(
                &deployer_provider,
                <[u8; 32]>::from(control_root).into(),
                <[u8; 32]>::from(bn254_control_id).into(),
            )
            .await?
        }
    };
    let set_verifier = deploy_set_verifier(&deployer_provider, verifier, set_builder_id).await?;

    let router_instance = RiscZeroVerifierRouter::RiscZeroVerifierRouterInstance::new(
        verifier_router,
        deployer_provider.clone(),
    );

    let verifier_parameters_digest = Groth16ReceiptVerifierParameters::default().digest();
    let groth16_selector: [u8; 4] = match is_dev_mode() {
        true => [0u8; 4],
        false => verifier_parameters_digest.as_bytes()[..4].try_into()?,
    };
    let call = &router_instance
        .addVerifier(groth16_selector.into(), verifier)
        .from(deployer_signer.address());
    let _ = call.send().await?;

    let verifier_parameters_digest =
        SetInclusionReceiptVerifierParameters { image_id: set_builder_id }.digest();
    let set_verifier_selector: [u8; 4] = verifier_parameters_digest.as_bytes()[..4].try_into()?;
    let call = &router_instance
        .addVerifier(set_verifier_selector.into(), set_verifier)
        .from(deployer_signer.address());
    let _ = call.send().await?;

    let hit_points = deploy_hit_points(deployer_address, &deployer_provider).await?;
    let boundless_market = deploy_boundless_market(
        deployer_address,
        &deployer_provider,
        verifier_router,
        hit_points,
        assessor_guest_id,
        None,
    )
    .await?;

    // Mine forward some blocks using the provider
    deployer_provider.anvil_mine(Some(10), Some(2)).await.unwrap();
    deployer_provider.anvil_set_interval_mining(2).await.unwrap();

    Ok((verifier, set_verifier, hit_points, boundless_market))
}

// Spin up a test deployment with a RiscZeroMockVerifier if in dev mode or
// with a RiscZeroGroth16Verifier otherwise.
pub async fn create_test_ctx(
    anvil: &AnvilInstance,
    set_builder_id: impl Into<Digest>,
    assessor_guest_id: impl Into<Digest>,
) -> Result<TestCtx<impl Provider + WalletProvider + Clone + 'static>> {
    create_test_ctx_with_rpc_url(anvil, &anvil.endpoint(), set_builder_id, assessor_guest_id).await
}

pub async fn create_test_ctx_with_rpc_url(
    anvil: &AnvilInstance,
    rpc_url: &str,
    set_builder_id: impl Into<Digest>,
    assessor_guest_id: impl Into<Digest>,
) -> Result<TestCtx<impl Provider + WalletProvider + Clone + 'static>> {
    let (verifier_addr, set_verifier_addr, hit_points_addr, boundless_market_addr) =
        deploy_contracts(anvil, set_builder_id.into(), assessor_guest_id.into()).await.unwrap();

    let prover_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
    let customer_signer: PrivateKeySigner = anvil.keys()[2].clone().into();
    let verifier_signer: PrivateKeySigner = anvil.keys()[0].clone().into();

    let prover_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(prover_signer.clone()))
        .on_builtin(rpc_url)
        .await?;
    let customer_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(customer_signer.clone()))
        .on_builtin(rpc_url)
        .await?;
    let verifier_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(verifier_signer.clone()))
        .on_builtin(rpc_url)
        .await?;

    let prover_market = BoundlessMarketService::new(
        boundless_market_addr,
        prover_provider.clone(),
        prover_signer.address(),
    );

    let customer_market = BoundlessMarketService::new(
        boundless_market_addr,
        customer_provider.clone(),
        customer_signer.address(),
    );

    let set_verifier = SetVerifierService::new(
        set_verifier_addr,
        verifier_provider.clone(),
        verifier_signer.address(),
    );

    let hit_points_service = HitPointsService::new(
        hit_points_addr,
        verifier_provider.clone(),
        verifier_signer.address(),
    );

    hit_points_service.mint(prover_signer.address(), default_allowance()).await?;

    Ok(TestCtx {
        verifier_address: verifier_addr,
        set_verifier_address: set_verifier_addr,
        hit_points_address: hit_points_addr,
        boundless_market_address: boundless_market_addr,
        prover_signer,
        customer_signer,
        prover_provider,
        prover_market,
        customer_provider,
        customer_market,
        set_verifier,
        hit_points_service,
    })
}
