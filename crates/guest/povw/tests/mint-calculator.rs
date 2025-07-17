// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    network::EthereumWallet,
    node_bindings::Anvil,
    primitives::{Address, Bytes, FixedBytes, U256},
    providers::{Provider, ProviderBuilder, WalletProvider},
    signers::local::PrivateKeySigner,
    sol,
};
use risc0_ethereum_contracts::{encode_seal, selector::Selector};

// Import the Solidity contracts using alloy's sol! macro
// Use the compiled contracts output to allow for deploying the contracts.
// NOTE: This requires running `forge build` before running this test.
// TODO: Work on making this more robust.
sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    MockRiscZeroVerifier,
    "../../../contracts/out/RiscZeroMockVerifier.sol/RiscZeroMockVerifier.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    MockERC20Mint,
    "../../../contracts/out/MockERC20Mint.sol/MockERC20Mint.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    PoVW,
    "../../../contracts/out/PoVW.sol/PoVW.json"
);

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    Mint,
    "../../../contracts/out/Mint.sol/Mint.json"
);

#[tokio::test]
async fn basic() -> anyhow::Result<()> {
    // 1. Create an Anvil instance.
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    
    // Create wallet and provider
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(rpc_url);

    // 2. Deploy PoVW and Mint contracts to the Anvil instance, using a MockRiscZeroVerifier and a
    //    basic ERC-20.
    
    // Deploy MockRiscZeroVerifier
    let mock_verifier = MockRiscZeroVerifier::deploy(&provider, FixedBytes([0xFFu8; 4])).await?;
    println!("MockRiscZeroVerifier deployed at: {:?}", mock_verifier.address());
    
    // Deploy MockERC20 token
    let mock_token = MockERC20Mint::deploy(&provider).await?;
    println!("MockERC20 deployed at: {:?}", mock_token.address());
    
    // Deploy PoVW contract (needs verifier and log builder ID)
    let log_builder_id = [0u8; 32]; // Placeholder for actual log builder ID
    let povw_contract = PoVW::deploy(&provider, *mock_verifier.address(), log_builder_id.into()).await?;
    println!("PoVW contract deployed at: {:?}", povw_contract.address());
    
    // Deploy Mint contract (needs verifier, povw, mint calculator ID, and token)
    let mint_calculator_id = [0u8; 32]; // Placeholder for actual mint calculator ID
    let mint_contract = Mint::deploy(
        &provider,
        *mock_verifier.address(),
        *povw_contract.address(),
        mint_calculator_id.into(),
        *mock_token.address()
    ).await?;
    println!("Mint contract deployed at: {:?}", mint_contract.address());

    // 3. Post a single work log update to the PoVW contract.
    // 4. Advance time by 1 epoch.
    // 5. Finalize the epoch.
    // 6. Query for WorkLogUpdated and EpochFinalized events, recording the block numbers that
    //    include these events.
    // 7. Preflight the Steel queries that the guest will make to assemble the guest input.
    // 8. Run the mint calculator guest.
    // 9. Assemble a fake receipt and use it to call the mint function on the Mint contract.
    // 10. Verify that the minted values are as expected.
    todo!("Steps 3-10 to be implemented next")
}
