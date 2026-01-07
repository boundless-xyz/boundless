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

use std::{collections::BTreeSet, sync::Arc};

use alloy::{
    network::EthereumWallet,
    node_bindings::{Anvil, AnvilInstance},
    primitives::{utils::Unit, Address, U256},
    providers::{ext::AnvilApi, DynProvider, Provider, ProviderBuilder},
    rpc::types::{TransactionReceipt, TransactionRequest},
    signers::{local::PrivateKeySigner, Signer},
    sol,
    sol_types::{SolCall, SolValue},
};
use anyhow::Context;
use boundless_market::contracts::bytecode::ERC1967Proxy;
use boundless_povw::{
    contracts::bytecode::{PovwAccounting, PovwMint},
    log_updater::{
        self,
        IPovwAccounting::{self, IPovwAccountingInstance},
        LogBuilderJournal, BOUNDLESS_POVW_LOG_UPDATER_ELF, BOUNDLESS_POVW_LOG_UPDATER_ID,
    },
    mint_calculator::{
        self, IPovwMint::IPovwMintInstance, WorkLogFilter, BOUNDLESS_POVW_MINT_CALCULATOR_ELF,
        BOUNDLESS_POVW_MINT_CALCULATOR_ID,
    },
};
use derive_builder::Builder;
use risc0_ethereum_contracts::encode_seal;
use risc0_povw::{guest::RISC0_POVW_LOG_BUILDER_ID, PovwJobId};
use risc0_steel::ethereum::{EthChainSpec, EthEvmEnv, STEEL_TEST_PRAGUE_CHAIN_SPEC};
use risc0_zkvm::{
    default_executor, Digest, ExecutorEnv, ExitCode, FakeReceipt, MaybePruned, Receipt,
    ReceiptClaim, Work, WorkClaim,
};
use tokio::sync::Mutex;

use crate::verifier::deploy_mock_verifier;

// Import the Solidity contracts using alloy's sol! macro
// Use the compiled contracts output to allow for deploying the contracts.
// NOTE: This requires running `forge build` before running this test.
// TODO(povw): Work on making this more robust. If the requirement to run forge build before this
// test is removed, then make sure to remove that step from CI.
sol!(
    #[allow(clippy::too_many_arguments)]
    #[sol(rpc)]
    MockZKC,
    "../../out/MockZKC.sol/MockZKC.json"
);

sol!(
    #[sol(rpc)]
    MockZKCRewards,
    "../../out/MockZKC.sol/MockZKCRewards.json"
);

#[derive(Clone)]
pub struct TestCtx {
    pub anvil: Arc<Mutex<AnvilInstance>>,
    pub chain_id: u64,
    pub provider: DynProvider,
    pub zkc: MockZKC::MockZKCInstance<DynProvider>,
    pub zkc_rewards: MockZKCRewards::MockZKCRewardsInstance<DynProvider>,
    pub povw_accounting: IPovwAccounting::IPovwAccountingInstance<DynProvider>,
    pub povw_mint: IPovwMintInstance<DynProvider>,
    pub owner: PrivateKeySigner,
}

