// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Shared library for the Mint Calculator guest between guest and host.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{Add, AddAssign},
    sync::LazyLock,
};

use alloy_primitives::{Address, ChainId, U256};
use alloy_sol_types::sol;
use risc0_povw::PovwLogId;
use risc0_steel::{
    ethereum::{
        EthChainSpec, EthEvmEnv, EthEvmInput, ETH_MAINNET_CHAIN_SPEC, ETH_SEPOLIA_CHAIN_SPEC,
        STEEL_TEST_PRAGUE_CHAIN_SPEC,
    },
    Commitment, StateDb, SteelVerifier,
};
use serde::{Deserialize, Serialize};

#[cfg(feature = "build-guest")]
pub use crate::guest_artifacts::BOUNDLESS_POVW_MINT_CALCULATOR_PATH;
pub use crate::guest_artifacts::{
    BOUNDLESS_POVW_MINT_CALCULATOR_ELF, BOUNDLESS_POVW_MINT_CALCULATOR_ID,
};

// HACK: Defining a Steel::Commitment symbol here allowed resolution of the Steel.Commitment
// reference in IPovwMint.sol.
#[expect(non_snake_case)]
mod Steel {
    pub(super) use risc0_steel::Commitment;
}

#[cfg(feature = "host")]
sol!(
    #[sol(extra_derives(Debug, Serialize, Deserialize), rpc)]
    "./src/contracts/artifacts/IPovwMint.sol"
);
#[cfg(not(feature = "host"))]
sol!(
    #[sol(extra_derives(Debug, Serialize, Deserialize))]
    "./src/contracts/artifacts/IPovwMint.sol"
);

/// A mapping of well-known chain IDs to their [EthChainSpec].
pub static CHAIN_SPECS: LazyLock<BTreeMap<ChainId, EthChainSpec>> = LazyLock::new(|| {
    BTreeMap::from([
        (ETH_MAINNET_CHAIN_SPEC.chain_id, ETH_MAINNET_CHAIN_SPEC.clone()),
        (ETH_SEPOLIA_CHAIN_SPEC.chain_id, ETH_SEPOLIA_CHAIN_SPEC.clone()),
        (STEEL_TEST_PRAGUE_CHAIN_SPEC.chain_id, STEEL_TEST_PRAGUE_CHAIN_SPEC.clone()),
    ])
});

#[derive(Clone, Serialize, Deserialize)]
pub struct MultiblockEthEvmInput(pub Vec<EthEvmInput>);

impl MultiblockEthEvmInput {
    pub fn into_env(self, chain_spec: &EthChainSpec) -> MultiblockEthEvmEnv<StateDb, Commitment> {
        // Converts the input into `EvmEnv` structs for execution.
        let mut multiblock_env = MultiblockEthEvmEnv(Default::default());
        for env_input in self.0 {
            let env = env_input.into_env(chain_spec);
            if let Some(collision) = multiblock_env.0.insert(env.header().number, env) {
                // NOTE: This could instead be handled via extending the original, if that was
                // available in the guest. But keeping things constrained is reasonable.
                panic!("more than one env input provided for block {}", collision.header().number);
            };
        }
        // Verify that the envs form a subsequence of a since chain. This is a required check, and
        // so we do it here before returning the env for the user to make queries.
        multiblock_env.verify_continuity();
        multiblock_env
    }
}

/// An ordered map of block numbers to [EthEvmEnv] that form a subsequence in a single chain.
pub struct MultiblockEthEvmEnv<Db, Commit>(pub BTreeMap<u64, EthEvmEnv<Db, Commit>>);

impl MultiblockEthEvmEnv<StateDb, Commitment> {
    /// Ensure that the [EthEvmEnv] in this multiblock env form a subsequence of blocks from a
    /// single chain, all blocks being an ancestor of the latest block.
    fn verify_continuity(&mut self) {
        // NOTE: We don't check that the map is non-empty here.
        self.0.values().reduce(|env_prev, env| {
            SteelVerifier::new(env).verify(env_prev.commitment());
            env
        });
    }

    /// Return the commitment to the last block in the subsequence, which indirectly commitment to
    /// all blocks in this environment.
    pub fn commitment(&self) -> Option<&Commitment> {
        self.0.values().last().map(|env| env.commitment())
    }
}

/// A filter for [PovwLogId] used to select which work logs to include in the mint proof.
///
/// The default value of this filter sets it to include all log IDs. If the filter is constructed
/// with a list of values, then it will only include those values.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkLogFilter(Option<BTreeSet<PovwLogId>>);

