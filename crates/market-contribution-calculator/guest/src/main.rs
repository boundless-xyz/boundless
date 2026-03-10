#![no_main]
risc0_zkvm::guest::entry!(main);

use alloy_primitives::{Address, U256};
use alloy_sol_types::SolValue;
use market_contribution_calculator::{
    op_chain_spec_from_id, IBoundlessMarket, MarketContribution, MarketContributionJournal,
};
use risc0_op_steel::optimism::OpEvmFactory;
use risc0_steel::{Contract, EvmBlockHeader, MultiblockEvmInput};
use risc0_zkvm::guest::env;

fn main() {
    // Read inputs in the same order the host writes them.
    let market_address: Address = env::read();
    let epoch_start: U256 = env::read();
    let epoch_end: U256 = env::read();
    let start_block: u64 = env::read();
    let end_block: u64 = env::read();
    let chain_id: u64 = env::read();
    let multiblock_input: MultiblockEvmInput<OpEvmFactory> = env::read();

    // Convert the input into a multiblock env for execution.
    let chain_spec = op_chain_spec_from_id(chain_id).expect("unrecognized chain id in input");
    let envs = multiblock_input.into_env(chain_spec);

    // Find the environments for the start and end blocks.
    let start_env = envs.iter().find(|e| e.header().number() == start_block).unwrap_or_else(|| {
        panic!("start block {start_block} not found in multiblock env");
    });
    let end_env = envs.iter().find(|e| e.header().number() == end_block).unwrap_or_else(|| {
        panic!("end block {end_block} not found in multiblock env");
    });

    // Read the full list of contributing provers from the end block.
    // The array is append-only, so the end block has a superset of all provers.
    let end_contract = Contract::new(market_address, end_env);
    let prover_count: U256 = end_contract
        .call_builder(&IBoundlessMarket::contributingProversLengthCall {})
        .call();

    let count: u64 = prover_count.to::<u64>();
    let mut contributions = Vec::new();

    let start_contract = Contract::new(market_address, start_env);

    for i in 0..count {
        let index = U256::from(i);

        // Read prover address from the array at end block.
        let prover: Address = end_contract
            .call_builder(&IBoundlessMarket::contributingProversCall { index })
            .call();

        // Read cumulative contribution at both blocks.
        let end_value: U256 = end_contract
            .call_builder(
                &IBoundlessMarket::proverCumulativeContributionCall { prover },
            )
            .call();

        let start_value: U256 = start_contract
            .call_builder(
                &IBoundlessMarket::proverCumulativeContributionCall { prover },
            )
            .call();

        let contribution = end_value.saturating_sub(start_value);
        if contribution > U256::ZERO {
            contributions.push(MarketContribution { prover, contribution });
        }
    }

    let journal = MarketContributionJournal {
        contributions,
        marketAddress: market_address,
        epochStart: epoch_start,
        epochEnd: epoch_end,
        steelCommit: envs.into_commitment(),
    };
    env::commit_slice(&journal.abi_encode());
}