/// Creates a new [TestCtx] with all the setup needed to test PoVW.
///
/// NOTE: The Avil chain created here uses a Steel test chain ID and the Prague hardfork.
pub async fn test_ctx() -> anyhow::Result<TestCtx> {
    let anvil = Anvil::new().chain_id(STEEL_TEST_PRAGUE_CHAIN_SPEC.chain_id).prague().spawn();
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

    // Setup the owner wallet. We use a new wallet to ensure that we, by default, are testing from
    // an address that does not have special rights.
    let owner = PrivateKeySigner::random();
    let tx_fund_owner = TransactionRequest::default()
        .from(signer.address())
        .to(owner.address())
        .value(Unit::ETHER.wei());
    provider.send_transaction(tx_fund_owner).await?.watch().await?;

    // Deploy PovwAccounting and PovwMint contracts to the Anvil instance, using a MockRiscZeroVerifier and a
    // basic ERC-20.

    // Setup verifiers (will use mock in dev mode, real Groth16 otherwise)
    let mock_verifier = deploy_mock_verifier(provider.clone()).await?;
    println!("Mock verifier deployed at: {:?}", mock_verifier);

    // Deploy MockZKC
    let zkc_contract = MockZKC::deploy(provider.clone()).await?;
    println!("MockZKC deployed at: {:?}", zkc_contract.address());

    // Deploy MockZKCRewards
    let zkc_rewards_contract = MockZKCRewards::deploy(provider.clone()).await?;
    println!("MockZKCRewards deployed at: {:?}", zkc_rewards_contract.address());

    // Deploy PovwAccounting contract (needs verifier, zkc, and log updater ID)
    let povw_accounting_contract = PovwAccounting::deploy(
        provider.clone(),
        mock_verifier,
        *zkc_contract.address(),
        bytemuck::cast::<_, [u8; 32]>(BOUNDLESS_POVW_LOG_UPDATER_ID).into(),
    )
    .await?;
    println!("PovwAccounting contract deployed at: {:?}", povw_accounting_contract.address());
    let povw_accounting_proxy = ERC1967Proxy::deploy(
        provider.clone(),
        *povw_accounting_contract.address(),
        PovwAccounting::initializeCall { initialOwner: owner.address() }.abi_encode().into(),
    )
    .await
    .context("Failed to deploy PovwAccounting proxy")?;
    println!("PovwAccounting proxy deployed at: {:?}", povw_accounting_proxy.address());
    let povw_accounting_interface =
        IPovwAccountingInstance::new(*povw_accounting_proxy.address(), provider.clone());

    // Deploy PovwMint contract (needs verifier, povw accounting, mint calculator ID, zkc, zkc rewards)
    let povw_mint_contract = PovwMint::deploy(
        provider.clone(),
        mock_verifier,
        *povw_accounting_interface.address(),
        bytemuck::cast::<_, [u8; 32]>(BOUNDLESS_POVW_MINT_CALCULATOR_ID).into(),
        *zkc_contract.address(),
        *zkc_rewards_contract.address(),
    )
    .await?;
    println!("PovwMint contract deployed at: {:?}", povw_mint_contract.address());
    let povw_mint_proxy = ERC1967Proxy::deploy(
        provider.clone(),
        *povw_mint_contract.address(),
        PovwMint::initializeCall { initialOwner: owner.address() }.abi_encode().into(),
    )
    .await
    .context("Failed to deploy PovwMint proxy")?;
    println!("PovwMint proxy deployed at: {:?}", povw_mint_proxy.address());
    let povw_mint_interface = IPovwMintInstance::new(*povw_mint_proxy.address(), provider.clone());

    let chain_id = anvil.lock().await.chain_id();
    Ok(TestCtx {
        anvil,
        chain_id,
        provider,
        zkc: zkc_contract,
        zkc_rewards: zkc_rewards_contract,
        povw_accounting: povw_accounting_interface,
        povw_mint: povw_mint_interface,
        owner,
    })
}

impl TestCtx {
    pub async fn advance_to_epoch(&self, epoch: U256) -> anyhow::Result<()> {
        let epoch_start_time = self.zkc.getEpochStartTime(epoch).call().await?;

        let diff = self.provider.anvil_set_time(epoch_start_time.to::<u64>()).await?;
        self.provider.anvil_mine(Some(1), None).await?;
        println!("Anvil time advanced by {diff} seconds to advance to epoch {epoch}");

        let new_epoch = self.zkc.getCurrentEpoch().call().await?;
        assert_eq!(new_epoch, epoch, "Expected epoch to be {epoch}; actually {new_epoch}");
        Ok(())
    }

