// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

//! Shared library for the Mint Calculator guest between guest and host.

use std::{
    collections::BTreeMap,
    ops::{Add, AddAssign},
};

use alloy_primitives::{Address, ChainId, U256};
use risc0_steel::{
    ethereum::{EthChainSpec, EthEvmEnv, EthEvmInput},
    Commitment, StateDb, SteelVerifier,
};
use serde::{Deserialize, Serialize};

alloy_sol_types::sol! {
    // Copied from contracts/src/povw/PovwMint.sol
    #[derive(Debug)]
    struct MintCalculatorUpdate {
        address workLogId;
        bytes32 initialCommit;
        bytes32 updatedCommit;
    }

    #[derive(Debug, Default)]
    struct FixedPoint {
        uint256 value;
    }

    #[derive(Debug)]
    struct MintCalculatorMint {
        address recipient;
        FixedPoint value;
    }

    #[derive(Debug)]
    struct MintCalculatorJournal {
        MintCalculatorMint[] mints;
        MintCalculatorUpdate[] updates;
        address povwContractAddress;
        Commitment steelCommit;
    }
}

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

#[derive(Serialize, Deserialize)]
pub struct Input {
    /// Address of the PoVW accounting contract to query.
    ///
    /// It is not possible to be assured that this is the correct contract when the guest is
    /// running, and so the behavior of the contract may deviate from expected. If the prover did
    /// supply the wrong address, the proof will be rejected by the minting contract when it checks
    /// the address written to the journal.
    pub povw_contract_address: Address,
    /// EIP-155 chain ID for the chain being queried.
    ///
    /// This chain ID is used to select the [ChainSpec][risc0_steel::config::ChainSpec] that will
    /// be used to construct the EVM.
    pub chain_id: ChainId,
    /// Input for constructing a [MultiblockEthEvmEnv] to query a sequence of blocks.
    pub env: MultiblockEthEvmInput,
}

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
        Self { value: fraction }
    }

    pub fn mul_unwrap(&self, x: U256) -> U256 {
        self.value.checked_mul(x).unwrap().wrapping_shr(Self::BITS)
    }
}

impl Add for FixedPoint {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self { value: self.value + rhs.value }
    }
}

impl AddAssign for FixedPoint {
    fn add_assign(&mut self, rhs: Self) {
        self.value += rhs.value
    }
}

#[cfg(feature = "host")]
pub mod host {
    use alloy_provider::Provider;
    use anyhow::Context;
    use risc0_steel::{
        alloy::network::Ethereum,
        beacon::BeaconCommit,
        ethereum::{EthBlockHeader, EthEvmFactory},
        host::{
            db::{ProofDb, ProviderDb},
            Beacon, BlockNumberOrTag, EvmEnvBuilder, HostCommit,
        },
        BlockHeaderCommit, Event,
    };

    use super::*;
    use crate::log_updater::IPovwAccounting;

    alloy_sol_types::sol! {
        #[sol(rpc)]
        interface IPovwMint {
            function mint(bytes calldata journalBytes, bytes calldata seal) external;
            function lastCommit(address) external view returns (bytes32);
            function EPOCH_REWARD() external view returns (uint256);
        }
    }

