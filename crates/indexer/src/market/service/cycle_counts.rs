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

use super::IndexerService;
use crate::db::market::{CycleCount, IndexerDb};
use crate::market::ServiceError;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use boundless_market::GuestEnv;
use rand::Rng;
use risc0_zkvm::serde::from_slice;
use std::collections::{HashMap, HashSet};

const REQUESTOR_1: Address =
    alloy::primitives::address!("c197ebe12c7bcf1d9f3b415342bdbc795425335c");
const REQUESTOR_2: Address =
    alloy::primitives::address!("e198c6944cae382902a375b0b8673084270a7f8e");
const SIGNAL_REQUESTOR: Address =
    alloy::primitives::address!("734df7809c4ef94da037449c287166d114503198");

const SIGNAL_REQUESTOR_MIN_CYCLES: u64 = 50_000_000_000; // 50 billion
const SIGNAL_REQUESTOR_MAX_CYCLES: u64 = 54_000_000_000; // 54 billion

/// Try to extract program cycles from inline request input data.
/// Returns Some(program_cycles) if extraction succeeds, None otherwise.
fn try_extract_program_cycles(input_type: &str, input_data_hex: &str) -> Option<u64> {
    // Check if inline input
    if input_type != "Inline" {
        tracing::debug!("Skipping URL-based input for cycle count extraction");
        return None;
    }

    // Trim 0x if input data hex starts with it
    let input_data_hex = input_data_hex.trim_start_matches("0x");

    // Decode hex input data
    let input_data = match hex::decode(input_data_hex) {
        Ok(data) => data,
        Err(e) => {
            tracing::debug!("Failed to decode hex input data: {}", e);
            return None;
        }
    };

    // Decode GuestEnv
    match GuestEnv::decode(&input_data) {
        Ok(guest_env) => {
            // Convert stdin bytes to u32 words for risc0 deserialization
            match bytemuck::try_cast_slice::<u8, u32>(&guest_env.stdin) {
                Ok(words) => {
                    // Decode first u64 from stdin words
                    match from_slice::<u64, u32>(words) {
                        Ok(cycle_count) => {
                            tracing::trace!("Successfully decoded cycle count: {}", cycle_count);
                            Some(cycle_count)
                        }
                        Err(e) => {
                            tracing::debug!("Failed to decode cycle count from stdin: {}", e);
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to convert stdin bytes to u32 words: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            tracing::debug!("Failed to decode GuestEnv: {}", e);
            None
        }
    }
}

/// Generate random program cycles for Signal requestor (same logic as CLI)
fn generate_signal_program_cycles() -> u64 {
    let mut rng = rand::rng();
    rng.random_range(SIGNAL_REQUESTOR_MIN_CYCLES..=SIGNAL_REQUESTOR_MAX_CYCLES)
}

/// Convert program cycles to total cycles using 1.0158 multiplier (1.58% overhead)
fn program_cycles_to_total(program_cycles: U256) -> U256 {
    // Multiply by 1.0158 using fixed-point arithmetic
    // 1.0158 * 10000 = 10158
    // So: program_cycles * 10158 / 10000
    let multiplier = U256::from(10158);
    let divisor = U256::from(10000);
    program_cycles.saturating_mul(multiplier) / divisor
}

/// Compute program_cycles and total_cycles based on requestor and input data.
/// Returns Some((program_cycles, total_cycles)) if computation succeeds, None otherwise.
/// total_cycles = program_cycles × 1.0158 (1.58% overhead)
fn compute_cycle_counts(
    client_address: Address,
    input_type: &str,
    input_data: &str,
) -> Option<(U256, U256)> {
    let program_cycles = if client_address == SIGNAL_REQUESTOR {
        // Signal requestor: generate random program cycles
        Some(U256::from(generate_signal_program_cycles()))
    } else if client_address == REQUESTOR_1 || client_address == REQUESTOR_2 {
        // Specific requestors: try to extract from input
        try_extract_program_cycles(input_type, input_data).map(U256::from)
    } else {
        // All other requestors: mark as PENDING
        None
    };

    program_cycles.map(|pc| {
        let tc = program_cycles_to_total(pc);
        (pc, tc)
    })
}

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(super) async fn request_cycle_counts(
        &mut self,
        request_digests: HashSet<B256>,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        if request_digests.is_empty() {
            return Ok(());
        }

        // Check which cycle counts already exist
        let digest_vec: Vec<B256> = request_digests.iter().copied().collect();
        let existing = self.db.has_cycle_counts(&digest_vec).await?;

        // Filter out requests that already have cycle count records
        let new_requests: Vec<B256> =
            request_digests.into_iter().filter(|digest| !existing.contains(digest)).collect();

        if new_requests.is_empty() {
            tracing::info!(
                "process_cycle_counts completed in {:?} [0 new cycle counts]",
                start.elapsed()
            );
            return Ok(());
        }

        tracing::debug!("Requesting cycle counts for {} new requests", new_requests.len());

        // Query proof_requests table for input_type, input_data, and client_address
        let request_inputs = self.db.get_request_inputs(&new_requests).await?;

        let current_timestamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        // Try to compute cycle counts and build batch
        let mut cycle_counts = Vec::new();
        for (request_digest, input_type, input_data, client_address) in request_inputs.clone() {
            // Compute cycle counts based on requestor
            let cycles = compute_cycle_counts(client_address, &input_type, &input_data);

            let (cycle_status, program_cycles, total_cycles) = if let Some((pc, tc)) = cycles {
                ("COMPLETED".to_string(), Some(pc), Some(tc))
            } else {
                ("PENDING".to_string(), None, None)
            };

            cycle_counts.push(CycleCount {
                request_digest,
                cycle_status,
                program_cycles,
                total_cycles,
                created_at: current_timestamp,
                updated_at: current_timestamp,
            });
        }

        // debug client_address to whether we got cycle counts or not
        // build map of client_address to whether we got cycle counts or not. to get cliebnt address we will need to first build a map of request_digest to client_address.
        let request_digest_to_client_address = request_inputs
            .iter()
            .map(|(request_digest, _, _, client_address)| (request_digest, client_address))
            .collect::<HashMap<_, _>>();

        // then print debug of client_address to whether we got cycle counts or not
        let mut seen_client_addresses = HashSet::new();
        for (request_digest, client_address) in request_digest_to_client_address {
            if seen_client_addresses.contains(client_address) {
                continue;
            }
            seen_client_addresses.insert(client_address);
            tracing::debug!(
                "client_address: {:?}, cycle_status: {:?}",
                client_address,
                cycle_counts
                    .iter()
                    .find(|cc| cc.request_digest == *request_digest)
                    .map(|cc| cc.cycle_status.clone())
            );
        }

        // Batch insert
        if !cycle_counts.is_empty() {
            self.db.add_cycle_counts(&cycle_counts).await?;
            tracing::info!(
                "request_cycle_counts completed in {:?} [{} cycle counts inserted: {} COMPLETED (hardcoded), {} PENDING]",
                start.elapsed(),
                cycle_counts.len(),
                cycle_counts.iter().filter(|cc| cc.cycle_status == "COMPLETED").count(),
                cycle_counts.iter().filter(|cc| cc.cycle_status == "PENDING").count(),
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_cycle_counts_signal_requestor() {
        let cycles = compute_cycle_counts(SIGNAL_REQUESTOR, "Inline", "0x1234");
        assert!(cycles.is_some(), "SIGNAL_REQUESTOR should return cycle counts");
        let (program_cycles, total_cycles) = cycles.unwrap();
        let _program_cycles_u64 = program_cycles.to::<u64>();
        assert!(
            (SIGNAL_REQUESTOR_MIN_CYCLES..=SIGNAL_REQUESTOR_MAX_CYCLES)
                .contains(&program_cycles.to::<u64>()),
            "SIGNAL_REQUESTOR program_cycles {} should be between {} and {}",
            program_cycles,
            SIGNAL_REQUESTOR_MIN_CYCLES,
            SIGNAL_REQUESTOR_MAX_CYCLES
        );
        // Verify total_cycles = program_cycles * 1.0158
        let expected_total = program_cycles_to_total(program_cycles);
        assert_eq!(total_cycles, expected_total, "total_cycles should be program_cycles * 1.0158");
    }

    #[test]
    fn test_compute_cycle_counts_requestor_1_with_valid_input() {
        let input_data = "0181a5737464696edc00100000000400000000ccf50a7acc9fccc5cce112cce3";
        let cycles = compute_cycle_counts(REQUESTOR_1, "Inline", input_data);
        assert!(cycles.is_some(), "REQUESTOR_1 with valid input should extract cycle counts");
        let (program_cycles, total_cycles) = cycles.unwrap();
        assert_eq!(
            program_cycles, 67108864,
            "Expected program_cycles 67108864, got {}",
            program_cycles
        );
        // Verify total_cycles = program_cycles * 1.0158
        let expected_total = program_cycles_to_total(program_cycles);
        assert_eq!(total_cycles, expected_total, "total_cycles should be program_cycles * 1.0158");
    }

    #[test]
    fn test_compute_cycle_counts_requestor_2_with_valid_input() {
        let input_data = "0181a5737464696edc00100000000400000000ccf50a7acc9fccc5cce112cce3";
        let cycles = compute_cycle_counts(REQUESTOR_2, "Inline", input_data);
        assert!(cycles.is_some(), "REQUESTOR_2 with valid input should extract cycle counts");
        let (program_cycles, total_cycles) = cycles.unwrap();
        assert_eq!(
            program_cycles, 67108864,
            "Expected program_cycles 67108864, got {}",
            program_cycles
        );
        // Verify total_cycles = program_cycles * 1.0158
        let expected_total = program_cycles_to_total(program_cycles);
        assert_eq!(total_cycles, expected_total, "total_cycles should be program_cycles * 1.0158");
    }

    #[test]
    fn test_compute_cycle_counts_requestor_with_url_input() {
        let cycles = compute_cycle_counts(REQUESTOR_1, "Url", "https://example.com/input");
        assert!(cycles.is_none(), "URL inputs should not extract cycle counts");
    }

    #[test]
    fn test_compute_cycle_counts_unknown_requestor() {
        let unknown_address = Address::from([0xFF; 20]);
        let cycles = compute_cycle_counts(
            unknown_address,
            "Inline",
            "0181a5737464696edc00100000000400000000ccf50a7acc9fccc5cce112cce3",
        );
        assert!(cycles.is_none(), "Unknown requestors should return None (PENDING)");
    }

    #[test]
    fn test_try_extract_program_cycles_valid() {
        let input_data = "0181a5737464696edc00100000000400000000ccf50a7acc9fccc5cce112cce3";
        let program_cycles = try_extract_program_cycles("Inline", input_data);
        assert!(program_cycles.is_some(), "Valid inline input should extract program cycles");
        let count = program_cycles.unwrap();
        assert_eq!(count, 67108864, "Expected program_cycles 67108864, got {}", count);
    }

    #[test]
    fn test_try_extract_program_cycles_invalid_hex() {
        let program_cycles = try_extract_program_cycles("Inline", "invalid_hex_data");
        assert!(program_cycles.is_none(), "Invalid hex data should return None");
    }

    #[test]
    fn test_try_extract_program_cycles_url_input() {
        let program_cycles = try_extract_program_cycles(
            "Url",
            "0181a5737464696edc00100000000400000000ccf50a7acc9fccc5cce112cce3",
        );
        assert!(program_cycles.is_none(), "URL input type should return None");
    }

    #[test]
    fn test_program_cycles_to_total() {
        // Test the 1.0158 multiplier
        let program_cycles = U256::from(100_000_000u64);
        let total_cycles = program_cycles_to_total(program_cycles);
        let expected = U256::from((100_000_000.0 * 1.0158) as u64);
        assert_eq!(total_cycles, expected, "total_cycles should be program_cycles * 1.0158");
        assert_eq!(total_cycles, U256::from(101_580_000u64), "Expected 101,580,000 total cycles");
    }
}
