// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

// Some of this code is used by the log_updater test and some by mint_calculator test. Each does
// its own dead code analysis and so will report code used only by the other as dead.
#![allow(dead_code)]

use std::{collections::BTreeSet, sync::Arc};

use alloy::{
    network::EthereumWallet,
    node_bindings::{Anvil, AnvilInstance},
    primitives::{Address, FixedBytes},
    providers::{ext::AnvilApi, DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionReceipt,
    signers::local::PrivateKeySigner,
    sol,
};
use alloy_signer::Signer;
use alloy_sol_types::SolValue;
use anyhow::bail;
use boundless_povw_guests::{
    log_updater::{self, IPovwAccounting, LogBuilderJournal, WorkLogUpdate},
    mint_calculator::{self, host::IPovwMint::IPovwMintInstance},
    BOUNDLESS_POVW_LOG_UPDATER_ELF, BOUNDLESS_POVW_LOG_UPDATER_ID,
    BOUNDLESS_POVW_MINT_CALCULATOR_ELF, BOUNDLESS_POVW_MINT_CALCULATOR_ID,
};
use risc0_povw_guests::RISC0_POVW_LOG_BUILDER_ID;
use risc0_steel::ethereum::ANVIL_CHAIN_SPEC;
use risc0_zkvm::{
    default_executor, sha::Digestible, ExecutorEnv, ExitCode, FakeReceipt, InnerReceipt, Receipt,
    ReceiptClaim,
};
use tokio::sync::Mutex;

// Import the Solidity contracts using alloy's sol! macro
// Use the compiled contracts output to allow for deploying the contracts.
// NOTE: This requires running `forge build` before running this test.
// TODO: Work on making this more robust.
sol!(
    #[sol(rpc)]
    MockRiscZeroVerifier,
    "../../../out/RiscZeroMockVerifier.sol/RiscZeroMockVerifier.json"
);

sol!(
    #[allow(clippy::too_many_arguments)]
    #[sol(rpc)]
    MockERC20Mint,
    "../../../out/MockERC20Mint.sol/MockERC20Mint.json"
);

sol!(
    #[sol(rpc)]
    #[derive(Debug)]
    PovwAccounting,
    "../../../out/PovwAccounting.sol/PovwAccounting.json"
);

sol!(
    #[sol(rpc)]
    PovwMint,
    "../../../out/PovwMint.sol/PovwMint.json"
);

#[derive(Clone)]
pub struct TestCtx {
    pub anvil: Arc<Mutex<AnvilInstance>>,
    pub chain_id: u64,
    pub provider: DynProvider,
    pub token_contract: MockERC20Mint::MockERC20MintInstance<DynProvider>,
    pub povw_contract: PovwAccounting::PovwAccountingInstance<DynProvider>,
    pub mint_contract: IPovwMintInstance<DynProvider>,
}

pub async fn text_ctx() -> anyhow::Result<TestCtx> {
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
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url).erased();

    // Deploy PovwAccounting and PovwMint contracts to the Anvil instance, using a MockRiscZeroVerifier and a
    // basic ERC-20.

    // Deploy MockRiscZeroVerifier
    let mock_verifier =
        MockRiscZeroVerifier::deploy(provider.clone(), FixedBytes([0xFFu8; 4])).await?;
    println!("MockRiscZeroVerifier deployed at: {:?}", mock_verifier.address());

    // Deploy MockERC20 token
    let token_contract = MockERC20Mint::deploy(provider.clone()).await?;
    println!("MockERC20 deployed at: {:?}", token_contract.address());

    // Deploy PovwAccounting contract (needs verifier and log builder ID)
    let povw_contract = PovwAccounting::deploy(
        provider.clone(),
        *mock_verifier.address(),
        bytemuck::cast::<_, [u8; 32]>(BOUNDLESS_POVW_LOG_UPDATER_ID).into(),
    )
    .await?;
    println!("PovwAccounting contract deployed at: {:?}", povw_contract.address());

    // Deploy PovwMint contract (needs verifier, povw, mint calculator ID, and token)
    let mint_contract = PovwMint::deploy(
        provider.clone(),
        *mock_verifier.address(),
        *povw_contract.address(),
        bytemuck::cast::<_, [u8; 32]>(BOUNDLESS_POVW_MINT_CALCULATOR_ID).into(),
        *token_contract.address(),
    )
    .await?;
    println!("PovwMint contract deployed at: {:?}", mint_contract.address());

    // Cast the deployed PovwMintInstance to an IPovwMintInstance from the source crate, which is
    // considered a fully independent type by Rust.
    let mint_interface = IPovwMintInstance::new(*mint_contract.address(), provider.clone());

    let chain_id = anvil.lock().await.chain_id();
    Ok(TestCtx {
        anvil,
        chain_id,
        provider,
        token_contract,
        povw_contract,
        mint_contract: mint_interface,
    })
}