impl WorkLogFilter {
    /// Construct a [WorkLogFilter] that includes all log IDs.
    pub const fn any() -> Self {
        Self(None)
    }

    /// Construct a [WorkLogFilter] that does not include any log IDs.
    pub const fn none() -> Self {
        Self(Some(BTreeSet::new()))
    }

    /// Check whether to filter indicates that the given log ID should be included.
    pub fn includes(&self, log_id: PovwLogId) -> bool {
        self.0.as_ref().map(|set| set.contains(&log_id)).unwrap_or(true)
    }
}

impl<T: AsRef<[PovwLogId]>> From<T> for WorkLogFilter {
    /// Construct a [WorkLogFilter] from the given slice of log IDs. Only the given log IDs will be
    /// included in the filter. If the slice is empty, no log IDs will be included.
    fn from(value: T) -> Self {
        Self::from_iter(value.as_ref().iter().cloned())
    }
}

impl FromIterator<PovwLogId> for WorkLogFilter {
    /// Construct a [WorkLogFilter] from the given iterator of log IDs. Only the given log IDs will
    /// be included in the filter. If the iterator is empty, no log IDs will be included.
    fn from_iter<T: IntoIterator<Item = PovwLogId>>(iter: T) -> Self {
        Self(Some(BTreeSet::from_iter(iter)))
    }
}

#[derive(Serialize, Deserialize)]
#[non_exhaustive]
pub struct Input {
    /// Address of the PoVW accounting contract to query.
    ///
    /// It is not possible to be assured that this is the correct contract when the guest is
    /// running, and so the behavior of the contract may deviate from expected. If the prover did
    /// supply the wrong address, the proof will be rejected by the minting contract when it checks
    /// the address written to the journal.
    pub povw_accounting_address: Address,
    /// Address of the IZKC contract to query.
    ///
    /// See note on `povw_accounting_address` above about safety.
    pub zkc_address: Address,
    /// Address of the IZKCRewards contract to query.
    ///
    /// See note on `povw_accounting_address` above about safety.
    pub zkc_rewards_address: Address,
    /// EIP-155 chain ID for the chain being queried.
    ///
    /// This chain ID is used to select the [ChainSpec][risc0_steel::config::ChainSpec] that will
    /// be used to construct the EVM.
    pub chain_id: ChainId,
    /// Input for constructing a [MultiblockEthEvmEnv] to query a sequence of blocks.
    pub env: MultiblockEthEvmInput,
    /// Filter for the work log IDs to be included in this mint calculation.
    ///
    /// If not specified, all work logs with updates in the given blocks will be included. If
    /// specified, only the given set of work log IDs will be included. This is useful in cases
    /// where the processing of an epoch must be broken up into multiple proofs, and multiple
    /// onchain transactions.
    pub work_log_filter: WorkLogFilter,
}

impl Input {
    /// Serialize the input to a vector of bytes.
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        postcard::to_allocvec(self).map_err(Into::into)
    }

    /// Deserialize the input from a slice of bytes.
    pub fn decode(buffer: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        postcard::from_bytes(buffer.as_ref()).map_err(Into::into)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FixedPoint(U256);

impl FixedPoint {
    pub const BITS: usize = 128;
    pub const BASE: U256 = U256::ONE.checked_shl(Self::BITS).unwrap();

    /// Construct a fixed-point representation of a fractional value.
    ///
    /// # Panics
    ///
    /// Panics if the given numerator is too close to U256::MAX, or if the represented fraction
    /// greater than one (e.g. numerator > denominator). Also panics if the denominator is zero.
    pub fn fraction(num: U256, dem: U256) -> Self {
        let fraction = num.checked_mul(Self::BASE).unwrap() / dem;
        assert!(fraction <= Self::BASE, "expected fractional value is greater than one");
        Self(fraction)
    }

    pub fn mul_unwrap(&self, x: U256) -> U256 {
        self.0.checked_mul(x).unwrap().wrapping_shr(Self::BITS)
    }
}

impl Add for FixedPoint {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.checked_add(rhs.0).unwrap())
    }
}

impl AddAssign for FixedPoint {
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.checked_add(rhs.0).unwrap()
    }
}

#[cfg(feature = "host")]
pub mod host {
    use std::{future::Future, marker::PhantomData};