    pub async fn advance_epochs(&self, epochs: U256) -> anyhow::Result<U256> {
        let initial_epoch = self.zkc.getCurrentEpoch().call().await?;
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
        // Use execute_log_updater_guest to get a Journal.
        let input = log_updater::Input::builder()
            .update(update.clone())
            .value_recipient(value_recipient)
            .contract_address(*self.povw_accounting.address())
            .chain_id(self.chain_id)
            .sign_and_build(signer)
            .await?;
        let journal = execute_log_updater_guest(&input)?;
        println!("Guest execution completed, journal: {journal:#?}");

        let fake_receipt: Receipt =
            FakeReceipt::new(ReceiptClaim::ok(BOUNDLESS_POVW_LOG_UPDATER_ID, journal.abi_encode()))
                .try_into()?;

        // Call the PovwAccounting.updateWorkLog function and confirm that it does not revert.
        let tx_result = self
            .povw_accounting
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
        let finalize_tx = self.povw_accounting.finalizeEpoch().send().await?;
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
        let work_log_event_filter =
            self.povw_accounting.WorkLogUpdated_filter().from_block(0).to_block(latest_block);
        let work_log_events = work_log_event_filter.query().await?;
        println!("Found {} total WorkLogUpdated events", work_log_events.len());

        // Query for EpochFinalized events
        let epoch_finalized_filter =
            self.povw_accounting.EpochFinalized_filter().from_block(0).to_block(latest_block);
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

        // Combine the block numbers for the WorkLogUpdated and EpochFinalized events, minus the
        // ones that are in the exclude_blocks set.
        let block_numbers =
            &(&work_log_update_block_numbers | &epoch_finalized_block_numbers) - &exclude_blocks;

        // Build the input for the mint_calculator, including input for Steel.
        let env_builder =
            EthEvmEnv::builder().chain_spec(chain_spec).provider(self.provider.clone());
        let mint_input = mint_calculator::Input::build(
            *self.povw_accounting.address(),
            *self.zkc.address(),
            *self.zkc_rewards.address(),
            chain_spec.chain_id,
            env_builder,
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
            .povw_mint
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
    #[builder(default = "&STEEL_TEST_PRAGUE_CHAIN_SPEC")]
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

// Execute the log updater guest with the given input
pub fn execute_log_updater_guest(
    input: &log_updater::Input,
) -> anyhow::Result<log_updater::Journal> {
    let log_builder_receipt =
        FakeReceipt::new(ReceiptClaim::ok(RISC0_POVW_LOG_BUILDER_ID, input.update.encode()?));
    let env = ExecutorEnv::builder()
        .write_frame(&input.encode()?)
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
    let env = ExecutorEnv::builder().write_frame(&input.encode()?).build()?;
    let session_info = default_executor().execute(env, BOUNDLESS_POVW_MINT_CALCULATOR_ELF)?;
    assert_eq!(session_info.exit_code, ExitCode::Halted(0));

    let decoded_journal =
        mint_calculator::MintCalculatorJournal::abi_decode(&session_info.journal.bytes)?;
    Ok(decoded_journal)
}

pub fn make_work_claim(
    job_id: impl Into<PovwJobId>,
    num_segments: u32,
    work_value: u64,
) -> anyhow::Result<WorkClaim<ReceiptClaim>> {
    let job_id = job_id.into();
    let segment_num_max = num_segments
        .checked_sub(1)
        .ok_or_else(|| anyhow::anyhow!("num_segments must be greater than 0"))?;

    Ok(WorkClaim {
        // Use a random claim digest, which stands in for some unknown claim.
        claim: MaybePruned::Pruned(Digest::new(rand::random())),
        work: Work {
            nonce_min: job_id.nonce(0),
            nonce_max: job_id.nonce(segment_num_max),
            value: work_value,
        }
        .into(),
    })
}

/// Mock Bento API server for testing work receipt endpoints
pub mod bento_mock {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use risc0_zkvm::{GenericReceipt, ReceiptClaim, WorkClaim};
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;
    use wiremock::{
        matchers::{method, path, path_regex},
        Mock, MockServer, Request, ResponseTemplate,
    };

    // TODO: Create a common crate that Bento, test-utils and the CLI can all use.
    /// Work receipt info matching Bento API format
    /// Copied from bento/crates/api/src/lib.rs
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WorkReceiptInfo {
        pub key: String,
        /// PoVW log ID if PoVW is enabled, None otherwise
        pub povw_log_id: Option<String>,
        /// PoVW job number if PoVW is enabled, None otherwise
        pub povw_job_number: Option<String>,
    }

    /// Work receipt list matching Bento API format
    /// Copied from bento/crates/api/src/lib.rs
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WorkReceiptList {
        pub receipts: Vec<WorkReceiptInfo>,
    }

    #[derive(Debug, Clone)]
    struct WorkReceiptEntry {
        receipt_bytes: Vec<u8>,
        info: WorkReceiptInfo,
    }

    /// Mock Bento server that provides work receipt endpoints compatible with the real Bento API
    pub struct BentoMockServer {
        server: MockServer,
        /// Storage for work receipts: receipt_id -> (bincode serialized receipt bytes, metadata)
        receipts: Arc<Mutex<HashMap<String, WorkReceiptEntry>>>,
    }

    impl BentoMockServer {
        /// Create a new mock Bento server
        pub async fn new() -> Self {
            let server = MockServer::start().await;
            let receipts = Arc::new(Mutex::new(HashMap::new()));

            let mock_server = Self { server, receipts };

            mock_server.setup_mocks().await;
            mock_server
        }

        /// Get the base URL of the mock server
        pub fn base_url(&self) -> String {
            self.server.uri()
        }

        /// Add a work receipt to the mock server
        /// Returns the receipt ID that can be used to retrieve it
        /// Extracts job number from the work receipt's work claim
        pub fn add_work_receipt(
            &self,
            receipt: &GenericReceipt<WorkClaim<ReceiptClaim>>,
        ) -> anyhow::Result<String> {
            let receipt_id = Uuid::new_v4().to_string();
            let receipt_bytes = bincode::serialize(receipt)?;

            // Extract job number from the work receipt. Format them to Strings as Bento does.
            let work = receipt.claim().clone().value().and_then(|x| x.work.value()).ok();
            let povw_job_number = work.as_ref().map(|x| format!("{}", x.nonce_min.job));
            let povw_log_id = work.as_ref().map(|x| format!("{:#x}", x.nonce_min.log));

            let receipt_info =
                WorkReceiptInfo { key: receipt_id.clone(), povw_log_id, povw_job_number };

            self.receipts.lock().unwrap().insert(
                receipt_id.clone(),
                WorkReceiptEntry { receipt_bytes, info: receipt_info.clone() },
            );

            tracing::debug!(
                "Added work receipt with ID: {} (job: {:?})",
                receipt_id,
                receipt_info.povw_job_number
            );
            Ok(receipt_id)
        }

        /// Setup the wiremock endpoints
        async fn setup_mocks(&self) {
            self.setup_work_receipts_list_endpoint().await;
            self.setup_work_receipt_get_endpoint().await;
        }

        /// Setup the GET /work-receipts endpoint
        async fn setup_work_receipts_list_endpoint(&self) {
            let receipts = self.receipts.clone();

            Mock::given(method("GET"))
                .and(path("/work-receipts"))
                .respond_with(move |_req: &Request| {
                    let receipts_guard = receipts.lock().unwrap();
                    let receipt_infos: Vec<WorkReceiptInfo> =
                        receipts_guard.values().map(|entry| entry.info.clone()).collect();
                    let response_body = WorkReceiptList { receipts: receipt_infos };

                    ResponseTemplate::new(200)
                        .insert_header("content-type", "application/json")
                        .set_body_json(response_body)
                })
                .mount(&self.server)
                .await;
        }

        /// Setup the GET /work-receipts/:receipt_id endpoint
        async fn setup_work_receipt_get_endpoint(&self) {
            let receipts = self.receipts.clone();

            Mock::given(method("GET"))
                .and(path_regex(r"^/work-receipts/[a-f0-9-]+$"))
                .respond_with(move |req: &Request| {
                    let path = req.url.path();
                    let receipt_id = path.strip_prefix("/work-receipts/").unwrap();

                    let receipts_guard = receipts.lock().unwrap();
                    if let Some(entry) = receipts_guard.get(receipt_id) {
                        ResponseTemplate::new(200)
                            .insert_header("content-type", "application/octet-stream")
                            .set_body_bytes(entry.receipt_bytes.clone())
                    } else {
                        ResponseTemplate::new(404)
                    }
                })
                .mount(&self.server)
                .await;
        }

        /// Get the number of receipts stored
        pub fn receipt_count(&self) -> usize {
            self.receipts.lock().unwrap().len()
        }

        /// Clear all stored receipts
        pub fn clear_receipts(&self) {
            self.receipts.lock().unwrap().clear();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::povw::make_work_claim;
        use risc0_povw::PovwLogId;
        use risc0_zkvm::{sha::Digestible, FakeReceipt, GenericReceipt};

        #[tokio::test]
        async fn test_bento_mock_server_basic() -> anyhow::Result<()> {
            let server = BentoMockServer::new().await;

            // Create a test work receipt
            let log_id = PovwLogId::random();
            let job_id = rand::random();
            let work_claim = make_work_claim((log_id, job_id), 10, 1000)?;
            let work_receipt = FakeReceipt::new(work_claim).into();

            // Add the receipt to the mock server
            let receipt_id = server.add_work_receipt(&work_receipt)?;

            // Test listing work receipts
            let list_url = format!("{}/work-receipts", server.base_url());
            let response = reqwest::get(&list_url).await?;
            assert_eq!(response.status(), 200);

            let receipt_list: WorkReceiptList = response.json().await?;
            assert_eq!(receipt_list.receipts.len(), 1);
            assert_eq!(receipt_list.receipts[0].key, receipt_id);
            assert_eq!(receipt_list.receipts[0].povw_log_id, Some(format!("{:#x}", log_id)));
            assert_eq!(receipt_list.receipts[0].povw_job_number, Some(job_id.to_string()));

            // Test fetching individual receipt
            let get_url = format!("{}/work-receipts/{}", server.base_url(), receipt_id);
            let response = reqwest::get(&get_url).await?;
            assert_eq!(response.status(), 200);

            let receipt_bytes = response.bytes().await?;
            let retrieved_receipt: GenericReceipt<WorkClaim<ReceiptClaim>> =
                bincode::deserialize(&receipt_bytes)?;

            assert_eq!(retrieved_receipt.claim().digest(), work_receipt.claim().digest());
            Ok(())
        }

        #[tokio::test]
        async fn test_multiple_receipts() -> anyhow::Result<()> {
            let server = BentoMockServer::new().await;

            // Add multiple receipts
            let mut receipt_ids = Vec::new();
            for i in 0..3 {
                let log_id = PovwLogId::random();
                let work_claim = make_work_claim((log_id, rand::random()), 5, 500 + i * 100)?;
                let work_receipt: GenericReceipt<WorkClaim<ReceiptClaim>> =
                    FakeReceipt::new(work_claim).into();

                let receipt_id = server.add_work_receipt(&work_receipt)?;
                receipt_ids.push(receipt_id);
            }

            // Test that all receipts are listed
            let list_url = format!("{}/work-receipts", server.base_url());
            let response = reqwest::get(&list_url).await?;
            let receipt_list: WorkReceiptList = response.json().await?;

            assert_eq!(receipt_list.receipts.len(), 3);
            assert_eq!(server.receipt_count(), 3);

            // Test that each receipt can be retrieved individually
            for receipt_id in &receipt_ids {
                let get_url = format!("{}/work-receipts/{}", server.base_url(), receipt_id);
                let response = reqwest::get(&get_url).await?;
                assert_eq!(response.status(), 200);

                // Should be able to deserialize as work receipt
                let receipt_bytes = response.bytes().await?;
                let _retrieved_receipt: GenericReceipt<WorkClaim<ReceiptClaim>> =
                    bincode::deserialize(&receipt_bytes)?;
            }

            Ok(())
        }

        #[tokio::test]
        async fn test_404_for_unknown_receipt() -> anyhow::Result<()> {
            let server = BentoMockServer::new().await;

            // Try to fetch a receipt that doesn't exist
            let unknown_id = Uuid::new_v4().to_string();
            let get_url = format!("{}/work-receipts/{}", server.base_url(), unknown_id);
            let response = reqwest::get(&get_url).await?;

            // Should return 404 for unknown receipt ID
            assert_eq!(response.status(), 404);

            Ok(())
        }
    }
}
