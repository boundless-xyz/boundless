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

use alloy::{
    network::EthereumWallet,
    node_bindings::AnvilInstance,
    primitives::{Address, Bytes, FixedBytes},
    providers::{ext::AnvilApi, fillers::ChainIdFiller, Provider, ProviderBuilder, WalletProvider},
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
    transports::http::reqwest::Url,
};
use alloy_primitives::{B256, U256};
use alloy_sol_types::{Eip712Domain, SolStruct, SolValue};
use anyhow::{Context, Ok, Result};
use boundless_market::{
    contracts::{
        boundless_market::BoundlessMarketService,
        bytecode::*,
        hit_points::{default_allowance, HitPointsService},
        AssessorCommitment, AssessorJournal, Fulfillment, ProofRequest,
    },
    deployments::Deployment,
    dynamic_gas_filler::DynamicGasFiller,
    nonce_layer::NonceProvider,
};
use risc0_aggregation::{
    merkle_path, merkle_root, GuestState, SetInclusionReceipt,
    SetInclusionReceiptVerifierParameters,
};
use risc0_circuit_recursion::control_id::{ALLOWED_CONTROL_ROOT, BN254_IDENTITY_CONTROL_ID};
use risc0_ethereum_contracts::{encode_seal, set_verifier::SetVerifierService};
use risc0_zkvm::{
    is_dev_mode,
    sha::{Digest, Digestible},
    FakeReceipt, Groth16ReceiptVerifierParameters, InnerReceipt, Journal, MaybePruned, Receipt,
    ReceiptClaim,
};

// Export image IDs and paths publicly to ensure all dependants use the same ones.
pub use guest_assessor::{ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
pub use guest_set_builder::{SET_BUILDER_ELF, SET_BUILDER_ID, SET_BUILDER_PATH};
pub use guest_util::{
    ECHO_ELF, ECHO_ID, ECHO_PATH, IDENTITY_ELF, IDENTITY_ID, IDENTITY_PATH, LOOP_ELF, LOOP_ID,
    LOOP_PATH,
};

/// Re-export of the boundless_market crate, which can be used to avoid dependency issues when
/// writing tests in the boundless_market crate itself, where two version of boundless_market end
/// up in the Cargo dependency tree due to the way cycle-breaking works.
pub use boundless_market;

#[non_exhaustive]
pub struct TestCtx<P> {
    pub deployment: Deployment,
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
    let instance = RiscZeroMockVerifier::deploy(deployer_provider, FixedBytes([0xFFu8; 4]))
        .await
        .context("failed to deploy RiscZeroMockVerifier")?;
    Ok(*instance.address())
}

pub async fn deploy_set_verifier<P: Provider>(
    deployer_provider: P,
    verifier_address: Address,
    set_builder_id: Digest,
    set_builder_url: String,
) -> Result<Address> {
    let instance = RiscZeroSetVerifier::deploy(
        deployer_provider,
        verifier_address,
        <[u8; 32]>::from(set_builder_id).into(),
        set_builder_url,
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
    assessor_guest_url: String,
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
        BoundlessMarket::initializeCall {
            initialOwner: owner_address,
            imageUrl: assessor_guest_url,
        }
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

pub async fn deploy_mock_callback<P: Provider>(
    deployer_provider: P,
    verifier: Address,
    boundless_market_address: Address,
    image_id: impl Into<Digest>,
    target_gas: U256,
) -> Result<Address> {
    let mock_callback_instance = MockCallback::deploy(
        &deployer_provider,
        verifier,
        boundless_market_address,
        <[u8; 32]>::from(image_id.into()).into(),
        target_gas,
    )
    .await
    .context("failed to deploy MockCallback contract")?;

    Ok(*mock_callback_instance.address())
}

pub async fn get_mock_callback_count(provider: &impl Provider, address: Address) -> Result<U256> {
    let instance = MockCallback::MockCallbackInstance::new(address, provider);
    Ok(instance.getCallCount().call().await?)
}

pub async fn deploy_contracts(
    anvil: &AnvilInstance,
    set_builder_id: Digest,
    set_builder_url: String,
    assessor_guest_id: Digest,
    assessor_guest_url: String,
) -> Result<(Address, Address, Address, Address)> {
    let deployer_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let deployer_address = deployer_signer.address();
    let deployer_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(deployer_signer.clone()))
        .connect(&anvil.endpoint())
        .await?;

    println!("PRE DEPLOY PROVIDER: {:?}", deployer_provider);

    // Deploy contracts
    let verifier_router = deploy_verifier_router(&deployer_provider, deployer_address).await?;
    let (verifier, groth16_selector) = match is_dev_mode() {
        true => (deploy_mock_verifier(&deployer_provider).await?, [0xFFu8; 4]),
        false => {
            let control_root = ALLOWED_CONTROL_ROOT;
            let mut bn254_control_id = BN254_IDENTITY_CONTROL_ID;
            bn254_control_id.as_mut_bytes().reverse();
            let verifier_parameters_digest = Groth16ReceiptVerifierParameters::default().digest();
            (
                deploy_groth16_verifier(
                    &deployer_provider,
                    <[u8; 32]>::from(control_root).into(),
                    <[u8; 32]>::from(bn254_control_id).into(),
                )
                .await?,
                verifier_parameters_digest.as_bytes()[..4].try_into()?,
            )
        }
    };
    let set_verifier =
        deploy_set_verifier(&deployer_provider, verifier, set_builder_id, set_builder_url).await?;

    let router_instance = RiscZeroVerifierRouter::RiscZeroVerifierRouterInstance::new(
        verifier_router,
        deployer_provider.clone(),
    );

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
        assessor_guest_url,
        None,
    )
    .await?;

    // Mine forward some blocks using the provider
    deployer_provider.anvil_mine(Some(10), Some(2)).await.unwrap();
    deployer_provider.anvil_set_interval_mining(2).await.unwrap();

    Ok((verifier_router, set_verifier, hit_points, boundless_market))
}

// Spin up a test deployment with a RiscZeroMockVerifier if in dev mode or
// with a RiscZeroGroth16Verifier otherwise.
pub async fn create_test_ctx(
    anvil: &AnvilInstance,
) -> Result<TestCtx<impl Provider + WalletProvider + Clone + 'static>> {
    create_test_ctx_with_rpc_url(anvil, &anvil.endpoint()).await
}