    use alloy_contract::CallBuilder;
    use alloy_provider::Provider;
    use alloy_sol_types::SolValue;
    use anyhow::Context;
    use risc0_steel::{
        alloy::network::Ethereum,
        beacon::BeaconCommit,
        ethereum::{EthBlockHeader, EthEvmFactory},
        history::HistoryCommit,
        host::{
            db::{ProofDb, ProviderDb},
            Beacon, BlockNumberOrTag, EvmEnvBuilder, History, HostCommit,
        },
        BlockHeaderCommit, Contract, Event,
    };
    use risc0_zkvm::Receipt;

    use super::*;
    use crate::{
        log_updater::IPovwAccounting,
        mint_calculator::IPovwMint::IPovwMintInstance,
        zkc::{IZKCRewards, IZKC},
    };

    // Private type aliases, to make the definitions in this modules more concise.
    type EthEvmEnvBuilder<P, B> = EvmEnvBuilder<P, EthEvmFactory, &'static EthChainSpec, B>;
    type EthHostDb<P> = ProofDb<ProviderDb<Ethereum, P>>;

    impl<P, C> MultiblockEthEvmEnv<EthHostDb<P>, HostCommit<C>>
    where
        P: Provider + Clone + 'static,
        C: Clone + BlockHeaderCommit<EthBlockHeader>,
    {
        /// Preflight the verification that the blocks in the multiblock environment form a
        /// subsequence of a single chain.
        ///
        /// The verify call within the guest occurs atomically with
        /// [MutltiblockEthEvmInput::into_env]. If this method is not called by the host, the
        /// conversion of the input into an env will fail in the guest, as the required Merkle
        /// proofs will not be available.
        pub async fn preflight_verify_continuity(&mut self) -> anyhow::Result<()> {
            let mut env_iter = self.0.values_mut();
            let Some(mut env_prev) = env_iter.next() else {
                // If the env is empty, return early as it is a trivial subsequence.
                return Ok(());
            };
            for env in env_iter {
                SteelVerifier::preflight(env)
                    .verify(&env_prev.commitment())
                    .await
                    .with_context(|| format!("failed to preflight SteelVerifier verify of commit for block {} using env of block {}", env_prev.header().number, env.header().number))?;
                env_prev = env;
            }
            Ok(())
        }
    }

    pub trait IntoEthEvmInput {
        type Error;

        fn into_input(self) -> impl Future<Output = Result<EthEvmInput, Self::Error>>;
    }

    impl<P> IntoEthEvmInput for EthEvmEnv<EthHostDb<P>, HostCommit<()>>
    where
        P: Provider + Clone + 'static,
    {
        type Error = anyhow::Error;

        async fn into_input(self) -> Result<EthEvmInput, Self::Error> {
            self.into_input().await
        }
    }

    impl<P> IntoEthEvmInput for EthEvmEnv<EthHostDb<P>, HostCommit<BeaconCommit>>
    where
        P: Provider + Clone + 'static,
    {
        type Error = anyhow::Error;

        async fn into_input(self) -> Result<EthEvmInput, Self::Error> {
            self.into_input().await
        }
    }

    impl<P> IntoEthEvmInput for EthEvmEnv<EthHostDb<P>, HostCommit<HistoryCommit>>
    where
        P: Provider + Clone + 'static,
    {
        type Error = anyhow::Error;

        async fn into_input(self) -> Result<EthEvmInput, Self::Error> {
            self.into_input().await
        }
    }

    impl<P, C> MultiblockEthEvmEnv<EthHostDb<P>, HostCommit<C>>
    where
        P: Provider + Clone + 'static,
        EthEvmEnv<EthHostDb<P>, HostCommit<C>>: IntoEthEvmInput<Error = anyhow::Error>,
    {
        pub async fn into_input(self) -> anyhow::Result<MultiblockEthEvmInput> {
            let mut input = MultiblockEthEvmInput(Vec::with_capacity(self.0.len()));
            for (block_number, env) in self.0 {
                let block_input = env.into_input().await.with_context(|| {
                    format!("failed to convert env for block number {block_number} into input")
                })?;
                input.0.push(block_input);
            }
            Ok(input)
        }
    }

    // TODO: Based on how this is implemented right now, the caller must provide a chain of block
    // number that can be verified via chaining with SteelVerifier. This means, for example, if there
    // is a 3 days gap in the subsequence of blocks I am processing, I need to additionally provide 2-3
    // more blocks in the middle of that gap.
    pub struct MultiblockEthEvmEnvBuilder<P: Provider, B, C> {
        builder: EthEvmEnvBuilder<P, B>,
        env: MultiblockEthEvmEnv<EthHostDb<P>, HostCommit<C>>,
    }

    /// A trait used to capture the shared behavior of the [EvmEnvBuilder] instantiations with
    /// different commitment types.
    pub trait EnvBuilder {
        type Env;
        type Error;

