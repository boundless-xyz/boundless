// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy_primitives::{Bytes, B256};
use alloy_sol_types::{sol, SolValue};
use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    constants::*,
    merkle::{merkleize_sha256, process_inclusion_proof_sha256, verify_inclusion_sha256},
};

// ABI encodable Output data.
sol! {
    struct WithdrawalJournal {
        uint40 validatorIndex;
        uint64 withdrawalAmountGwei;
        uint64 withdrawalTimestamp;
        bytes32 beaconStateRoot;
        bool fullWithdrawal;
        bytes32 validatorPubkeyHash;
    }
}

sol! {
    struct WithdrawalJournals {
        WithdrawalJournal[] journals;
    }
}

impl From<WithdrawalProof> for WithdrawalJournal {
    fn from(proof: WithdrawalProof) -> Self {
        WithdrawalJournal {
            beaconStateRoot: proof.beacon_state_root,
            withdrawalTimestamp: proof.get_withdrawal_timestamp(),
            fullWithdrawal: proof.is_full_withdrawal(),
            withdrawalAmountGwei: proof.get_withdrawal_amount_gwei(),
            validatorIndex: proof.validator_index,
            validatorPubkeyHash: proof.get_pubkey_hash(),
        }
    }
}

impl WithdrawalJournal {
    pub fn digest(&self) -> Digest {
        let bytes = self.abi_encode();
        Digest::try_from(Sha256::digest(bytes).as_slice()).unwrap()
    }
}

impl WithdrawalJournals {
    pub fn digest(&self) -> Digest {
        let bytes = self.abi_encode();
        Digest::try_from(Sha256::digest(bytes).as_slice()).unwrap()
    }
}

/// WithdrawalProof contains the merkle proofs and leaves needed to verify a partial/full withdrawal
#[derive(Debug, Serialize, Deserialize)]
pub struct WithdrawalProof {
    pub withdrawal_proof: Bytes,
    pub validator_index: u64,
    pub slot_proof: Bytes,
    pub execution_payload_proof: Bytes,
    pub timestamp_proof: Bytes,
    pub historical_summary_block_root_proof: Bytes,
    pub block_root_index: u64,
    pub historical_summary_index: u64,
    pub withdrawal_index: u64,
    pub block_root: B256,
    pub block_header_root_index: u64,
    pub latest_block_header_root: B256,
    pub beacon_state_root: B256,
    pub slot_root: B256,
    pub timestamp_root: B256,
    pub execution_payload_root: B256,
    pub state_root_proof: Bytes,
    pub withdrawal_fields: Vec<B256>,
    pub validator_fields: Vec<B256>,
    pub validator_proof: Bytes,
}

// Intermediate struct for deserialization
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawalProofJson {
    pub slot: u64,
    pub validator_index: u64,
    pub historical_summary_index: u64,
    pub withdrawal_index: u64,
    pub block_header_root_index: u64,
    pub beacon_state_root: B256,
    pub slot_root: B256,
    pub timestamp_root: B256,
    pub block_header_root: B256,
    pub latest_block_header_root: B256,
    pub execution_payload_root: B256,
    #[serde(rename = "WithdrawalProof")]
    pub withdrawal_proof: Vec<B256>,
    #[serde(rename = "SlotProof")]
    pub slot_proof: Vec<B256>,
    #[serde(rename = "ExecutionPayloadProof")]
    pub execution_payload_proof: Vec<B256>,
    #[serde(rename = "TimestampProof")]
    pub timestamp_proof: Vec<B256>,
    #[serde(rename = "HistoricalSummaryProof")]
    pub historical_summary_block_root_proof: Vec<B256>,
    #[serde(rename = "StateRootAgainstLatestBlockHeaderProof")]
    pub state_root_proof: Vec<B256>,
    #[serde(rename = "WithdrawalFields")]
    pub withdrawal_fields: Vec<B256>,
    #[serde(rename = "ValidatorFields")]
    pub validator_fields: Vec<B256>,
    #[serde(rename = "ValidatorProof")]
    pub validator_proof: Vec<B256>,
}

