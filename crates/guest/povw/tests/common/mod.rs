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
    signers::Signer,
    sol,
    sol_types::SolValue,
};
use alloy_primitives::U256;
use anyhow::{anyhow, bail};
use boundless_povw_guests::{
    log_updater::{self, IPovwAccounting, LogBuilderJournal, WorkLogUpdate},
    mint_calculator::{self, host::IPovwMint::IPovwMintInstance, WorkLogFilter},
    BOUNDLESS_POVW_LOG_UPDATER_ELF, BOUNDLESS_POVW_LOG_UPDATER_ID,
    BOUNDLESS_POVW_MINT_CALCULATOR_ELF, BOUNDLESS_POVW_MINT_CALCULATOR_ID,
};
use derive_builder::Builder;
use risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ID;
use risc0_steel::ethereum::{EthChainSpec, ANVIL_CHAIN_SPEC};
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
    MockZKC,
    "../../../out/MockZKC.sol/MockZKC.json"
);

sol!(
    #[sol(rpc)]
    MockZKCRewards,
    "../../../out/MockZKC.sol/MockZKCRewards.json"
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
    pub zkc_contract: MockZKC::MockZKCInstance<DynProvider>,
    pub zkc_rewards_contract: MockZKCRewards::MockZKCRewardsInstance<DynProvider>,
    pub povw_accounting_contract: PovwAccounting::PovwAccountingInstance<DynProvider>,
    pub povw_mint_contract: IPovwMintInstance<DynProvider>,
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

    // Deploy MockZKC
    let zkc_contract = MockZKC::deploy(provider.clone()).await?;
    println!("MockZKC deployed at: {:?}", zkc_contract.address());

    // Deploy MockZKCRewards
    let zkc_rewards_contract = MockZKCRewards::deploy(provider.clone()).await?;
    println!("MockZKCRewards deployed at: {:?}", zkc_rewards_contract.address());

    // Deploy PovwAccounting contract (needs verifier and log updater ID)
    let povw_contract = PovwAccounting::deploy(
        provider.clone(),
        *mock_verifier.address(),
        *zkc_contract.address(),
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
        *zkc_contract.address(),
        *zkc_rewards_contract.address(),
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
        zkc_contract,
        zkc_rewards_contract,
        povw_accounting_contract: povw_contract,
        povw_mint_contract: mint_interface,
    })
}

impl TestCtx {
    pub async fn advance_to_epoch(&self, epoch: U256) -> anyhow::Result<()> {
        let epoch_start_time = self.zkc_contract.getEpochStartTime(epoch).call().await?;

        let diff = self.provider.anvil_set_time(epoch_start_time.to::<u64>()).await?;
        self.provider.anvil_mine(Some(1), None).await?;
        println!("Anvil time advanced by {diff} seconds to advance to epoch {epoch}");

        let new_epoch = self.zkc_contract.getCurrentEpoch().call().await?;
        assert_eq!(new_epoch, epoch, "Expected epoch to be {epoch}; actually {new_epoch}");
        Ok(())
    }

    pub async fn advance_epochs(&self, epochs: U256) -> anyhow::Result<U256> {
        let initial_epoch = self.zkc_contract.getCurrentEpoch().call().await?;
        let new_epoch = initial_epoch + epochs;
        self.advance_to_epoch(new_epoch).await?;
        Ok(new_epoch)
    }