        fn build(self) -> impl Future<Output = Result<Self::Env, Self::Error>>;
    }

    impl<P: Provider> EnvBuilder for EthEvmEnvBuilder<P, ()> {
        type Env = EthEvmEnv<EthHostDb<P>, HostCommit<()>>;
        type Error = anyhow::Error;

        async fn build(self) -> Result<Self::Env, Self::Error> {
            self.build().await
        }
    }

    impl<P: Provider> EnvBuilder for EthEvmEnvBuilder<P, Beacon> {
        type Env = EthEvmEnv<EthHostDb<P>, HostCommit<BeaconCommit>>;
        type Error = anyhow::Error;

        async fn build(self) -> Result<Self::Env, Self::Error> {
            self.build().await
        }
    }

    impl<P: Provider> EnvBuilder for EthEvmEnvBuilder<P, History> {
        type Env = EthEvmEnv<EthHostDb<P>, HostCommit<HistoryCommit>>;
        type Error = anyhow::Error;

        async fn build(self) -> Result<Self::Env, Self::Error> {
            self.build().await
        }
    }

    impl<P: Provider, B, C> MultiblockEthEvmEnvBuilder<P, B, C> {
        pub async fn insert(
            &mut self,
            block: impl Into<BlockNumberOrTag>,
        ) -> anyhow::Result<&mut EthEvmEnv<EthHostDb<P>, HostCommit<C>>>
        where
            P: Clone,
            B: Clone,
            EthEvmEnvBuilder<P, B>:
                EnvBuilder<Env = EthEvmEnv<EthHostDb<P>, HostCommit<C>>, Error = anyhow::Error>,
        {
            let env = self.builder.clone().block_number_or_tag(block.into()).build().await?;
            self.insert_env(env)
        }

        pub async fn get_or_insert(
            &mut self,
            block_number: u64,
        ) -> anyhow::Result<&mut EthEvmEnv<EthHostDb<P>, HostCommit<C>>>
        where
            P: Clone,
            B: Clone,
            EthEvmEnvBuilder<P, B>:
                EnvBuilder<Env = EthEvmEnv<EthHostDb<P>, HostCommit<C>>, Error = anyhow::Error>,
        {
            if self.env.0.contains_key(&block_number) {
                return Ok(self.env.0.get_mut(&block_number).unwrap());
            }
            self.insert(block_number).await
        }

        /// Insert the given [EthEvmEnv] into the [MultiblockEthEvmEnv].
        ///
        /// Returns a mutable reference to the [EthEvmEnv]. If there is already an env in the
        /// [MultiblockEthEvmEnv] with the same block number, this will be the merged env.
        pub fn insert_env(
            &mut self,
            mut env: EthEvmEnv<EthHostDb<P>, HostCommit<C>>,
        ) -> anyhow::Result<&mut EthEvmEnv<EthHostDb<P>, HostCommit<C>>> {
            let block_number = env.header().number;
            // If the name block is specified multiple times, merge the envs.
            if let Some(existing_env) = self.env.0.remove(&block_number) {
                env = existing_env
                    .merge(env)
                    .with_context(|| format!("conflicting blocks with number {block_number}"))?;
            };
            self.env.0.insert(block_number, env);
            Ok(self.env.0.get_mut(&block_number).unwrap())
        }

        /// Finalize the builder to obtain a [MultiblockEthEvmEnv].
        ///
        /// This method runs [MultiblockEthEvmEnv::preflight_verify_continuity] to provide the
        /// necessary witness data for the guest to verify chaining of the blocks in the env.
        pub async fn build(
            mut self,
        ) -> anyhow::Result<MultiblockEthEvmEnv<EthHostDb<P>, HostCommit<C>>>
        where
            P: Clone + 'static,
            C: Clone + BlockHeaderCommit<EthBlockHeader>,
        {
            self.env
                .preflight_verify_continuity()
                .await
                .context("Failed to preflight the multi-block continuity check")?;
            Ok(self.env)
        }
    }

    impl<P: Provider> From<EthEvmEnvBuilder<P, ()>> for MultiblockEthEvmEnvBuilder<P, (), ()> {
        fn from(builder: EthEvmEnvBuilder<P, ()>) -> Self {
            Self { builder, env: MultiblockEthEvmEnv(Default::default()) }
        }
    }

