#![no_main]
risc0_zkvm::guest::entry!(main);

use alloy_primitives::Address;
use alloy_sol_types::SolValue;
use fulfillment_counter::{op_chain_spec_from_id, FulfillmentCounterJournal, IBoundlessMarket};
use risc0_steel::{Event, EvmBlockHeader, MultiblockEvmInput};
use risc0_op_steel::optimism::OpEvmFactory;
use risc0_zkvm::guest::env;

fn main() {
    // Read inputs in the same order the host writes them.
    let prover_address: Address = env::read();
    let market_address: Address = env::read();
    let chain_id: u64 = env::read();
    let multiblock_input: MultiblockEvmInput<OpEvmFactory> = env::read();

    // Convert the input into a multiblock env for execution.
    let chain_spec = op_chain_spec_from_id(chain_id).expect("unrecognized chain id in input");
    let envs = multiblock_input.into_env(chain_spec);

    // Iterate over each block env and count matching RequestFulfilled events.
    let mut count: u64 = 0;
    let mut start_block: Option<u64> = None;
    let mut end_block: Option<u64> = None;

    for env in envs.iter() {
        let block_number = env.header().number();

        // Track block range.
        if start_block.is_none() || block_number < start_block.unwrap() {
            start_block = Some(block_number);
        }
        if end_block.is_none() || block_number > end_block.unwrap() {
            end_block = Some(block_number);
        }

        // Query all RequestFulfilled events at the market address for this block.
        let events = Event::new::<IBoundlessMarket::RequestFulfilled>(env)
            .address(market_address)
            .topic2(prover_address)
            .query();

        // Filter by the target prover address and count matches.
        for event in events {
            if event.prover == prover_address {
                count += 1;
            }
        }
    }

    let journal = FulfillmentCounterJournal {
        prover: prover_address,
        market: market_address,
        startBlock: start_block.unwrap_or(0),
        endBlock: end_block.unwrap_or(0),
        count,
        steelCommit: envs.into_commitment(),
    };
    env::commit_slice(&journal.abi_encode());
}