pub async fn create_test_ctx_with_rpc_url(
    anvil: &AnvilInstance,
    rpc_url: &str,
) -> Result<TestCtx<impl Provider + WalletProvider + Clone + 'static>> {
    // NOTE: There may be use cases for making these configurable, but its not obviously the case.
    let set_builder_id = Digest::from(SET_BUILDER_ID);
    let set_builder_url = format!("file://{SET_BUILDER_PATH}");
    let assessor_guest_id = Digest::from(ASSESSOR_GUEST_ID);
    let assessor_guest_url = format!("file://{ASSESSOR_GUEST_PATH}");

    let (verifier_addr, set_verifier_addr, hit_points_addr, boundless_market_addr) =
        deploy_contracts(
            anvil,
            set_builder_id,
            set_builder_url,
            assessor_guest_id,
            assessor_guest_url,
        )
        .await
        .unwrap();

    let prover_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
    let customer_signer: PrivateKeySigner = anvil.keys()[2].clone().into();
    let verifier_signer: PrivateKeySigner = anvil.keys()[0].clone().into();

    let dynamic_gas_filler = DynamicGasFiller::new(
        0.2,  // 20% increase of gas limit
        0.05, // 5% increase of gas_price per pending transaction
        2.0,  // 2x max gas multiplier
        prover_signer.address(),
    );
    let base_prover_provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(ChainIdFiller::default())
        .filler(dynamic_gas_filler)
        .connect_http(Url::parse(rpc_url).unwrap());
    let prover_provider =
        NonceProvider::new(base_prover_provider, EthereumWallet::from(prover_signer.clone()));

    let dynamic_gas_filler = DynamicGasFiller::new(0.2, 0.05, 2.0, customer_signer.address());
    let base_customer_provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(ChainIdFiller::default())
        .filler(dynamic_gas_filler)
        .connect_http(Url::parse(rpc_url).unwrap());
    let customer_provider =
        NonceProvider::new(base_customer_provider, EthereumWallet::from(customer_signer.clone()));

    let dynamic_gas_filler = DynamicGasFiller::new(0.2, 0.05, 2.0, verifier_signer.address());
    let base_verifier_provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(ChainIdFiller::default())
        .filler(dynamic_gas_filler)
        .connect_http(Url::parse(rpc_url).unwrap());
    let verifier_provider =
        NonceProvider::new(base_verifier_provider, EthereumWallet::from(verifier_signer.clone()));

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
        deployment: Deployment::builder()
            .chain_id(anvil.chain_id())
            .boundless_market_address(boundless_market_addr)
            .verifier_router_address(verifier_addr)
            .set_verifier_address(set_verifier_addr)
            .stake_token_address(hit_points_addr)
            .build()?,
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