impl From<WithdrawalProofJson> for WithdrawalProof {
    fn from(val: WithdrawalProofJson) -> Self {
        WithdrawalProof {
            withdrawal_proof: Bytes::from(
                val.withdrawal_proof
                    .iter()
                    .flat_map(|b| b.as_slice().to_vec())
                    .collect::<Vec<u8>>(),
            ),
            slot_proof: Bytes::from(
                val.slot_proof.iter().flat_map(|b| b.as_slice().to_vec()).collect::<Vec<u8>>(),
            ),
            execution_payload_proof: Bytes::from(
                val.execution_payload_proof
                    .iter()
                    .flat_map(|b| b.as_slice().to_vec())
                    .collect::<Vec<u8>>(),
            ),
            timestamp_proof: Bytes::from(
                val.timestamp_proof.iter().flat_map(|b| b.as_slice().to_vec()).collect::<Vec<u8>>(),
            ),
            historical_summary_block_root_proof: Bytes::from(
                val.historical_summary_block_root_proof
                    .iter()
                    .flat_map(|b| b.as_slice().to_vec())
                    .collect::<Vec<u8>>(),
            ),
            block_root_index: val.block_header_root_index,
            historical_summary_index: val.historical_summary_index,
            withdrawal_index: val.withdrawal_index,
            block_root: val.block_header_root,
            slot_root: val.slot_root,
            timestamp_root: val.timestamp_root,
            execution_payload_root: val.execution_payload_root,
            validator_index: val.validator_index,
            block_header_root_index: val.block_header_root_index,
            latest_block_header_root: val.latest_block_header_root,
            beacon_state_root: val.beacon_state_root,
            state_root_proof: Bytes::from(
                val.state_root_proof
                    .iter()
                    .flat_map(|b| b.as_slice().to_vec())
                    .collect::<Vec<u8>>(),
            ),
            withdrawal_fields: val.withdrawal_fields,
            validator_fields: val.validator_fields,
            validator_proof: Bytes::from(
                val.validator_proof.iter().flat_map(|b| b.as_slice().to_vec()).collect::<Vec<u8>>(),
            ),
        }
    }
}

