// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::B256;
use anyhow::Result;
use risc0_zkvm::{default_executor, ExecutorEnv, Journal};

use crate::contracts::ProvingRequest;

pub mod client;

/// Dry run with executor to compute the expected [Journal] and the cycle count (in millions).
/// This can be useful to estimate the cost of the proving request, based on a desired price per
/// million cycles. It can also be useful to ensure the guest can be executed correctly and we do
/// not send into the market unprovable proving requests.
pub fn dry_run(elf: &[u8], input: &[u8]) -> Result<(Journal, u32)> {
    let env = ExecutorEnv::builder().write_slice(input).build()?;
    let executor = default_executor();
    let session_info = executor.execute(env, elf)?;
    let total_mcycles: u32 =
        session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u32>() / 1_000_000;

    Ok((session_info.journal, total_mcycles))
}

// Check that the request is valid.
fn validate_request(request: &ProvingRequest) -> Result<()> {
    // validate image URL
    if request.imageUrl.is_empty() {
        anyhow::bail!("Image URL must not be empty");
    };
    // validate the image ID
    if request.requirements.imageId == B256::default() {
        anyhow::bail!("Image ID must not be ZERO");
    };
    // validate the offer
    if request.offer.timeout == 0 {
        anyhow::bail!("Offer timeout must be greater than 0");
    };
    if request.offer.maxPrice == 0 {
        anyhow::bail!("Offer maxPrice must be greater than 0");
    };
    if request.offer.biddingStart == 0 {
        anyhow::bail!("Offer biddingStart must be greater than 0");
    };

    Ok(())
}