impl TestCtx {
    pub async fn advance_epochs(&self, epochs: u32) -> anyhow::Result<u32> {
        let initial_epoch = self.povw_contract.currentEpoch().call().await?;

        // Advance time on the Anvil instance, forward to the next epoch.
        let epoch_length = self.povw_contract.EPOCH_LENGTH().call().await?;
        let advance_time = epochs * epoch_length.to::<u32>();

        self.provider.anvil_increase_time(advance_time as u64).await?;
        self.provider.anvil_mine(Some(1), None).await?;

        let new_epoch = self.povw_contract.currentEpoch().call().await?;
        assert_eq!(new_epoch, initial_epoch + epochs, "Epoch should have advanced by {epochs}");
        println!("Time advanced: epoch {} -> {}", initial_epoch, new_epoch);
        Ok(new_epoch)
    }

    pub async fn post_work_log_update(
        &self,
        signer: &impl Signer,
        update: &LogBuilderJournal,
        value_recipient: Address,
    ) -> anyhow::Result<IPovwAccounting::WorkLogUpdated> {
        let signature = WorkLogUpdate::from_log_builder_journal(update.clone(), value_recipient)
            .sign(signer, *self.povw_contract.address(), self.chain_id)
            .await?;

        // Use execute_log_updater_guest to get a Journal.
        let input = log_updater::Input {
            update: update.clone(),
            value_recipient,
            signature: signature.as_bytes().to_vec(),
            contract_address: *self.povw_contract.address(),
            chain_id: self.chain_id,
        };
        let journal = execute_log_updater_guest(&input)?;
        println!("Guest execution completed, journal: {:#?}", journal);

        let fake_receipt: Receipt =
            FakeReceipt::new(ReceiptClaim::ok(BOUNDLESS_POVW_LOG_UPDATER_ID, journal.abi_encode()))
                .try_into()?;

        // Call the PovwAccounting.updateWorkLog function and confirm that it does not revert.
        let tx_result = self
            .povw_contract
            .updateWorkLog(
                journal.update.workLogId,
                journal.update.updatedCommit,
                journal.update.updateValue,
                journal.update.valueRecipient,
                encode_seal(&fake_receipt)?.into(),
            )
            .send()
            .await?;
        println!("updateWorkLog transaction sent: {:?}", tx_result.tx_hash());

        // Query for the expected WorkLogUpdated event.
        let receipt = tx_result.get_receipt().await?;
        let logs = receipt.logs();

        // Find the WorkLogUpdated event
        let work_log_updated_events = logs
            .iter()
            .filter_map(|log| log.log_decode::<IPovwAccounting::WorkLogUpdated>().ok())
            .collect::<Vec<_>>();

        assert_eq!(work_log_updated_events.len(), 1, "Expected exactly one WorkLogUpdated event");
        let update_event = &work_log_updated_events[0].inner.data;
        Ok(update_event.clone())
    }

    pub async fn finalize_epoch(&self) -> anyhow::Result<IPovwAccounting::EpochFinalized> {
        let finalize_tx = self.povw_contract.finalizeEpoch().send().await?;
        println!("finalizeEpoch transaction sent: {:?}", finalize_tx.tx_hash());

        let finalize_receipt = finalize_tx.get_receipt().await?;
        let finalize_logs = finalize_receipt.logs();

        // Find the EpochFinalized event
        let epoch_finalized_events = finalize_logs
            .iter()
            .filter_map(|log| log.log_decode::<IPovwAccounting::EpochFinalized>().ok())
            .collect::<Vec<_>>();

        assert_eq!(epoch_finalized_events.len(), 1, "Expected exactly one EpochFinalized event");
        Ok(epoch_finalized_events[0].inner.data.clone())
    }

    pub async fn build_mint_input(&self) -> anyhow::Result<mint_calculator::Input> {
        self.build_mint_input_for_epochs(&[]).await
    }