    impl<P: Provider> From<EthEvmEnvBuilder<P, Beacon>>
        for MultiblockEthEvmEnvBuilder<P, Beacon, BeaconCommit>
    {
        fn from(builder: EthEvmEnvBuilder<P, Beacon>) -> Self {
            Self { builder, env: MultiblockEthEvmEnv(Default::default()) }
        }
    }

    impl<P: Provider> From<EthEvmEnvBuilder<P, History>>
        for MultiblockEthEvmEnvBuilder<P, History, HistoryCommit>
    {
        fn from(builder: EthEvmEnvBuilder<P, History>) -> Self {
            Self { builder, env: MultiblockEthEvmEnv(Default::default()) }
        }
    }

    impl Input {
        pub async fn build<P, B, C>(
            povw_accounting_address: Address,
            zkc_address: Address,
            zkc_rewards_address: Address,
            chain_id: ChainId,
            env_builder: EthEvmEnvBuilder<P, B>,
            block_numbers: impl IntoIterator<Item = u64>,
            work_log_filter: impl Into<WorkLogFilter>,
        ) -> anyhow::Result<Self>
        where
            P: Provider + Clone + 'static,
            B: Clone,
            C: Clone + BlockHeaderCommit<EthBlockHeader>,
            EthEvmEnvBuilder<P, B>: EnvBuilder<Env = EthEvmEnv<EthHostDb<P>, HostCommit<C>>, Error = anyhow::Error>
                + Into<MultiblockEthEvmEnvBuilder<P, B, C>>,
            EthEvmEnv<EthHostDb<P>, HostCommit<C>>: IntoEthEvmInput<Error = anyhow::Error>,
        {
            // NOTE: The way this function is currently structured, there is some risk that is a
            // reorg were to occur while it is running, the build check at the end will fail, or
            // the guest will reject the input.

            let block_numbers = block_numbers.into_iter().collect::<BTreeSet<u64>>();
            let work_log_filter = work_log_filter.into();
            let mut envs = env_builder.into();

            let mut latest_epoch_finalization_block: Option<u64> = None;
            let mut epochs = BTreeSet::<U256>::new();
            for block_number in block_numbers.iter() {
                let env = envs.get_or_insert(*block_number).await?;
                let epoch_finalized_events =
                    Event::preflight::<IPovwAccounting::EpochFinalized>(env)
                        .address(povw_accounting_address)
                        .query()
                        .await
                        .context("failed to query EpochFinalized events")?;

                for epoch_finalized_event in epoch_finalized_events {
                    epochs.insert(epoch_finalized_event.epoch);
                    latest_epoch_finalization_block = Some(env.header().number);
                }
            }
            let latest_epoch_finalization_block = latest_epoch_finalization_block
                .context("No EpochFinalized events in the given blocks")?;

            // Mapping containing the epochs, and the work logs receiving value in those epochs.
            let mut epoch_work_logs = BTreeMap::<U256, BTreeSet<Address>>::new();
            let mut work_logs = BTreeSet::<Address>::new();
            for block_number in block_numbers.iter() {
                let env = envs.get_or_insert(*block_number).await?;
                let update_events = Event::preflight::<IPovwAccounting::WorkLogUpdated>(env)
                    .address(povw_accounting_address)
                    .query()
                    .await
                    .context("failed to query WorkLogUpdated events")?;

                for update_event in update_events {
                    // If the work log ID is filtered out or the value is zero, then this update
                    // can be skipped for deciding which calls to preflight.
                    if !work_log_filter.includes(update_event.data.workLogId.into()) {
                        continue;
                    }
                    if !epochs.contains(&update_event.epochNumber) {
                        continue;
                    }
                    work_logs.insert(update_event.data.workLogId);
                    if update_event.data.updateValue == U256::ZERO {
                        continue;
                    }
                    epoch_work_logs
                        .entry(update_event.epochNumber)
                        .or_default()
                        .insert(update_event.data.workLogId);
                }
            }

            // Preflight the contract calls the guest will make for completeness checks.
            let completeness_check_block_number = latest_epoch_finalization_block - 1;
            let completeness_check_env =
                envs.get_or_insert(completeness_check_block_number).await?;

            // In the guest, we query events across all environments, so if this one was not
            // previously included, we must preflight the query as well.
            if !block_numbers.contains(&completeness_check_block_number) {
                Event::preflight::<IPovwAccounting::EpochFinalized>(completeness_check_env)
                    .address(povw_accounting_address)
                    .query()
                    .await
                    .context("failed to query EpochFinalized events")?;
                Event::preflight::<IPovwAccounting::WorkLogUpdated>(completeness_check_env)
                    .address(povw_accounting_address)
                    .query()
                    .await
                    .context("failed to query WorkLogUpdated events")?;
            }

            let mut povw_accounting_contract =
                Contract::preflight(povw_accounting_address, completeness_check_env);
            for work_log_id in work_logs {
                povw_accounting_contract
                    .call_builder(&IPovwAccounting::workLogCommitCall { workLogId: work_log_id })
                    .call()
                    .await
                    .with_context(|| {
                        format!("Failed to preflight call: workLogCommit({work_log_id})")
                    })?;
            }

            // Preflight the contract calls the guest will make to calculate the reward values.
            let finalization_env = envs.get_or_insert(latest_epoch_finalization_block).await?;
            for (epoch, work_log_ids) in epoch_work_logs {
                let epoch_end_time = {
                    // NOTE: zkc_contract must be in a limited scope because it holds lastest_env.
                    let mut zkc_contract = Contract::preflight(zkc_address, finalization_env);
                    zkc_contract
                        .call_builder(&IZKC::getPoVWEmissionsForEpochCall { epoch })
                        .call()
                        .await
                        .with_context(|| {
                            format!("Failed to preflight call: getPoVWEmissionsForEpoch({epoch})")
                        })?;
                    zkc_contract
                        .call_builder(&IZKC::getEpochEndTimeCall { epoch })
                        .call()
                        .await
                        .with_context(|| {
                            format!("Failed to preflight call: getEpochEndTime({epoch})")
                        })?
                };

                for work_log_id in work_log_ids {
                    let mut zkc_rewards_contract =
                        Contract::preflight(zkc_rewards_address, finalization_env);
                    let call = IZKCRewards::getPastPoVWRewardCapCall {
                        account: work_log_id,
                        timepoint: epoch_end_time,
                    };
                    zkc_rewards_contract.call_builder(&call)
                        .call()
                        .await
                        .with_context(|| format!("Failed to preflight call: getPastPoVWRewardCap({work_log_id}, {epoch_end_time})"))?;
                }
            }

            let env_input = envs
                .build()
                .await?
                .into_input()
                .await
                .context("Failed to convert multi-block env to input")?;

            Ok(Self {
                povw_accounting_address,
                zkc_address,
                zkc_rewards_address,
                chain_id,
                env: env_input,
                work_log_filter,
            })
        }
    }

