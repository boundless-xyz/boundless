// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Integration test demonstrating the full PoVW proving pipeline from work receipts
//! to smart contract updates using WorkLogUpdateProver, LogUpdaterProver, and MintCalculatorProver.

use std::ops::Deref;

use alloy::{primitives::U256, signers::local::PrivateKeySigner};
use alloy_provider::Provider;
use boundless_povw::{
    log_updater::{prover::LogUpdaterProver, IPovwAccounting},
    mint_calculator::{prover::MintCalculatorProver, WorkLogFilter},
};
use boundless_test_utils::povw::{make_work_claim, test_ctx};
use risc0_povw::{prover::WorkLogUpdateProver, PovwLogId};
use risc0_steel::ethereum::STEEL_TEST_PRAGUE_CHAIN_SPEC;
use risc0_zkvm::{default_prover, FakeReceipt, ProverOpts, VerifierContext};

#[tokio::test(flavor = "multi_thread")]
async fn test_workflow() -> anyhow::Result<()> {
    // Setup test context with smart contracts
    let ctx = test_ctx().await?;
    let signer = PrivateKeySigner::random();
    let log_id: PovwLogId = signer.address().into();

    // Step 1: Create a fake work receipt
    let work_claim = make_work_claim((log_id, 42), 11, 1 << 20)?; // 11 segments, 1M value
    let work_receipt = FakeReceipt::new(work_claim).into();

    // Step 2: Use WorkLogUpdateProver to create a Log Builder receipt
    let mut work_log_prover = WorkLogUpdateProver::builder()
        .prover(default_prover())
        .log_id(log_id)
        .prover_opts(ProverOpts::default().with_dev_mode(true))
        .verifier_ctx(VerifierContext::default().with_dev_mode(true))
        .build()?;

    let log_builder_prove_info = work_log_prover.prove_update([work_receipt])?;
    let log_builder_receipt = log_builder_prove_info.receipt;

    // Step 3: Use LogUpdaterProver to create a Log Updater receipt
    let log_updater_prover = LogUpdaterProver::builder()
        .prover(default_prover())
        .contract_address(*ctx.povw_accounting.address())
        .chain_id(ctx.chain_id)
        .prover_opts(ProverOpts::default().with_dev_mode(true))
        .verifier_ctx(VerifierContext::default().with_dev_mode(true))
        .build()?;

    let log_updater_prove_info =
        log_updater_prover.prove_update(log_builder_receipt, &signer).await?;

    // Step 4: Post the proven log update to the smart contract
    let tx_receipt = ctx
        .povw_accounting
        .update_work_log(&log_updater_prove_info.receipt)?
        .send()
        .await?
        .get_receipt()
        .await?;

    assert!(tx_receipt.status());

    // Query for the expected WorkLogUpdated event
    let logs = tx_receipt.logs();
    let work_log_updated_events = logs
        .iter()
        .filter_map(|log| log.log_decode::<IPovwAccounting::WorkLogUpdated>().ok())
        .collect::<Vec<_>>();

    assert_eq!(work_log_updated_events.len(), 1, "Expected exactly one WorkLogUpdated event");
    let event = &work_log_updated_events[0].inner.data;
    assert_eq!(event.workLogId, signer.address());
    assert_eq!(event.updateValue, 1 << 20);
    assert_eq!(event.valueRecipient, signer.address());

    // Step 5: Advance time and finalize the epoch
    let initial_epoch = ctx.zkc.getCurrentEpoch().call().await?;
    println!("Current epoch: {initial_epoch}");

    ctx.advance_epochs(U256::ONE).await?;
    let finalized_event = ctx.finalize_epoch().await?;

    assert_eq!(finalized_event.epoch, initial_epoch);
    assert_eq!(finalized_event.totalWork, U256::from(1 << 20)); // Our work value
    println!(
        "EpochFinalized event verified: epoch={}, totalWork={}",
        finalized_event.epoch, finalized_event.totalWork
    );

    // Step 6: Use MintCalculatorProver to create a mint proof
    // NOTE: In a real application, don't use 0..=latest_block. Instead, only include blocks that
    // have either WorkLogUpdated or EpochFinalized events.
    let latest_block = ctx.provider.get_block_number().await?;
    let block_numbers: Vec<u64> = (0..=latest_block).collect();

    let mint_calculator_prover = MintCalculatorProver::builder()
        .prover(default_prover())
        .provider(ctx.provider.clone())
        .povw_accounting_address(*ctx.povw_accounting.address())
        .zkc_address(*ctx.zkc.address())
        .zkc_rewards_address(*ctx.zkc_rewards.address())
        .chain_spec(STEEL_TEST_PRAGUE_CHAIN_SPEC.deref())
        .prover_opts(ProverOpts::default().with_dev_mode(true))
        .verifier_ctx(VerifierContext::default().with_dev_mode(true))
        .build()?;

    let mint_input =
        mint_calculator_prover.build_input(block_numbers, WorkLogFilter::any()).await?;
    let mint_prove_info = mint_calculator_prover.prove_mint(&mint_input).await?;

    // Step 7: Post the mint proof.
    let mint_tx_receipt = ctx
        .povw_mint
        .mint_with_receipt(&mint_prove_info.receipt)?
        .send()
        .await?
        .get_receipt()
        .await?;

    assert!(mint_tx_receipt.status());
    println!("Mint transaction succeeded with {} gas used", mint_tx_receipt.gas_used);

    // Step 8: Verify the mint was successful by checking the recipient's balance
    let final_balance = ctx.zkc.balanceOf(signer.address()).call().await?;
    let epoch_reward = ctx.zkc.getPoVWEmissionsForEpoch(finalized_event.epoch).call().await?;

    assert_eq!(final_balance, epoch_reward, "Minted amount should match expected calculation");
    Ok(())
}