fn to_b256(digest: Digest) -> B256 {
    <[u8; 32]>::from(digest).into()
}

pub fn mock_singleton(
    request: &ProofRequest,
    eip712_domain: Eip712Domain,
    prover: Address,
) -> (B256, Bytes, Fulfillment, Bytes) {
    let app_journal = Journal::new(vec![0x41, 0x41, 0x41, 0x41]);
    let app_receipt_claim = ReceiptClaim::ok(ECHO_ID, app_journal.clone().bytes);
    let app_claim_digest = app_receipt_claim.digest();
    let request_digest = request.eip712_signing_hash(&eip712_domain);
    let assessor_root = AssessorCommitment {
        index: U256::ZERO,
        id: request.id,
        requestDigest: request_digest,
        claimDigest: <[u8; 32]>::from(app_claim_digest).into(),
    }
    .eip712_hash_struct();
    let assessor_journal =
        AssessorJournal { selectors: vec![], root: assessor_root, prover, callbacks: vec![] };
    let assesor_receipt_claim = ReceiptClaim::ok(ASSESSOR_GUEST_ID, assessor_journal.abi_encode());
    let assessor_claim_digest = assesor_receipt_claim.digest();

    let set_builder_root = merkle_root(&[app_claim_digest, assessor_claim_digest]);
    let set_builder_journal = {
        let mut state = GuestState::initial(SET_BUILDER_ID);
        state.mmr.push(app_claim_digest).unwrap();
        state.mmr.push(assessor_claim_digest).unwrap();
        state.mmr.finalize().unwrap();
        state.encode()
    };
    let set_builder_receipt_claim = ReceiptClaim::ok(SET_BUILDER_ID, set_builder_journal.clone());

    let set_builder_receipt = Receipt::new(
        InnerReceipt::Fake(FakeReceipt::new(set_builder_receipt_claim)),
        set_builder_journal,
    );
    let set_builder_seal = encode_seal(&set_builder_receipt).unwrap();

    let verifier_parameters =
        SetInclusionReceiptVerifierParameters { image_id: Digest::from(SET_BUILDER_ID) };
    let set_inclusion_seal = SetInclusionReceipt::from_path_with_verifier_params(
        ReceiptClaim::ok(ECHO_ID, MaybePruned::Pruned(app_journal.digest())),
        merkle_path(&[app_claim_digest, assessor_claim_digest], 0),
        verifier_parameters.digest(),
    )
    .abi_encode_seal()
    .unwrap();

    let fulfillment = Fulfillment {
        id: request.id,
        requestDigest: request_digest,
        imageId: to_b256(Digest::from(ECHO_ID)),
        journal: app_journal.bytes.into(),
        seal: set_inclusion_seal.into(),
    };

    let assessor_seal = SetInclusionReceipt::from_path_with_verifier_params(
        ReceiptClaim::ok(ASSESSOR_GUEST_ID, MaybePruned::Pruned(Digest::ZERO)),
        merkle_path(&[app_claim_digest, assessor_claim_digest], 1),
        verifier_parameters.digest(),
    )
    .abi_encode_seal()
    .unwrap();

    (to_b256(set_builder_root), set_builder_seal.into(), fulfillment, assessor_seal.into())
}