    impl<P: Provider> IPovwMintInstance<P> {
        /// Create a call to the [IPovwMint::mint] function to be sent in a tx.
        pub fn mint_with_receipt(
            &self,
            receipt: &Receipt,
        ) -> anyhow::Result<CallBuilder<&P, PhantomData<IPovwMint::mintCall>>> {
            let journal = MintCalculatorJournal::abi_decode(&receipt.journal.bytes)
                .context("Failed to decode journal from Mint Calculator receipt")?;
            let seal = risc0_ethereum_contracts::encode_seal(receipt)
                .context("Failed to encode seal for mint")?;

            Ok(self.mint(journal.abi_encode().into(), seal.into()))
        }
    }
}

#[cfg(feature = "prover")]
pub mod prover {
    use std::{borrow::Cow, convert::Infallible};

    use alloy_primitives::Address;
    use anyhow::Context;
    use derive_builder::Builder;
    use risc0_steel::ethereum::{EthChainSpec, EthEvmEnv};
    use risc0_zkvm::{
        compute_image_id, Digest, ExecutorEnv, ProveInfo, Prover, ProverOpts, VerifierContext,
    };
    use url::Url;

    use super::{
        Input, WorkLogFilter, BOUNDLESS_POVW_MINT_CALCULATOR_ELF, BOUNDLESS_POVW_MINT_CALCULATOR_ID,
    };