    pub async fn post_work_log_update(
        &self,
        signer: &impl Signer,
        update: &LogBuilderJournal,
        value_recipient: Address,
    ) -> anyhow::Result<IPovwAccounting::WorkLogUpdated> {
        let signature = WorkLogUpdate::from_log_builder_journal(update.clone(), value_recipient)
            .sign(signer, *self.povw_accounting_contract.address(), self.chain_id)
            .await?;

        // Use execute_log_updater_guest to get a Journal.
        let input = log_updater::Input {
            update: update.clone(),
            value_recipient,
            signature: signature.as_bytes().to_vec(),
            contract_address: *self.povw_accounting_contract.address(),
            chain_id: self.chain_id,
        };
        let journal = execute_log_updater_guest(&input)?;
        println!("Guest execution completed, journal: {journal:#?}");

        let fake_receipt: Receipt =
            FakeReceipt::new(ReceiptClaim::ok(BOUNDLESS_POVW_LOG_UPDATER_ID, journal.abi_encode()))
                .try_into()?;

        // Call the PovwAccounting.updateWorkLog function and confirm that it does not revert.
        let tx_result = self
            .povw_accounting_contract
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
        let finalize_tx = self.povw_accounting_contract.finalizeEpoch().send().await?;
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

    pub async fn build_mint_input(
        &self,
        opts: impl Into<MintOptions>,
    ) -> anyhow::Result<mint_calculator::Input> {
        let MintOptions { epochs, chain_spec, work_log_filter, exclude_blocks } = opts.into();

        // NOTE: This implementation includes all events for the specified epochs, and not just the
        // ones that related to the ids in the work_log_filter, to provide extra events and test
        // the filtering.

        // Query for WorkLogUpdated and EpochFinalized events, recording the block numbers that include these events.
        let latest_block = self.provider.get_block_number().await?;
        let epoch_filter_str =
            if epochs.is_empty() { "all epochs".to_string() } else { format!("epochs {epochs:?}") };
        println!("Running mint operation for blocks: 0 to {latest_block}, filtering for {epoch_filter_str}");

        // Query for WorkLogUpdated events
        let work_log_event_filter = self
            .povw_accounting_contract
            .WorkLogUpdated_filter()
            .from_block(0)
            .to_block(latest_block);
        let work_log_events = work_log_event_filter.query().await?;
        println!("Found {} total WorkLogUpdated events", work_log_events.len());

        // Query for EpochFinalized events
        let epoch_finalized_filter = self
            .povw_accounting_contract
            .EpochFinalized_filter()
            .from_block(0)
            .to_block(latest_block);
        let epoch_finalized_events = epoch_finalized_filter.query().await?;
        println!("Found {} total EpochFinalized events", epoch_finalized_events.len());

        // Filter events by epoch if specified
        let filtered_work_log_events: Vec<_> = if epochs.is_empty() {
            work_log_events
        } else {
            work_log_events
                .into_iter()
                .filter(|(event, _)| epochs.contains(&event.epochNumber))
                .collect()
        };

        let filtered_epoch_finalized_events: Vec<_> = if epochs.is_empty() {
            epoch_finalized_events
        } else {
            epoch_finalized_events
                .into_iter()
                .filter(|(event, _)| epochs.contains(&event.epoch))
                .collect()
        };

        println!(
            "After filtering: {} WorkLogUpdated events, {} EpochFinalized events",
            filtered_work_log_events.len(),
            filtered_epoch_finalized_events.len()
        );

        // Collect and sort unique block numbers that contain filtered events.
        let mut work_log_update_block_numbers = BTreeSet::new();
        for (event, log) in &filtered_work_log_events {
            if let Some(block_number) = log.block_number {
                work_log_update_block_numbers.insert(block_number);
                println!(
                    "WorkLogUpdated event at block {} (epoch {})",
                    block_number, event.epochNumber
                );
            }
        }
        let mut epoch_finalized_block_numbers = BTreeSet::new();
        for (event, log) in &filtered_epoch_finalized_events {
            if let Some(block_number) = log.block_number {
                epoch_finalized_block_numbers.insert(block_number);
                println!("EpochFinalized event at block {} (epoch {})", block_number, event.epoch);
            }
        }

        // Combine the sets of blocks that contain either event, and add the block used for the completeness_check.
        let completeness_check_block = epoch_finalized_block_numbers
            .last()
            .ok_or_else(|| anyhow!("no epoch finalized events processed"))?
            - 1;
        let mut block_numbers = &work_log_update_block_numbers | &epoch_finalized_block_numbers;
        block_numbers.insert(completeness_check_block);

        // Remove excluded blocks, but error if trying to exclude the completeness check block
        for excluded_block in &exclude_blocks {
            if *excluded_block == completeness_check_block {
                bail!("Cannot exclude completeness check block {completeness_check_block}");
            }
            block_numbers.remove(excluded_block);
        }

        // Build the input for the mint_calculator, including input for Steel.
        let mint_input = mint_calculator::Input::build(
            *self.povw_accounting_contract.address(),
            *self.zkc_contract.address(),
            *self.zkc_rewards_contract.address(),
            self.provider.clone(),
            chain_spec,
            block_numbers,
            work_log_filter,
        )
        .await?;

        println!("Mint calculator input built with {} blocks", mint_input.env.0.len());
        Ok(mint_input)
    }

    pub async fn run_mint(&self) -> anyhow::Result<TransactionReceipt> {
        self.run_mint_with_opts(MintOptions::default()).await
    }

    pub async fn run_mint_with_opts(
        &self,
        opts: impl Into<MintOptions>,
    ) -> anyhow::Result<TransactionReceipt> {
        let mint_input = self.build_mint_input(opts).await?;

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
            .povw_mint_contract
            .mint(mint_journal.abi_encode().into(), encode_seal(&mint_receipt)?.into())
            .send()
            .await?;

        println!("Mint transaction sent: {:?}", mint_tx.tx_hash());

        Ok(mint_tx.get_receipt().await?)
    }
}

#[derive(Clone, Debug, Builder)]
#[builder(build_fn(name = "build_inner", private))]
pub struct MintOptions {
    #[builder(setter(into), default)]
    epochs: Vec<U256>,
    #[builder(default = "&ANVIL_CHAIN_SPEC")]
    chain_spec: &'static EthChainSpec,
    #[builder(setter(into), default)]
    work_log_filter: WorkLogFilter,
    #[builder(setter(into), default)]
    exclude_blocks: BTreeSet<u64>,
}

impl Default for MintOptions {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl MintOptions {
    pub fn builder() -> MintOptionsBuilder {
        Default::default()
    }
}

impl MintOptionsBuilder {
    fn build(&self) -> MintOptions {
        // Auto-generated build-inner is infallible because all fields have defaults.
        self.build_inner().unwrap()
    }
}

impl From<&mut MintOptionsBuilder> for MintOptions {
    fn from(value: &mut MintOptionsBuilder) -> Self {
        value.build()
    }
}

impl From<MintOptionsBuilder> for MintOptions {
    fn from(value: MintOptionsBuilder) -> Self {
        value.build()
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
