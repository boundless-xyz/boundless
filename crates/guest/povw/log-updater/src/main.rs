use alloy_sol_types::SolValue;
use boundless_povw_guests::log_updater::{
    Input, Journal, WorkLogUpdate, RISC0_POVW_LOG_BUILDER_ID,
};
use risc0_zkvm::guest::env;

fn main() {
    let input: Input = borsh::from_slice(&env::read_frame()).unwrap();

    // Verify that the update was produced by the work log builder.
    // NOTE: The povw log builder supports self-recursion by accepting its own image ID as input.
    // This means the verifier must check the value `self_image_id` written to the journal.
    env::verify(RISC0_POVW_LOG_BUILDER_ID, &borsh::to_vec(&input.update).unwrap()).unwrap();
    assert_eq!(input.update.self_image_id, RISC0_POVW_LOG_BUILDER_ID.into());

    // Convert the input to the Solidity struct and verify the EIP-712 signature, using the work
    // log ID as the authenticating party.
    let update = WorkLogUpdate::from_log_builder_journal(input.update, input.value_recipient);
    update
        .verify_signature(
            update.workLogId,
            &input.signature,
            input.contract_address,
            input.chain_id,
        )
        .expect("failed to verify signature on work log update");

    // Write the journal, including the EIP-712 domain hash for the verifying contract.
    let journal = Journal {
        update,
        eip712Domain: WorkLogUpdate::eip712_domain(input.contract_address, input.chain_id)
            .hash_struct(),
    };
    env::commit_slice(&journal.abi_encode());
}