    /// A prover for mint calculations which runs the Mint Calculator to produce a receipt for
    /// determining token mint distributions based on PoVW accounting data.
    #[derive(Builder)]
    #[builder(pattern = "owned")]
    #[non_exhaustive]
    pub struct MintCalculatorProver<P, Q> {
        /// The underlying RISC Zero zkVM [Prover].
        #[builder(setter(custom))]
        pub prover: P,
        /// The Ethereum provider for blockchain queries.
        #[builder(setter(custom))]
        pub provider: Q,
        /// Beacon API URL, for building historical Ethereum data access proofs.
        #[builder(setter(into), default)]
        pub beacon_api: Option<Url>,
        /// Address of the PoVW accounting contract.
        #[builder(setter(into))]
        pub povw_accounting_address: Address,
        /// Address of the ZKC token contract.
        #[builder(setter(into))]
        pub zkc_address: Address,
        /// Address of the ZKC rewards contract.
        #[builder(setter(into))]
        pub zkc_rewards_address: Address,
        /// Ethereum chain specification for Steel environment.
        #[builder(setter(into))]
        pub chain_spec: &'static EthChainSpec,
        /// Image ID for the Mint Calculator program.
        ///
        /// Defaults to the Mint Calculator program ID that is built into this crate.
        #[builder(setter(custom), default = "BOUNDLESS_POVW_MINT_CALCULATOR_ID.into()")]
        pub mint_calculator_id: Digest,
        /// Executable for the Mint Calculator program.
        ///
        /// Defaults to the Mint Calculator program that is built into this crate.
        #[builder(setter(custom), default = "BOUNDLESS_POVW_MINT_CALCULATOR_ELF.into()")]
        pub mint_calculator_program: Cow<'static, [u8]>,
        /// [ProverOpts] to use when proving the mint calculation.
        #[builder(default)]
        pub prover_opts: ProverOpts,
        /// [VerifierContext] to use when proving the mint calculation. This only needs to be set when using
        /// non-standard verifier parameters.
        #[builder(default)]
        pub verifier_ctx: VerifierContext,
    }

    impl<P, Q> MintCalculatorProverBuilder<P, Q> {
        /// Set the underlying RISC Zero zkVM [Prover].
        pub fn prover<T>(self, prover: T) -> MintCalculatorProverBuilder<T, Q> {
            MintCalculatorProverBuilder {
                prover: Some(prover),
                provider: self.provider,
                beacon_api: self.beacon_api,
                povw_accounting_address: self.povw_accounting_address,
                zkc_address: self.zkc_address,
                zkc_rewards_address: self.zkc_rewards_address,
                chain_spec: self.chain_spec,
                mint_calculator_id: self.mint_calculator_id,
                mint_calculator_program: self.mint_calculator_program,
                prover_opts: self.prover_opts,
                verifier_ctx: self.verifier_ctx,
            }
        }

        /// Set the Ethereum provider for blockchain queries.
        pub fn provider<T>(self, provider: T) -> MintCalculatorProverBuilder<P, T> {
            MintCalculatorProverBuilder {
                prover: self.prover,
                provider: Some(provider),
                beacon_api: self.beacon_api,
                povw_accounting_address: self.povw_accounting_address,
                zkc_address: self.zkc_address,
                zkc_rewards_address: self.zkc_rewards_address,
                chain_spec: self.chain_spec,
                mint_calculator_id: self.mint_calculator_id,
                mint_calculator_program: self.mint_calculator_program,
                prover_opts: self.prover_opts,
                verifier_ctx: self.verifier_ctx,
            }
        }

        /// Set the Mint Calculator program, returning error if the image ID cannot be calculated.
        pub fn mint_calculator_program(
            self,
            program: impl Into<Cow<'static, [u8]>>,
        ) -> anyhow::Result<Self> {
            let program = program.into();
            let image_id = compute_image_id(&program)
                .context("Failed to compute image ID for Mint Calculator program")?;

            Ok(Self {
                mint_calculator_program: Some(program),
                mint_calculator_id: Some(image_id),
                ..self
            })
        }
    }

