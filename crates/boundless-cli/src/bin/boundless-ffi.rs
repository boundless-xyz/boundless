// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    fs::{File, OpenOptions},
    io,
    io::Write,
    os::unix::io::{AsRawFd, FromRawFd},
};

use alloy::{
    hex::FromHex,
    primitives::{Address, Bytes, PrimitiveSignature, U256},
    sol_types::SolValue,
};
use anyhow::{ensure, Context, Result};
use boundless_cli::{fetch_url, DefaultProver, OrderFulfilled};
use boundless_market::{
    contracts::{eip712_domain, ProofRequest},
    order_stream_client::Order,
};
use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct MainArgs {
    /// URL of the SetBuilder ELF
    #[clap(long)]
    set_builder_url: String,
    /// URL of the Assessor ELF
    #[clap(long)]
    assessor_url: String,
    /// Address of the prover
    #[clap(long)]
    prover_address: Address,
    /// Address of the Boundless market contract
    #[clap(long)]
    boundless_market_address: Address,
    /// Chain ID of the network where Boundless market contract is deployed
    #[clap(long)]
    chain_id: U256,
    /// Hex encoded proof request
    #[clap(long)]
    request: String,
    /// Hex encoded request' signature
    #[clap(long)]
    signature: String,
    /// Whether to revert the fulfill transaction if payment conditions are not met (e.g. the
    /// request is locked to another prover).
    #[clap(long, default_value = "false")]
    require_payment: bool,
}

/// Print the result of fulfilling a proof request using the RISC Zero zkVM default prover.
/// This is used to generate test FFI calls for the Forge cheatcode.
#[tokio::main]
async fn main() -> Result<()> {
    let args = MainArgs::parse();
    // Take stdout is ensure no extra data is written to it.
    let mut stdout = take_stdout()?;
    let set_builder_elf = fetch_url(&args.set_builder_url).await?;
    let assessor_elf = fetch_url(&args.assessor_url).await?;
    let domain = eip712_domain(args.boundless_market_address, args.chain_id.try_into()?);
    let prover = DefaultProver::new(set_builder_elf, assessor_elf, args.prover_address, domain)?;
    let order = Order {
        request: <ProofRequest>::abi_decode(
            &hex::decode(args.request.trim_start_matches("0x"))?,
            true,
        )
        .map_err(|_| anyhow::anyhow!("Failed to decode ProofRequest from input"))?,
        signature: PrimitiveSignature::try_from(
            Bytes::from_hex(args.signature.trim_start_matches("0x"))?.as_ref(),
        )?,
    };
    let (fill, root_receipt, _, assessor_receipt) =
        prover.fulfill(order.clone(), args.require_payment).await?;
    let order_fulfilled =
        OrderFulfilled::new(fill, root_receipt, assessor_receipt, args.prover_address)?;

    // Forge test FFI calls expect hex encoded bytes sent to stdout
    write!(&mut stdout, "{}", hex::encode(order_fulfilled.abi_encode()))
        .context("failed to write to stdout")?;
    stdout.flush().context("failed to flush stdout")?;
    Ok(())
}

/// "Takes" stdout, returning a handle and ensuring no other code in this process can write to it.
/// This is used to ensure that no additional data (e.g. log lines) is written to stdout, as any
/// extra will cause a decoding failure in the Forge FFI cheatcode.
fn take_stdout() -> Result<File> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    // Ensure all buffered data is written before redirection
    handle.flush()?;

    let devnull = OpenOptions::new().write(true).open("/dev/null")?;

    unsafe {
        // Create a copy of stdout to use for our output.
        let dup_fd = libc::dup(handle.as_raw_fd());
        ensure!(dup_fd >= 0, "call to libc::dup failed: {}", dup_fd);
        // Redirect stdout to the fd we opened for /dev/null
        let dup2_result = libc::dup2(devnull.as_raw_fd(), libc::STDOUT_FILENO);
        ensure!(dup2_result >= 0, "call to libc::dup2 failed: {}", dup2_result);
        Ok(File::from_raw_fd(dup_fd))
    }
}