    impl<P, C> MultiblockEthEvmEnv<ProofDb<ProviderDb<Ethereum, P>>, HostCommit<C>>
    where
        P: Provider + Clone + 'static,
        C: Clone + BlockHeaderCommit<EthBlockHeader>,
    {
        // TODO(povw): Integrate this call into the construction of the input?
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
                    .with_context(|| format!("failed to preflight SteelVerifier verify of commit for block {} using env of block {}", env.header().number, env_prev.header().number))?;
                env_prev = env;
            }
            Ok(())
        }
    }

    impl<P> MultiblockEthEvmEnv<ProofDb<ProviderDb<Ethereum, P>>, HostCommit<()>>
    where
        P: Provider + Clone + 'static,
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

    impl<P> MultiblockEthEvmEnv<ProofDb<ProviderDb<Ethereum, P>>, HostCommit<BeaconCommit>>
    where
        P: Provider + Clone + 'static,
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

    // TODO(povw): Based on how this is implemented right now, the caller must provide a chain of block
    // number that can be verified via chaining with SteelVerifier. This means, for example, if there
    // is a 3 days gap in the subsequence of blocks I am processing, I need to additionally provide 2-3
    // more blocks in the middle of that gap. Additionally, using the history feature for the final
    // commit is not supported, so if the last block is e.g. 36 days ago an additional block needs to be
    // provided at the end that is within the EIP-4788 expiration time.
    pub struct MultiblockEthEvmEnvBuilder<P, B> {
        builder: EvmEnvBuilder<P, EthEvmFactory, &'static EthChainSpec, B>,
        block_refs: Vec<BlockNumberOrTag>,
    }

    impl<P, B> MultiblockEthEvmEnvBuilder<P, B> {
        pub fn block_numbers(
            self,
            numbers: impl IntoIterator<Item = impl Into<BlockNumberOrTag>>,
        ) -> Self {
            Self { block_refs: numbers.into_iter().map(Into::into).collect(), ..self }
        }
    }

    impl<P: Provider + Clone> MultiblockEthEvmEnvBuilder<P, ()> {
        pub async fn build(
            self,
        ) -> anyhow::Result<MultiblockEthEvmEnv<ProofDb<ProviderDb<Ethereum, P>>, HostCommit<()>>>
        {
            let mut multiblock_env = MultiblockEthEvmEnv(Default::default());
            for block_ref in self.block_refs {
                let mut env = self.builder.clone().block_number_or_tag(block_ref).build().await?;
                let block_number = env.header().number;
                // If the name block is specified multiple times, merge the envs.
                if let Some(existing_env) = multiblock_env.0.remove(&block_number) {
                    env = existing_env.merge(env).with_context(|| {
                        format!("conflicting blocks with number {block_number}")
                    })?;
                };
                multiblock_env.0.insert(block_number, env);
            }
            Ok(multiblock_env)
        }
    }

    // TODO(povw): Deduplicate these two blocks of code. They are duplicated right now due to type
    // system challenges.
    impl<P: Provider + Clone> MultiblockEthEvmEnvBuilder<P, Beacon> {
        pub async fn build(
            self,
        ) -> anyhow::Result<
            MultiblockEthEvmEnv<ProofDb<ProviderDb<Ethereum, P>>, HostCommit<BeaconCommit>>,
        > {
            let mut multiblock_env = MultiblockEthEvmEnv(Default::default());
            for block_ref in self.block_refs {
                let mut env = self.builder.clone().block_number_or_tag(block_ref).build().await?;
                let block_number = env.header().number;
                // If the name block is specified multiple times, merge the envs.
                if let Some(existing_env) = multiblock_env.0.remove(&block_number) {
                    env = existing_env.merge(env).with_context(|| {
                        format!("conflicting blocks with number {block_number}")
                    })?;
                };
                multiblock_env.0.insert(block_number, env).unwrap();
            }
            Ok(multiblock_env)
        }
    }

    type EthEvmEnvBuilder<P, B> = EvmEnvBuilder<P, EthEvmFactory, &'static EthChainSpec, B>;

    impl<P: Provider> From<EthEvmEnvBuilder<P, Beacon>> for MultiblockEthEvmEnvBuilder<P, Beacon> {
        fn from(builder: EthEvmEnvBuilder<P, Beacon>) -> Self {
            Self { builder, block_refs: Vec::new() }
        }
    }

    impl<P: Provider> From<EthEvmEnvBuilder<P, ()>> for MultiblockEthEvmEnvBuilder<P, ()> {
        fn from(builder: EthEvmEnvBuilder<P, ()>) -> Self {
            Self { builder, block_refs: Vec::new() }
        }
    }

    impl Input {
        // TODO(povw): Provide a way to do this with Beacon commits. Also, its not really ideal to
        // have to pass in each of the block numbers here.
        pub async fn build<P>(
            povw_contract_address: Address,
            provider: P,
            chain_spec: &'static EthChainSpec,
            block_refs: impl IntoIterator<Item = impl Into<BlockNumberOrTag>>,
        ) -> anyhow::Result<Self>
        where
            P: Provider + Clone + 'static,
        {
            let mut envs = MultiblockEthEvmEnvBuilder::from(
                EthEvmEnv::builder().chain_spec(chain_spec).provider(provider),
            )
            .block_numbers(block_refs)
            .build()
            .await?;
            envs.preflight_verify_continuity()
                .await
                .context("failed to preflight verify_continuity")?;

            for env in envs.0.values_mut() {
                Event::preflight::<IPovwAccounting::EpochFinalized>(env)
                    .address(povw_contract_address)
                    .query()
                    .await
                    .context("failed to query EpochFinalized events")?;
            }
            for env in envs.0.values_mut() {
                Event::preflight::<IPovwAccounting::WorkLogUpdated>(env)
                    .address(povw_contract_address)
                    .query()
                    .await
                    .context("failed to query WorkLogUpdated events")?;
            }
            Ok(Self {
                povw_contract_address,
                chain_id: chain_spec.chain_id,
                env: envs.into_input().await.context("failed to convert env to input")?,
            })
        }
    }
}
