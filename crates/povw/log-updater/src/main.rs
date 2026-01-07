// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use alloy_primitives::Address;
use alloy_sol_types::SolValue;
use boundless_povw::log_updater::{Input, Journal, WorkLogUpdate, RISC0_POVW_LOG_BUILDER_ID};
use risc0_zkvm::guest::env;

fn main() {
    let input = Input::decode(env::read_frame()).unwrap();

    // Verify that the update was produced by the work log builder.
    // NOTE: The povw log builder supports self-recursion by accepting its own image ID as input.
    // This means the verifier must check the value `self_image_id` written to the journal.
    env::verify(RISC0_POVW_LOG_BUILDER_ID, &input.update.encode().unwrap()).unwrap();
    assert_eq!(input.update.self_image_id, RISC0_POVW_LOG_BUILDER_ID.into());

    // NOTE: This check is included due to the fact that the ZKC contract does not allow sending
    // tokens to the zero address. Specifying a value recipient of zero would mean that rewards for
    // this update could not be distributed. Additionally, this is included to prevent accidental
    // burning of value.
    assert_ne!(input.value_recipient, Address::ZERO, "value recipient cannot be the zero address");

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