impl From<WithdrawalJournal> for WithdrawalProof {
    fn from(journal: WithdrawalJournal) -> Self {
        // Validator fields
        let mut validator_fields =
            vec![B256::default(); 2_usize.pow(VALIDATOR_FIELD_TREE_HEIGHT as u32)];
        validator_fields[VALIDATOR_PUBKEY_INDEX] = journal.validatorPubkeyHash;
        validator_fields[VALIDATOR_WITHDRAWAL_CREDENTIALS_INDEX] = journal.validatorPubkeyHash;
        validator_fields[VALIDATOR_BALANCE_INDEX] =
            B256::from_slice(&journal.withdrawalAmountGwei.to_le_bytes().abi_encode());
        validator_fields[VALIDATOR_WITHDRAWABLE_EPOCH_INDEX] =
            B256::from_slice(&(journal.withdrawalTimestamp).to_le_bytes().abi_encode());

        // Withdrawal fields
        let mut withdrawal_fields =
            vec![B256::default(); 2_usize.pow(WITHDRAWAL_FIELD_TREE_HEIGHT as u32)];
        withdrawal_fields[WITHDRAWAL_VALIDATOR_INDEX_INDEX] =
            B256::from_slice(&journal.validatorIndex.to_le_bytes().abi_encode());
        withdrawal_fields[WITHDRAWAL_VALIDATOR_AMOUNT_INDEX] =
            B256::from_slice(&journal.withdrawalAmountGwei.to_le_bytes().abi_encode());

        // // Generate the roots for validator and withdrawal fields
        let validator_root = merkleize_sha256(&validator_fields);
        let withdrawal_root = merkleize_sha256(&withdrawal_fields);

        let leaves = vec![withdrawal_root; 32];
        let (pre_withdrawal_proof, root) = generate_inclusion_proof_and_root(&leaves, 0);
        let mut leaves = vec![root; 16];
        let timestamp_root = B256::from_slice(&journal.withdrawalTimestamp.abi_encode());
        leaves[TIMESTAMP_INDEX] = timestamp_root;
        let (post_withdrawal_proof, execution_payload_root) =
            generate_inclusion_proof_and_root(&leaves, 15);
        let mut withdrawal_proof = pre_withdrawal_proof.clone();
        withdrawal_proof.extend_from_slice(&post_withdrawal_proof);
        let (timestamp_proof, _) = generate_inclusion_proof_and_root(&leaves, TIMESTAMP_INDEX);
        let leaves = vec![execution_payload_root; 16];
        let (pre_execution_payload_proof, root) = generate_inclusion_proof_and_root(&leaves, 0);
        let mut leaves = vec![root; 8];
        let slot_root = match journal.fullWithdrawal {
            true => B256::from_slice(&[255; 32]),
            false => B256::from_slice(&[0; 32]),
        };
        leaves[SLOT_INDEX] = slot_root;
        let (slot_proof, block_root) = generate_inclusion_proof_and_root(&leaves, SLOT_INDEX);
        let (post_execution_payload_proof, _root) = generate_inclusion_proof_and_root(&leaves, 7);
        let mut execution_payload_proof = pre_execution_payload_proof.clone();
        execution_payload_proof.extend_from_slice(&post_execution_payload_proof);
        let historical_block_header_index = ((HISTORICAL_SUMMARIES_INDEX as u64)
            << ((HISTORICAL_SUMMARIES_TREE_HEIGHT + 1) + 1 + BLOCK_ROOTS_TREE_HEIGHT))
            | (0 << (1 + BLOCK_ROOTS_TREE_HEIGHT))
            | ((BLOCK_SUMMARY_ROOT_INDEX as u64) << BLOCK_ROOTS_TREE_HEIGHT);
        let (pre_historical_summary_proof, _) =
            generate_proof_and_root(block_root, historical_block_header_index, 44);
        let len = pre_historical_summary_proof.len() - 1;
        let pre_root_1 = process_inclusion_proof_sha256(
            pre_historical_summary_proof.clone(),
            block_root,
            historical_block_header_index,
            len,
        );
        let index = (VALIDATOR_TREE_ROOT_INDEX as u64) << (VALIDATOR_TREE_HEIGHT + 1)
            | journal.validatorIndex;
        let (pre_validator_proof, _) = generate_proof_and_root(validator_root, index, 46);

        let len = pre_validator_proof.len() - 1;
        let pre_root_2 =
            process_inclusion_proof_sha256(pre_validator_proof.clone(), validator_root, index, len);
        let root_data = [pre_root_2.as_slice(), pre_root_1.as_slice()].concat();
        let mut hasher = Sha256::new();
        hasher.update(root_data);
        let beacon_state_root = B256::try_from(hasher.finalize().as_slice()).unwrap();

        let mut validator_proof = pre_validator_proof[..32 * 45].to_vec();
        validator_proof.extend_from_slice(pre_root_1.as_slice());
        let mut historical_summary_block_root_proof =
            pre_historical_summary_proof[..32 * 43].to_vec();
        historical_summary_block_root_proof.extend_from_slice(pre_root_2.as_slice());

        WithdrawalProof {
            validator_index: journal.validatorIndex,
            historical_summary_index: 0,
            withdrawal_index: 0,
            block_header_root_index: 0,
            beacon_state_root,
            slot_root,
            timestamp_root,
            block_root_index: 0,
            block_root,
            latest_block_header_root: B256::default(),
            execution_payload_root,
            withdrawal_proof: withdrawal_proof.into(),
            slot_proof: slot_proof.into(),
            execution_payload_proof: execution_payload_proof.into(),
            timestamp_proof: timestamp_proof.into(),
            historical_summary_block_root_proof: historical_summary_block_root_proof.into(),
            state_root_proof: Bytes::default(),
            withdrawal_fields,
            validator_fields,
            validator_proof: validator_proof.into(),
        }
    }
}

// Function to generate sibling hash
fn generate_sibling_hash(leaf: B256) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(leaf.as_slice());
    hasher.update(B256::default().as_slice());
    B256::try_from(hasher.finalize().as_slice()).unwrap()
}