    pub async fn build_mint_input_for_epochs(
        &self,
        epochs: &[u32],
    ) -> anyhow::Result<mint_calculator::Input> {
        // Query for WorkLogUpdated and EpochFinalized events, recording the block numbers that include these events.
        let latest_block = self.provider.get_block_number().await?;
        let epoch_filter_str = if epochs.is_empty() {
            "all epochs".to_string()
        } else {
            format!("epochs {:?}", epochs)
        };
        println!("Running mint operation for blocks: 0 to {latest_block}, filtering for {epoch_filter_str}");

        // Query for WorkLogUpdated events
        let work_log_filter =
            self.povw_contract.WorkLogUpdated_filter().from_block(0).to_block(latest_block);
        let work_log_events = work_log_filter.query().await?;
        println!("Found {} total WorkLogUpdated events", work_log_events.len());

        // Query for EpochFinalized events
        let epoch_finalized_filter =
            self.povw_contract.EpochFinalized_filter().from_block(0).to_block(latest_block);
        let epoch_finalized_events = epoch_finalized_filter.query().await?;
        println!("Found {} total EpochFinalized events", epoch_finalized_events.len());

        // Filter events by epoch if specified
        let filtered_work_log_events: Vec<_> = if epochs.is_empty() {
            work_log_events
        } else {
            work_log_events
                .into_iter()
                .filter(|(event, _)| epochs.contains(&event.epochNumber.to::<u32>()))
                .collect()
        };

        let filtered_epoch_finalized_events: Vec<_> = if epochs.is_empty() {
            epoch_finalized_events
        } else {
            epoch_finalized_events
                .into_iter()
                .filter(|(event, _)| epochs.contains(&event.epoch.to::<u32>()))
                .collect()
        };

        println!(
            "After filtering: {} WorkLogUpdated events, {} EpochFinalized events",
            filtered_work_log_events.len(),
            filtered_epoch_finalized_events.len()
        );

        // Collect and sort unique block numbers that contain filtered events.
        let mut block_numbers = BTreeSet::new();
        for (event, log) in &filtered_work_log_events {
            if let Some(block_number) = log.block_number {
                block_numbers.insert(block_number);
                println!(
                    "WorkLogUpdated event at block {} (epoch {})",
                    block_number, event.epochNumber
                );
            }
        }
        for (event, log) in &filtered_epoch_finalized_events {
            if let Some(block_number) = log.block_number {
                block_numbers.insert(block_number);
                println!("EpochFinalized event at block {} (epoch {})", block_number, event.epoch);
            }
        }

        let sorted_blocks: Vec<u64> = block_numbers.into_iter().collect();
        println!("Block numbers with events: {:?}", sorted_blocks);

        // Build the input for the mint_calculator, including input for Steel.
        let mint_input = mint_calculator::Input::build(
            *self.povw_contract.address(),
            self.provider.clone(),
            &ANVIL_CHAIN_SPEC,
            sorted_blocks,
        )
        .await?;

        println!("Mint calculator input built with {} blocks", mint_input.env.0.len());
        Ok(mint_input)
    }

    pub async fn run_mint(&self) -> anyhow::Result<TransactionReceipt> {
        self.run_mint_for_epochs(&[]).await
    }

    pub async fn run_mint_for_epochs(&self, epochs: &[u32]) -> anyhow::Result<TransactionReceipt> {
        let mint_input = self.build_mint_input_for_epochs(epochs).await?;

        // Execute the mint calculator guest
        let mint_journal = execute_mint_calculator_guest(&mint_input)?;
        println!(
            "Mint calculator guest executed: {} mints, {} updates",
            mint_journal.mints.len(),
            mint_journal.updates.len()
        );

        // Assemble a fake receipt and use it to call the mint function on the PovwMint contract.
        let mint_receipt: Receipt = FakeReceipt::new(ReceiptClaim::ok(
            BOUNDLESS_POVW_MINT_CALCULATOR_ID,
            mint_journal.abi_encode(),
        ))
        .try_into()?;

        let mint_tx = self
            .mint_contract
            .mint(mint_journal.abi_encode().into(), encode_seal(&mint_receipt)?.into())
            .send()
            .await?;

        println!("Mint transaction sent: {:?}", mint_tx.tx_hash());

        Ok(mint_tx.get_receipt().await?)
    }
}

// TODO(povw): This is copied from risc0_ethereum_contracts to work around version conflict
// issues. Remove this when we use a published version of risc0.
pub fn encode_seal(receipt: &risc0_zkvm::Receipt) -> anyhow::Result<Vec<u8>> {
    let seal = match receipt.inner.clone() {
        InnerReceipt::Fake(receipt) => {
            let seal = receipt.claim.digest().as_bytes().to_vec();
            let selector = &[0xFFu8; 4];
            // Create a new vector with the capacity to hold both selector and seal
            let mut selector_seal = Vec::with_capacity(selector.len() + seal.len());
            selector_seal.extend_from_slice(selector);
            selector_seal.extend_from_slice(&seal);
            selector_seal
        }
        _ => bail!("Unsupported receipt type"),
    };
    Ok(seal)
}

// Execute the log updater guest with the given input
pub fn execute_log_updater_guest(
    input: &log_updater::Input,
) -> anyhow::Result<log_updater::Journal> {
    let log_builder_receipt = FakeReceipt::new(ReceiptClaim::ok(
        RISC0_POVW_LOG_BUILDER_ID,
        borsh::to_vec(&input.update)?,
    ));
    let env = ExecutorEnv::builder()
        .write_frame(&borsh::to_vec(input)?)
        .add_assumption(log_builder_receipt)
        .build()?;
    let session_info = default_executor().execute(env, BOUNDLESS_POVW_LOG_UPDATER_ELF)?;
    assert_eq!(session_info.exit_code, ExitCode::Halted(0));

    let decoded_journal = log_updater::Journal::abi_decode(&session_info.journal.bytes)?;
    Ok(decoded_journal)
}

// Execute the mint calculator guest with the given input
pub fn execute_mint_calculator_guest(
    input: &mint_calculator::Input,
) -> anyhow::Result<mint_calculator::MintCalculatorJournal> {
    let env = ExecutorEnv::builder().write_frame(&postcard::to_allocvec(input)?).build()?;
    let session_info = default_executor().execute(env, BOUNDLESS_POVW_MINT_CALCULATOR_ELF)?;
    assert_eq!(session_info.exit_code, ExitCode::Halted(0));

    let decoded_journal =
        mint_calculator::MintCalculatorJournal::abi_decode(&session_info.journal.bytes)?;
    Ok(decoded_journal)
}