    impl<P, Q> MintCalculatorProver<P, Q>
    where
        P: Prover,
        Q: alloy_provider::Provider + Clone + 'static,
    {
        /// Build the Steel environment and Mint Calculator input.
        ///
        /// This method queries the provided RPC nodes.
        pub async fn build_input(
            &self,
            block_numbers: impl IntoIterator<Item = u64>,
            work_log_filter: impl Into<WorkLogFilter>,
        ) -> anyhow::Result<Input> {
            // NOTE: Branches repeat code because the types are distinct.
            if let Some(beacon_api) = self.beacon_api.clone() {
                // When a beacon API is provided, set up beacon commitments and enable History. Use
                // the parent of the latest block as the commit.
                // TODO: We would like to use the History commitment here, but it does not work
                // with the current implementation of MultiblockEthEvmEnv. In particular, it
                // results in the env.commitment() being "in the future" SteelVerifier rejects it.
                let env_builder = EthEvmEnv::builder()
                    .chain_spec(self.chain_spec)
                    .provider(self.provider.clone())
                    .beacon_api(beacon_api);

                // Patch in extra blocks in order to complete the chain of blocks as needed. This
                // is required because EIP 4788 has a buffer size of 8191. We use 6000 here as the
                // max gap. Add a recent block to make sure we will chain to a present value.
                let latest_block_number =
                    self.provider.get_block_number().await.context("Failed to get block number")?;
                let patched_block_numbers = PatchedIterator::<_, 6000>::new(
                    block_numbers.into_iter().chain([latest_block_number - 2]),
                );
                Input::build(
                    self.povw_accounting_address,
                    self.zkc_address,
                    self.zkc_rewards_address,
                    self.chain_spec.chain_id,
                    env_builder,
                    patched_block_numbers,
                    work_log_filter,
                )
                .await
                .context("Failed to build Mint Calculator input")
            } else {
                let env_builder = EthEvmEnv::builder()
                    .chain_spec(self.chain_spec)
                    .provider(self.provider.clone());

                Input::build(
                    self.povw_accounting_address,
                    self.zkc_address,
                    self.zkc_rewards_address,
                    self.chain_spec.chain_id,
                    env_builder,
                    block_numbers,
                    work_log_filter,
                )
                .await
                .context("Failed to build Mint Calculator input")
            }
        }

        /// Prove mint calculations using the given [Input].
        pub async fn prove_mint(&self, input: &Input) -> anyhow::Result<ProveInfo> {
            let env = ExecutorEnv::builder()
                .write_frame(&input.encode()?)
                .build()
                .context("failed to build ExecutorEnv")?;

            // Prove the mint calculation
            // NOTE: This may block the current thread for a significant amount of time. It is not
            // trivial to wrap this statement in e.g. tokio's spawn_blocking because self contains
            // a VerifierContext which does not implement Send. Using tokio block_in_place somewhat
            // mitigates the issue, but not fully.
            let prove_info = tokio::task::block_in_place(|| {
                self.prover
                    .prove_with_ctx(
                        env,
                        &self.verifier_ctx,
                        &self.mint_calculator_program,
                        &self.prover_opts,
                    )
                    .context("failed to prove mint calculation")
            })?;

            Ok(prove_info)
        }
    }

    impl MintCalculatorProver<Infallible, Infallible> {
        /// Create a new builder for [MintCalculatorProver].
        pub fn builder() -> MintCalculatorProverBuilder<Infallible, Infallible> {
            Default::default()
        }
    }

    // A utility type used to patch up a list of block numbers to have not too large of a gap.
    struct PatchedIterator<I: Iterator<Item = u64>, const MAX_GAP: u64> {
        iter: I,
        prev: Option<u64>,
        next: Option<u64>,
    }

    impl<I: Iterator<Item = u64>, const MAX_GAP: u64> PatchedIterator<I, MAX_GAP> {
        pub fn new(iter: I) -> Self {
            Self { iter, next: None, prev: None }
        }
    }

    impl<I: Iterator<Item = u64>, const MAX_GAP: u64> Iterator for PatchedIterator<I, MAX_GAP> {
        type Item = u64;

        fn next(&mut self) -> Option<Self::Item> {
            let mut next = self.next.take().or_else(|| self.iter.next())?;
            // If the gap between the last value and the next would be too large, buffer next.
            if self.prev.map(|prev| next > prev + MAX_GAP).unwrap_or(false) {
                self.next = Some(next);
                next = self.prev.unwrap() + MAX_GAP;
            }
            self.prev = Some(next);
            Some(next)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::PatchedIterator;

        fn patch(iter: impl IntoIterator<Item = u64>) -> Vec<u64> {
            PatchedIterator::<_, 10>::new(iter.into_iter()).collect()
        }

        #[test]
        fn patched_iter() {
            assert_eq!(patch([]), Vec::<u64>::new());
            assert_eq!(patch([1]), vec![1]);
            assert_eq!(patch([1, 5]), vec![1, 5]);
            assert_eq!(patch([1, 5, 20]), vec![1, 5, 15, 20]);
            assert_eq!(patch([1, 5, 20, 25]), vec![1, 5, 15, 20, 25]);
            assert_eq!(patch([1, 5, 20, 25, 50]), vec![1, 5, 15, 20, 25, 35, 45, 50]);
        }
    }
}

#[cfg(test)]
mod tests {
    use risc0_zkvm::compute_image_id;

    use super::{BOUNDLESS_POVW_MINT_CALCULATOR_ELF, BOUNDLESS_POVW_MINT_CALCULATOR_ID};

    #[test]
    fn image_id_consistency() {
        assert_eq!(
            BOUNDLESS_POVW_MINT_CALCULATOR_ID,
            <[u32; 8]>::from(compute_image_id(BOUNDLESS_POVW_MINT_CALCULATOR_ELF).unwrap())
        );
    }
}