// Function to generate proof and root
fn generate_proof_and_root(leaf: B256, index: u64, height: usize) -> (Vec<u8>, B256) {
    let mut proof = vec![];
    let mut index = index;

    let mut computed_hash = leaf;
    for _ in 0..height {
        let sibling = generate_sibling_hash(computed_hash);
        proof.extend_from_slice(sibling.as_slice());
        if index % 2 == 0 {
            computed_hash = B256::from_slice(
                Sha256::digest(&[computed_hash.as_slice(), sibling.as_slice()].concat()).as_slice(),
            );
        } else {
            computed_hash = B256::from_slice(
                Sha256::digest(&[sibling.as_slice(), computed_hash.as_slice()].concat()).as_slice(),
            );
        }
        index /= 2;
    }

    (proof, computed_hash)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StateRootProof {
    beacon_state_root: B256,
    proof: Bytes,
}

fn generate_inclusion_proof_and_root(leaves: &[B256], leaf_index: usize) -> (Vec<u8>, B256) {
    assert!(!leaves.is_empty(), "Leaves array should not be empty");
    assert!(leaves.len() % 2 == 0, "Number of leaves must be even");
    assert!(leaf_index < leaves.len(), "Leaf index out of bounds");

    let mut proof = vec![];
    let mut index = leaf_index;
    let mut layer = leaves.to_vec();

    while layer.len() > 1 {
        let mut next_layer = vec![];
        for i in (0..layer.len()).step_by(2) {
            let left = &layer[i];
            let right = &layer[i + 1];

            if i == index || i + 1 == index {
                proof.push(if i == index { *right } else { *left });
            }

            let mut hasher = Sha256::new();
            hasher.update(left.as_slice());
            hasher.update(right.as_slice());
            next_layer.push(B256::from_slice(hasher.finalize().as_slice()));
        }

        layer = next_layer;
        index /= 2;
    }

    let root = layer[0];
    let proof_bytes = proof.into_iter().flat_map(|b| b.as_slice().to_vec()).collect();

    (proof_bytes, root)
}

impl WithdrawalProof {
    pub fn verify_validator_fields(&self) -> Result<(), &'static str> {
        if self.validator_fields.len() != 2_usize.pow(VALIDATOR_FIELD_TREE_HEIGHT as u32) {
            return Err(
                "BeaconChainProofs.verifyValidatorFields: Validator fields has incorrect length",
            );
        }

        if self.validator_proof.len()
            != 32 * (VALIDATOR_TREE_HEIGHT + 1 + BEACON_STATE_FIELD_TREE_HEIGHT)
        {
            return Err("BeaconChainProofs.verifyValidatorFields: Proof has incorrect length");
        }

        let index = (VALIDATOR_TREE_ROOT_INDEX as u64) << (VALIDATOR_TREE_HEIGHT + 1)
            | self.validator_index;
        let validator_root = merkleize_sha256(&self.validator_fields);

        if !verify_inclusion_sha256(
            self.validator_proof.clone().into(),
            self.beacon_state_root,
            validator_root,
            index,
        ) {
            return Err("BeaconChainProofs.verifyValidatorFields: Invalid merkle proof");
        }

        Ok(())
    }

    pub fn verify_withdrawal(
        &self,
        // deneb_fork_timestamp: u64,
    ) -> Result<(), &'static str> {
        if self.withdrawal_fields.len() != 2_usize.pow(WITHDRAWAL_FIELD_TREE_HEIGHT as u32) {
            return Err(
                "BeaconChainProofs.verifyWithdrawal: withdrawalFields has incorrect length",
            );
        }

        if self.block_root_index >= 2_u64.pow(BLOCK_ROOTS_TREE_HEIGHT as u32) {
            return Err("BeaconChainProofs.verifyWithdrawal: blockRootIndex is too large");
        }
        if self.withdrawal_index >= 2_u64.pow(WITHDRAWALS_TREE_HEIGHT as u32) {
            return Err("BeaconChainProofs.verifyWithdrawal: withdrawalIndex is too large");
        }

        if self.historical_summary_index >= 2_u64.pow(HISTORICAL_SUMMARIES_TREE_HEIGHT as u32) {
            return Err("BeaconChainProofs.verifyWithdrawal: historicalSummaryIndex is too large");
        }

        let execution_payload_header_field_tree_height =
            EXECUTION_PAYLOAD_HEADER_FIELD_TREE_HEIGHT_CAPELLA;
        // if self.get_withdrawal_timestamp() < deneb_fork_timestamp {
        //     EXECUTION_PAYLOAD_HEADER_FIELD_TREE_HEIGHT_CAPELLA
        // } else {
        //     EXECUTION_PAYLOAD_HEADER_FIELD_TREE_HEIGHT_DENEB
        // };

        if self.withdrawal_proof.len()
            != 32 * (execution_payload_header_field_tree_height + WITHDRAWALS_TREE_HEIGHT + 1)
        {
            return Err("BeaconChainProofs.verifyWithdrawal: withdrawalProof has incorrect length");
        }
        if self.execution_payload_proof.len()
            != 32 * (BEACON_BLOCK_HEADER_FIELD_TREE_HEIGHT + BEACON_BLOCK_BODY_FIELD_TREE_HEIGHT)
        {
            return Err(
                "BeaconChainProofs.verifyWithdrawal: executionPayloadProof has incorrect length",
            );
        }
        if self.slot_proof.len() != 32 * BEACON_BLOCK_HEADER_FIELD_TREE_HEIGHT {
            return Err("BeaconChainProofs.verifyWithdrawal: slotProof has incorrect length");
        }
        if self.timestamp_proof.len() != 32 * execution_payload_header_field_tree_height {
            return Err("BeaconChainProofs.verifyWithdrawal: timestampProof has incorrect length");
        }

        if self.historical_summary_block_root_proof.len()
            != 32
                * (BEACON_STATE_FIELD_TREE_HEIGHT
                    + (HISTORICAL_SUMMARIES_TREE_HEIGHT + 1)
                    + 1
                    + BLOCK_ROOTS_TREE_HEIGHT)
        {
            return Err("BeaconChainProofs.verifyWithdrawal: historicalSummaryBlockRootProof has incorrect length");
        }

        let historical_block_header_index = ((HISTORICAL_SUMMARIES_INDEX as u64)
            << ((HISTORICAL_SUMMARIES_TREE_HEIGHT + 1) + 1 + BLOCK_ROOTS_TREE_HEIGHT))
            | ((self.historical_summary_index) << (1 + BLOCK_ROOTS_TREE_HEIGHT))
            | ((BLOCK_SUMMARY_ROOT_INDEX as u64) << BLOCK_ROOTS_TREE_HEIGHT)
            | (self.block_root_index);

        if !verify_inclusion_sha256(
            self.historical_summary_block_root_proof.clone().into(),
            self.beacon_state_root,
            self.block_root,
            historical_block_header_index,
        ) {
            return Err(
                "BeaconChainProofs.verifyWithdrawal: Invalid historical summary merkle proof",
            );
        }

        if !verify_inclusion_sha256(
            self.slot_proof.clone().into(),
            self.block_root,
            self.slot_root,
            SLOT_INDEX.try_into().unwrap(),
        ) {
            return Err("BeaconChainProofs.verifyWithdrawal: Invalid slot merkle proof");
        }

        let execution_payload_index =
            (BODY_ROOT_INDEX << BEACON_BLOCK_BODY_FIELD_TREE_HEIGHT) | EXECUTION_PAYLOAD_INDEX;

        if !verify_inclusion_sha256(
            self.execution_payload_proof.clone().into(),
            self.block_root,
            self.execution_payload_root,
            execution_payload_index.try_into().unwrap(),
        ) {
            return Err(
                "BeaconChainProofs.verifyWithdrawal: Invalid executionPayload merkle proof",
            );
        }

        if !verify_inclusion_sha256(
            self.timestamp_proof.clone().into(),
            self.execution_payload_root,
            self.timestamp_root,
            TIMESTAMP_INDEX.try_into().unwrap(),
        ) {
            return Err("BeaconChainProofs.verifyWithdrawal: Invalid timestamp merkle proof");
        }

        let withdrawal_index =
            (WITHDRAWALS_INDEX << (WITHDRAWALS_TREE_HEIGHT + 1)) | self.withdrawal_index as usize;
        let withdrawal_root = merkleize_sha256(&self.withdrawal_fields);

        if !verify_inclusion_sha256(
            self.withdrawal_proof.clone().into(),
            self.execution_payload_root,
            withdrawal_root,
            withdrawal_index.try_into().unwrap(),
        ) {
            return Err("BeaconChainProofs.verifyWithdrawal: Invalid withdrawal merkle proof");
        }

        Ok(())
    }

    pub fn get_withdrawal_timestamp(&self) -> u64 {
        u64::from_le_bytes(self.timestamp_root.as_slice()[..8].try_into().unwrap())
    }

    pub fn get_withdrawal_epoch(&self) -> u64 {
        u64::from_le_bytes(self.slot_root.as_slice()[..8].try_into().unwrap()) / SLOTS_PER_EPOCH
    }

    pub fn is_full_withdrawal(&self) -> bool {
        self.get_withdrawal_epoch() >= self.get_withdrawable_epoch()
    }

    pub fn get_pubkey_hash(&self) -> B256 {
        self.validator_fields[VALIDATOR_PUBKEY_INDEX]
    }

    pub fn get_withdrawal_credentials(&self) -> B256 {
        self.validator_fields[VALIDATOR_WITHDRAWAL_CREDENTIALS_INDEX]
    }

    pub fn get_effective_balance_gwei(&self) -> u64 {
        u64::from_le_bytes(
            self.validator_fields[VALIDATOR_BALANCE_INDEX].as_slice()[..8].try_into().unwrap(),
        )
    }

    pub fn get_withdrawable_epoch(&self) -> u64 {
        u64::from_le_bytes(
            self.validator_fields[VALIDATOR_WITHDRAWABLE_EPOCH_INDEX].as_slice()[..8]
                .try_into()
                .unwrap(),
        )
    }

    pub fn get_validator_index(&self) -> u64 {
        u64::from_le_bytes(
            self.withdrawal_fields[WITHDRAWAL_VALIDATOR_INDEX_INDEX].as_slice()[..8]
                .try_into()
                .unwrap(),
        )
    }

    pub fn get_withdrawal_amount_gwei(&self) -> u64 {
        u64::from_le_bytes(
            self.withdrawal_fields[WITHDRAWAL_VALIDATOR_AMOUNT_INDEX].as_slice()[..8]
                .try_into()
                .unwrap(),
        )
    }
}

