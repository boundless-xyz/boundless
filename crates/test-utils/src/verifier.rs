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

use alloy::{
    primitives::{Address, FixedBytes},
    providers::Provider,
};
use anyhow::{Context, Result};
use boundless_market::contracts::bytecode::RiscZeroVerifierRouter::RiscZeroVerifierRouterInstance;
use boundless_market::contracts::bytecode::{
    Blake3Groth16Verifier, RiscZeroGroth16Verifier, RiscZeroMockVerifier, RiscZeroSetVerifier,
    RiscZeroVerifierRouter,
};
use risc0_aggregation::SetInclusionReceiptVerifierParameters;
use risc0_circuit_recursion::control_id::{ALLOWED_CONTROL_ROOT, BN254_IDENTITY_CONTROL_ID};
use risc0_zkvm::sha::{Digest, Digestible};
use risc0_zkvm::{Groth16ReceiptVerifierParameters, VerifierContext};

/// Deploy a MockRiscZeroVerifier contract for testing
pub async fn deploy_mock_verifier<P: Provider>(deployer_provider: P) -> Result<Address> {
    let instance = RiscZeroMockVerifier::deploy(deployer_provider, FixedBytes([0xFFu8; 4]))
        .await
        .context("failed to deploy RiscZeroMockVerifier")?;
    Ok(*instance.address())
}

pub async fn deploy_mock_blake3_groth16_verifier<P: Provider>(
    deployer_provider: P,
) -> Result<Address> {
    let instance =
        RiscZeroMockVerifier::deploy(deployer_provider, FixedBytes([0xFFu8, 0xFF, 0x00, 0x00]))
            .await
            .context("failed to deploy RiscZeroMockVerifier")?;
    Ok(*instance.address())
}

/// Deploy a RiscZeroGroth16Verifier contract
pub async fn deploy_groth16_verifier<P: Provider>(
    deployer_provider: P,
    control_root: FixedBytes<32>,
    bn254_control_id: FixedBytes<32>,
) -> Result<Address> {
    let instance =
        RiscZeroGroth16Verifier::deploy(deployer_provider, control_root, bn254_control_id)
            .await
            .context("failed to deploy RiscZeroGroth16Verifier")?;
    Ok(*instance.address())
}

/// Deploy a Blake3Groth16Verifier contract
pub async fn deploy_blake3_groth16_verifier<P: Provider>(
    deployer_provider: P,
    control_root: FixedBytes<32>,
    bn254_control_id: FixedBytes<32>,
) -> Result<Address> {
    let instance = Blake3Groth16Verifier::deploy(deployer_provider, control_root, bn254_control_id)
        .await
        .context("failed to deploy Blake3Groth16Verifier")?;
    Ok(*instance.address())
}

/// Deploy a RiscZeroVerifierRouter contract
pub async fn deploy_verifier_router<P: Provider>(
    deployer_provider: P,
    owner: Address,
) -> Result<Address> {
    let instance = RiscZeroVerifierRouter::deploy(deployer_provider, owner)
        .await
        .context("failed to deploy RiscZeroVerifierRouter")?;
    Ok(*instance.address())
}

/// Deploy a RiscZeroSetVerifier contract
pub async fn deploy_set_verifier<P: Provider>(
    deployer_provider: P,
    verifier_address: Address,
    image_id: FixedBytes<32>,
    set_builder_url: String,
) -> Result<Address> {
    let instance =
        RiscZeroSetVerifier::deploy(deployer_provider, verifier_address, image_id, set_builder_url)
            .await
            .context("failed to deploy RiscZeroSetVerifier")?;
    Ok(*instance.address())
}

/// Check if running in dev mode based on VerifierContext
pub fn is_dev_mode() -> bool {
    VerifierContext::default().dev_mode()
}

/// Setup verifiers with router and register them
pub async fn setup_verifiers<P: Provider + Clone>(
    deployer_provider: P,
    deployer_address: Address,
    set_builder_id: FixedBytes<32>,
    set_builder_url: String,
) -> Result<(Address, Address, Address)> {
    // Deploy verifier router
    let verifier_router = deploy_verifier_router(&deployer_provider, deployer_address).await?;

    // Deploy groth16 verifier (mock in dev mode, real otherwise)
    let (groth16_verifier, groth16_selector) = match is_dev_mode() {
        true => (deploy_mock_verifier(&deployer_provider).await?, [0xFFu8; 4]),
        false => {
            let control_root = ALLOWED_CONTROL_ROOT;
            // Byte order in the contract is opposite that of Rust, because the EVM interprets the
            // digest as a big-endian uint256.
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

    // Deploy set verifier
    let set_verifier =
        deploy_set_verifier(&deployer_provider, verifier_router, set_builder_id, set_builder_url)
            .await?;

    // Register verifiers with the router
    let router_instance =
        RiscZeroVerifierRouterInstance::new(verifier_router, deployer_provider.clone());

    // Deploying the blake3 groth16 verifier
    let (blake_groth16_verifier, blake_groth16_selector) = match is_dev_mode() {
        true => (
            deploy_mock_blake3_groth16_verifier(&deployer_provider).await?,
            [0xFFu8, 0xFF, 0x00, 0x00],
        ),
        false => {
            let control_root = ALLOWED_CONTROL_ROOT;
            let mut bn254_control_id = BN254_IDENTITY_CONTROL_ID;
            bn254_control_id.as_mut_bytes().reverse();
            let verifier_parameters_digest = blake3_groth16::verify::verifier_parameters().digest();
            let b3_groth16_verifier = deploy_blake3_groth16_verifier(
                &deployer_provider,
                <[u8; 32]>::from(control_root).into(),
                <[u8; 32]>::from(bn254_control_id).into(),
            )
            .await?;

            let b3_groth16_selector = verifier_parameters_digest.as_bytes()[..4].try_into()?;

            (b3_groth16_verifier, b3_groth16_selector)
        }
    };

    // Add groth16 verifier to router
    let call = &router_instance
        .addVerifier(groth16_selector.into(), groth16_verifier)
        .from(deployer_address);
    call.send().await?.get_receipt().await?;

    // Add set verifier to router
    let verifier_parameters_digest =
        SetInclusionReceiptVerifierParameters { image_id: Digest::from(*set_builder_id) }.digest();
    let set_verifier_selector: [u8; 4] = verifier_parameters_digest.as_bytes()[..4].try_into()?;
    let call = &router_instance
        .addVerifier(set_verifier_selector.into(), set_verifier)
        .from(deployer_address);
    call.send().await?.get_receipt().await?;

    // Add blake3 groth16 verifier to router
    let call = &router_instance
        .addVerifier(blake_groth16_selector.into(), blake_groth16_verifier)
        .from(deployer_address);
    call.send().await?.get_receipt().await?;

    Ok((verifier_router, set_verifier, groth16_verifier))
}
