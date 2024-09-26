// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

// Constants for field tree heights
pub const BEACON_BLOCK_HEADER_FIELD_TREE_HEIGHT: usize = 3;
pub const BEACON_BLOCK_BODY_FIELD_TREE_HEIGHT: usize = 4;
pub const BEACON_STATE_FIELD_TREE_HEIGHT: usize = 5;
pub const VALIDATOR_FIELD_TREE_HEIGHT: usize = 3;
pub const EXECUTION_PAYLOAD_HEADER_FIELD_TREE_HEIGHT_DENEB: usize = 5;
pub const EXECUTION_PAYLOAD_HEADER_FIELD_TREE_HEIGHT_CAPELLA: usize = 4;
pub const BLOCK_ROOTS_TREE_HEIGHT: usize = 13;
pub const HISTORICAL_SUMMARIES_TREE_HEIGHT: usize = 24;
pub const BLOCK_SUMMARY_ROOT_INDEX: usize = 0;
pub const WITHDRAWAL_FIELD_TREE_HEIGHT: usize = 2;
pub const VALIDATOR_TREE_HEIGHT: usize = 40;
pub const WITHDRAWALS_TREE_HEIGHT: usize = 4;

// Indices
pub const EXECUTION_PAYLOAD_INDEX: usize = 9;
pub const SLOT_INDEX: usize = 0;
pub const STATE_ROOT_INDEX: usize = 3;
pub const BODY_ROOT_INDEX: usize = 4;
pub const VALIDATOR_TREE_ROOT_INDEX: usize = 11;
pub const HISTORICAL_SUMMARIES_INDEX: usize = 27;
pub const VALIDATOR_PUBKEY_INDEX: usize = 0;
pub const VALIDATOR_WITHDRAWAL_CREDENTIALS_INDEX: usize = 1;
pub const VALIDATOR_BALANCE_INDEX: usize = 2;
pub const VALIDATOR_WITHDRAWABLE_EPOCH_INDEX: usize = 7;
pub const TIMESTAMP_INDEX: usize = 9;
pub const WITHDRAWALS_INDEX: usize = 14;
pub const WITHDRAWAL_VALIDATOR_INDEX_INDEX: usize = 1;
pub const WITHDRAWAL_VALIDATOR_AMOUNT_INDEX: usize = 3;

// Misc Constants
pub const SLOTS_PER_EPOCH: u64 = 32;
pub const SECONDS_PER_SLOT: u64 = 12;
pub const SECONDS_PER_EPOCH: u64 = SLOTS_PER_EPOCH * SECONDS_PER_SLOT;
pub const UINT64_MASK: [u8; 8] = [0xff; 8];