pub fn verify_state_root_against_latest_block_root(
    latest_block_root: B256,
    beacon_state_root: B256,
    state_root_proof: Bytes,
) -> Result<(), &'static str> {
    if state_root_proof.len() != 32 * BEACON_BLOCK_HEADER_FIELD_TREE_HEIGHT {
        return Err(
            "BeaconChainProofs.verifyStateRootAgainstLatestBlockRoot: Proof has incorrect length",
        );
    }

    if !verify_inclusion_sha256(
        state_root_proof.into(),
        latest_block_root,
        beacon_state_root,
        STATE_ROOT_INDEX.try_into().unwrap(),
    ) {
        return Err("BeaconChainProofs.verifyStateRootAgainstLatestBlockRoot: Invalid latest block header root merkle proof");
    }

    Ok(())
}

pub fn hash_validator_bls_pubkey(validator_pubkey: &[u8]) -> Result<B256, &'static str> {
    if validator_pubkey.len() != 48 {
        return Err("Input should be 48 bytes in length");
    }
    let mut hasher = Sha256::new();
    hasher.update(validator_pubkey);
    hasher.update([0; 16]);
    let result = hasher.finalize();
    Ok(B256::from_slice(&result))
}

pub fn mock_proof(
    validator_index: u64,
    validator_pubkey_hash: B256,
    withdrawal_amount_gwei: u64,
    timestamp: u64,
    full_withdrawal: bool,
) -> WithdrawalProof {
    let withdrawal_journal = WithdrawalJournal {
        validatorIndex: validator_index,
        withdrawalAmountGwei: withdrawal_amount_gwei,
        withdrawalTimestamp: timestamp,
        beaconStateRoot: B256::default(),
        fullWithdrawal: full_withdrawal,
        validatorPubkeyHash: validator_pubkey_hash,
    };

    WithdrawalProof::from(withdrawal_journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mock_proof_test() {
        let withdrawal_proof = mock_proof(1, B256::default(), 100, 1, false);
        withdrawal_proof.verify_validator_fields().unwrap();
        withdrawal_proof.verify_withdrawal().unwrap();
        assert!(!withdrawal_proof.is_full_withdrawal());

        let withdrawal_proof = mock_proof(1, B256::default(), 100, 1, true);
        withdrawal_proof.verify_validator_fields().unwrap();
        withdrawal_proof.verify_withdrawal().unwrap();
        assert!(withdrawal_proof.is_full_withdrawal());
    }
}
