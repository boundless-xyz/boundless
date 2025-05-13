// Copyright 2025 RISC Zero, Inc.
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

use std::{
    fs::{File, OpenOptions},
    io,
    io::Write,
    os::unix::io::{AsRawFd, FromRawFd},
};

use alloy::{
    hex::FromHex,
    primitives::{Address, Bytes, Signature, U256},
    sol_types::{SolStruct, SolValue},
};
use anyhow::{ensure, Context, Result};
use boundless_cli::{DefaultProver, OrderFulfilled};
use boundless_market::{
    contracts::{eip712_domain, ProofRequest},
    order_stream_client::Order,
    storage::fetch_url,
};
use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct MainArgs {
    /// URL of the SetBuilder program
    #[clap(long)]
    set_builder_url: String,
    /// URL of the Assessor program
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
}

/// Print the result of fulfilling a proof request using the RISC Zero zkVM default prover.
/// This is used to generate test FFI calls for the Forge cheatcode.
#[tokio::main]
async fn main() -> Result<()> {
    let args = MainArgs::parse();
    // Take stdout is ensure no extra data is written to it.
    let mut stdout = take_stdout()?;
    let set_builder_program = fetch_url(&args.set_builder_url).await?;
    let assessor_program = fetch_url(&args.assessor_url).await?;
    let domain = eip712_domain(args.boundless_market_address, args.chain_id.try_into()?);
    let prover = DefaultProver::new(
        set_builder_program,
        assessor_program,
        args.prover_address,
        domain.clone(),
    )?;
    let request = <ProofRequest>::abi_decode(&hex::decode(args.request.trim_start_matches("0x"))?)
        .map_err(|_| anyhow::anyhow!("Failed to decode ProofRequest from input"))?;
    let request_digest = request.eip712_signing_hash(&domain.alloy_struct());
    let order = Order {
        request,
        request_digest,
        signature: Signature::try_from(
            Bytes::from_hex(args.signature.trim_start_matches("0x"))?.as_ref(),
        )?,
    };
    let (fills, root_receipt, assessor_receipt) = prover.fulfill(&[order.clone()]).await?;
    let order_fulfilled = OrderFulfilled::new(fills, root_receipt, assessor_receipt)?;

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
