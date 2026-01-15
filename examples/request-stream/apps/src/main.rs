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

//! # Request Stream Example
//!
//! This example demonstrates how to send a continuous stream of proof requests to the Boundless Market.
//! It showcases a common pattern: monitoring blockchain events and submitting proof requests based on
//! those events.
//!
//! ## Key Concepts Demonstrated
//!
//! 1. **Stream-based Request Processing**: Using Rust streams to process events asynchronously
//! 2. **Request Construction**: Building proof requests with custom input and request IDs
//! 3. **Request Lifecycle**: Submitting requests and waiting for fulfillment
//! 4. **Storage Provider Integration**: Uploading programs and inputs to external storage
//!
//! ## How It Works
//!
//! 1. Monitor the blockchain for new blocks
//! 2. Every N blocks (configurable, default: 2), collect block hashes
//! 3. Construct a proof request with the block hashes as input
//! 4. Submit the request to the Boundless Market
//! 5. Wait for a prover to fulfill the request
//! 6. Repeat for the next block range

use std::str::FromStr;

use alloy::{
    eips::BlockNumberOrTag,
    primitives::{B256, U256},
    providers::Provider,
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{RequestId, RequestInput},
    Client, Deployment, StorageProvider, StorageProviderConfig,
};
use clap::Parser;
use futures::{Stream, StreamExt};
use guest_util::ECHO_ELF;
use std::pin::Pin;
use tokio::time::Duration;
use tracing_subscriber::{filter::LevelFilter, prelude::*, EnvFilter};
use url::Url;

/// Event emitted when a new block range is ready for processing.
///
/// This represents a batch of blocks that will be included in a single proof request.
/// The block hashes are collected and used as input to the guest program.
#[derive(Debug, Clone)]
struct BlockRangeEvent {
    /// The first block number in the range
    start_block: u64,
    /// The last block number in the range
    end_block: u64,
    /// The hashes of all blocks in this range
    block_hashes: Vec<B256>,
}

/// Arguments for the request stream app CLI.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to interact with the Boundless Market.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Number of blocks to include in each request.
    #[clap(long, default_value_t = 2)]
    blocks_per_request: u64,
    /// Configuration for the StorageProvider to use for uploading programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Provider")]
    storage_config: StorageProviderConfig,
    #[clap(flatten, next_help_heading = "Boundless Market Deployment")]
    deployment: Option<Deployment>,
}

/// Main entry point for the request stream example.
///
/// This example demonstrates how to continuously submit proof requests to the Boundless Market
/// based on blockchain events. It's a common pattern for applications that need to prove
/// something about each block or block range.
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging so we can see what's happening
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::from_str("info")?.into())
                .from_env_lossy(),
        )
        .init();

    // Parse command-line arguments (or environment variables)
    let args = Args::parse();

    // Delegate to the run function for better testability
    // This pattern allows us to test the main logic without dealing with CLI parsing
    run(args).await
}

/// Creates a stream that yields block range events every N blocks.
///
/// This function demonstrates how to create an async stream that monitors the blockchain
/// and emits events when a new block range is ready. Streams are ideal for this use case
/// because they allow processing events as they arrive, rather than polling in a loop.
///
/// ## Stream Pattern
///
/// Using a stream here provides several benefits:
/// - **Async processing**: Events are processed as they arrive
/// - **Backpressure handling**: The consumer controls the rate of processing
/// - **Composable**: Can be combined with other stream operations (filter, map, etc.)
///
/// ## How It Works
///
/// 1. Get the current block number to start monitoring from
/// 2. Wait for N new blocks to be mined
/// 3. Collect the block hashes for those N blocks
/// 4. Emit a `BlockRangeEvent` with the collected data
/// 5. Repeat for the next block range
async fn create_block_range_stream<P: Provider + Clone + 'static>(
    provider: P,
    blocks_per_request: u64,
) -> Result<Pin<Box<dyn Stream<Item = Result<BlockRangeEvent>> + Send>>> {
    // Get the current block number to start monitoring from
    // This ensures we don't miss any blocks and start from a known point
    let initial_block =
        provider.get_block_number().await.context("failed to get initial block number")?;

    tracing::info!("Starting block monitoring from block {}", initial_block);

    // Wrap the provider in an Arc so we can share it across async tasks
    // This is necessary because the stream closure needs to capture the provider
    let provider = std::sync::Arc::new(provider);
    let provider_clone = provider.clone();

    // Create an async stream using the async_stream crate
    // This macro makes it easy to create streams that yield values over time
    // We box and pin it so it can be used with StreamExt::next()
    Ok(Box::pin(async_stream::stream! {
        let mut last_processed_block = initial_block;

        // Main loop: continuously monitor for new block ranges
        loop {
            // Calculate the target block number (N blocks ahead of last processed)
            let target_block = last_processed_block + blocks_per_request;

            // Poll the blockchain until we reach the target block
            // This loop ensures we wait for exactly N new blocks
            loop {
                let current_block = match provider_clone.get_block_number().await {
                    Ok(block) => block,
                    Err(e) => {
                        // If we can't get the block number, yield an error and continue
                        // This allows the stream consumer to handle the error
                        yield Err(anyhow::anyhow!("Failed to get block number: {}", e));
                        continue;
                    }
                };

                // Once we've reached the target block, break out of the polling loop
                if current_block >= target_block {
                    break;
                }

                // Sleep for 2 seconds before checking again
                // This prevents excessive RPC calls while waiting for new blocks
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            // Now that we've reached the target block, collect all block hashes in the range
            let start_block = last_processed_block + 1;
            let end_block = target_block;

            tracing::info!("Collecting block hashes from block {} to {}", start_block, end_block);

            let mut block_hashes = Vec::new();
            let mut error_occurred = false;

            // Fetch each block in the range and extract its hash
            for block_num in start_block..=end_block {
                match provider_clone
                    .get_block_by_number(BlockNumberOrTag::Number(block_num))
                    .await
                {
                    Ok(Some(block)) => {
                        // Extract the block hash and add it to our collection
                        block_hashes.push(block.header.hash);
                        tracing::debug!("Block {}: hash = {:?}", block_num, block.header.hash);
                    }
                    Ok(None) => {
                        // Block not found - this shouldn't happen but we handle it gracefully
                        yield Err(anyhow::anyhow!("Block {} not found", block_num));
                        error_occurred = true;
                        break;
                    }
                    Err(e) => {
                        // RPC error - yield the error and stop processing this range
                        yield Err(anyhow::anyhow!("Failed to get block {}: {}", block_num, e));
                        error_occurred = true;
                        break;
                    }
                }
            }

            // If we successfully collected all block hashes, emit an event
            if !error_occurred {
                yield Ok(BlockRangeEvent {
                    start_block,
                    end_block,
                    block_hashes,
                });
                // Update our tracking variable for the next iteration
                last_processed_block = end_block;
            }
        }
    }))
}

/// Constructs input data from block hashes.
///
/// This function demonstrates how to prepare input data for a proof request.
/// The input will be provided to the guest program (zkVM) when the proof is executed.
///
/// ## Input Format
///
/// In this example, we simply concatenate all block hashes together.
/// In a real application, you might want to:
/// - Serialize the data in a structured format (e.g., using bincode, serde)
/// - Include additional metadata
/// - Compress or encode the data
///
/// ## Guest Program Input
///
/// The guest program will receive this input via `risc0_zkvm::env::read()` or similar.
/// Make sure your guest program knows how to deserialize this format!
fn input_function(block_hashes: &[B256]) -> RequestInput {
    let mut input = Vec::new();
    // Concatenate all block hashes into a single byte vector
    // Each hash is 32 bytes (B256), so the total size is 32 * number_of_blocks
    for hash in block_hashes {
        input.extend_from_slice(hash.as_slice());
    }
    RequestInput::builder().write_slice(&input).build_inline().unwrap()
}

/// Constructs a request ID from the block range.
///
/// Request IDs in Boundless Market are composed of:
/// - The requestor's address (your address)
/// - A 32-bit index (unique identifier for this request)
///
/// ## Request ID Structure
///
/// The request ID is a 256-bit value where:
/// - Bits 0-31: The index (u32)
/// - Bits 32-191: The requestor address (160 bits)
/// - Bits 192+: Flags (e.g., smart contract signature flag)
///
/// ## Choosing an Index
///
/// In this example, we use the start block number as the index.
/// This ensures:
/// - Each block range gets a unique request ID
/// - Request IDs are deterministic (same block range = same ID)
/// - Easy to identify which block range a request corresponds to
///
/// **Important**: The index must be unique per requestor address. If you submit
/// multiple requests with the same index, only one will be accepted.
fn request_id_function(address: alloy::primitives::Address, start_block: u64) -> RequestId {
    // Use the start block number as the index
    // Note: We cast to u32, so block numbers > 2^32 will wrap around
    // For production, consider using a more sophisticated encoding
    let request_index = start_block as u32;
    RequestId::new(address, request_index)
}

/// Main logic which creates the Boundless client, monitors blocks, and submits requests.
///
/// This function demonstrates the complete workflow for submitting a stream of proof requests:
/// 1. Create a Boundless client
/// 2. Upload the program to storage
/// 3. Create a stream of block range events
/// 4. For each event, construct and submit a proof request
/// 5. Wait for each request to be fulfilled
///
/// ## Request Lifecycle
///
/// 1. **Construction**: Build a request with program, input, and request ID
/// 2. **Submission**: Submit the request to the Boundless Market contract
/// 3. **Fulfillment**: A prover picks up the request, executes the proof, and submits the result
/// 4. **Verification**: The market verifies the proof and marks the request as fulfilled
async fn run(args: Args) -> Result<()> {
    // ============================================================================
    // Step 1: Create a Boundless Client
    // ============================================================================
    // The Client is your main interface to the Boundless Market. It handles:
    // - Interacting with the Boundless Market smart contract
    // - Managing storage for programs and inputs
    // - Signing transactions with your private key
    // - Tracking request status and fulfillment
    let client = Client::builder()
        .with_rpc_url(args.rpc_url) // Ethereum RPC endpoint
        .with_deployment(args.deployment) // Contract addresses (optional, uses defaults if not provided)
        .with_storage_provider_config(&args.storage_config)? // Where to upload programs/inputs
        .with_private_key(args.private_key.clone()) // Your wallet for signing transactions
        .build()
        .await?;

    // ============================================================================
    // Step 2: Upload the Program
    // ============================================================================
    // Before submitting a request, you need to make the program available to provers.
    // The program is uploaded to your configured storage provider (e.g., S3, IPFS, local file server).
    // Provers will download the program from this URL when they pick up your request.
    //
    // **Note**: In production, you might want to:
    // - Upload the program once and reuse the URL
    // - Use content-addressed storage (hash-based URLs) for caching
    // - Verify the program hash matches what you expect
    let program_url = if let Some(storage_provider) = &client.storage_provider {
        storage_provider.upload_program(ECHO_ELF).await.context("failed to upload program")?
    } else {
        anyhow::bail!("Storage provider is required to upload the program");
    };

    // ============================================================================
    // Step 3: Create the Event Stream
    // ============================================================================
    // Create a stream that monitors the blockchain and emits events when new block ranges
    // are ready. The stream will run indefinitely, yielding new events as blocks are mined.
    //
    // Note: We clone the provider so it can be moved into the stream, which requires 'static lifetime.
    // The provider is cloneable and doesn't need the client to remain alive.
    let provider = client.boundless_market.instance().provider().clone();
    let mut stream = create_block_range_stream(provider, args.blocks_per_request).await?;

    // ============================================================================
    // Step 4: Process Events and Submit Requests
    // ============================================================================
    // This is the main loop: for each block range event, we construct and submit a proof request.
    // The `while let Some(...)` pattern is idiomatic Rust for processing streams.
    while let Some(event_result) = stream.next().await {
        // Handle any errors from the stream (e.g., RPC failures)
        let event = event_result?;

        // ========================================================================
        // Step 4a: Prepare the Request Data
        // ========================================================================
        // Construct the input data that will be provided to the guest program.
        // This is the data the zkVM will process when generating the proof.
        let input = input_function(&event.block_hashes);

        // Construct a unique request ID for this block range.
        // The request ID ensures we can track this specific request through its lifecycle.
        let request_id = request_id_function(args.private_key.address(), event.start_block);

        tracing::info!(
            "Submitting request for blocks {} to {} (request ID: 0x{:x})",
            event.start_block,
            event.end_block,
            U256::try_from(request_id.clone()).unwrap()
        );

        // ========================================================================
        // Step 4b: Build the Proof Request
        // ========================================================================
        // A proof request consists of:
        // - **Program URL**: Where provers can download the program to execute
        // - **Input**: The data to provide to the guest program (can be inline or a URL)
        // - **Request ID**: Unique identifier for this request
        //
        // You can also configure:
        // - **Offer**: Maximum price, timeout, ramp-up period
        // - **Requirements**: Callback address, gas limit, proof type
        let request = client
            .new_request()
            .with_program_url(program_url.clone())? // Program location
            .with_request_input(input) // Input data (inline, not uploaded)
            .with_request_id(request_id); // Unique request identifier

        // ========================================================================
        // Step 4c: Submit the Request
        // ========================================================================
        // Submit the request to the Boundless Market.
        // This step will:
        // 1. Make the request visible to provers
        // 2. Return the request ID and expiration time
        //
        // **Note**: The returned request_id should match what you provided,
        // but it's good practice to use the returned value to ensure consistency.
        let (submitted_request_id, expires_at) = client.submit(request).await?;

        tracing::info!(
            "Submitted request {:x} for blocks {} to {}",
            submitted_request_id,
            event.start_block,
            event.end_block
        );

        // ========================================================================
        // Step 4d: Wait for Fulfillment
        // ========================================================================
        // After submission, provers will:
        // 1. See your request in the market
        // 2. Download the program and input
        // 3. Execute the program in a zkVM
        // 4. Generate a proof
        // 5. Submit the proof on-chain
        //
        // This function polls the blockchain until the request is fulfilled or expires.
        // The `expires_at` timestamp ensures we don't wait forever if no prover picks up the request.
        let _fulfillment = client
            .wait_for_request_fulfillment(
                submitted_request_id,
                Duration::from_secs(5), // Poll every 5 seconds
                expires_at,             // Stop waiting after this timestamp
            )
            .await?;

        tracing::info!(
            "Request {:x} fulfilled for blocks {} to {}",
            submitted_request_id,
            event.start_block,
            event.end_block
        );

        // The fulfillment object contains the proof result (journal, seal, etc.)
        // In this example, we don't use it, but you could:
        // - Verify the journal contains expected output
        // - Trigger a callback contract
        // - Store the result for later use
    }

    // This point is never reached in practice since the stream runs indefinitely
    // But it's good practice to have a return value for testing purposes
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::node_bindings::Anvil;
    use boundless_market::contracts::hit_points::default_allowance;
    use boundless_market::storage::StorageProviderType;
    use boundless_test_utils::market::create_test_ctx;
    use broker::test_utils::BrokerBuilder;
    use test_log::test;
    use tokio::task::JoinSet;

    #[test(tokio::test)]
    async fn test_main() -> Result<()> {
        // Setup anvil and deploy contracts.
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        ctx.prover_market
            .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();

        // A JoinSet automatically aborts all its tasks when dropped
        let mut tasks = JoinSet::new();

        // Start a broker.
        let (broker, _config) =
            BrokerBuilder::new_test(&ctx, anvil.endpoint_url()).await.build().await?;
        tasks.spawn(async move { broker.start_service().await });

        // Start the run function in a separate task so we can cancel it
        let mut run_handle = {
            let args = Args {
                rpc_url: anvil.endpoint_url(),
                private_key: ctx.customer_signer,
                blocks_per_request: 2, // Use default value for testing
                storage_config: StorageProviderConfig::builder()
                    .storage_provider(StorageProviderType::Mock)
                    .build()
                    .unwrap(),
                deployment: Some(ctx.deployment),
            };
            tokio::spawn(async move { run(args).await })
        };

        // Wait for at least one block range to be processed
        // The stream will wait for blocks_per_request blocks (default: 2), collect hashes, submit a request, and wait for fulfillment
        const TEST_TIMEOUT_SECS: u64 = 120; // 2 minutes should be enough for at least one request

        tokio::select! {
            run_result = &mut run_handle => {
                // If run completes (shouldn't happen normally), check if it was an error
                match run_result {
                    Ok(Ok(())) => {
                        // This is unexpected but not necessarily a failure
                        tracing::info!("Run function completed normally");
                    }
                    Ok(Err(e)) => {
                        panic!("Run function returned an error: {:?}", e);
                    }
                    Err(e) => {
                        panic!("Run task panicked: {:?}", e);
                    }
                }
            },

            broker_task_result = tasks.join_next() => {
                panic!("Broker exited unexpectedly: {:?}", broker_task_result.unwrap());
            },

            _ = tokio::time::sleep(tokio::time::Duration::from_secs(TEST_TIMEOUT_SECS)) => {
                // After waiting, we've verified the stream is working
                // Cancel the run task to clean up
                run_handle.abort();
                // Wait a bit for cleanup
                let _ = tokio::time::timeout(
                    tokio::time::Duration::from_secs(5),
                    run_handle
                ).await;
                tracing::info!("Test completed: stream processed events successfully");
            }
        }

        Ok(())
    }

    #[test(tokio::test)]
    async fn test_block_range_stream() -> Result<()> {
        // Test the stream creation and that it yields at least one event
        let anvil = Anvil::new().spawn();

        // Create a test context which sets up the deployment for chain_id 31337
        let ctx = create_test_ctx(&anvil).await.unwrap();

        // Create a client to get a provider (similar to how it's done in run())
        let client = Client::builder()
            .with_rpc_url(anvil.endpoint_url())
            .with_deployment(Some(ctx.deployment))
            .with_storage_provider_config(
                &StorageProviderConfig::builder()
                    .storage_provider(StorageProviderType::Mock)
                    .build()
                    .unwrap(),
            )?
            .build()
            .await?;

        // Clone the provider so it can be moved into the stream
        // The provider is cloneable and doesn't need the client to live
        let provider = client.boundless_market.instance().provider().clone();
        const DEFAULT_BLOCKS_PER_REQUEST: u64 = 2;
        let mut stream = create_block_range_stream(provider, DEFAULT_BLOCKS_PER_REQUEST).await?;

        // Create a timeout for the test (should be enough for blocks on anvil)
        let timeout = tokio::time::Duration::from_secs(60);
        let start = std::time::Instant::now();

        // Try to get at least one event from the stream
        let event_result = tokio::time::timeout(timeout, stream.next()).await;

        match event_result {
            Ok(Some(Ok(event))) => {
                // Verify the event structure
                assert_eq!(
                    event.end_block - event.start_block + 1,
                    DEFAULT_BLOCKS_PER_REQUEST,
                    "Event should cover exactly {} blocks",
                    DEFAULT_BLOCKS_PER_REQUEST
                );
                assert_eq!(
                    event.block_hashes.len(),
                    DEFAULT_BLOCKS_PER_REQUEST as usize,
                    "Event should contain {} block hashes",
                    DEFAULT_BLOCKS_PER_REQUEST
                );
                tracing::info!(
                    "Successfully received block range event: blocks {} to {}",
                    event.start_block,
                    event.end_block
                );
                Ok(())
            }
            Ok(Some(Err(e))) => {
                panic!("Stream returned an error: {:?}", e);
            }
            Ok(None) => {
                panic!("Stream ended unexpectedly");
            }
            Err(_) => {
                panic!(
                    "Timeout waiting for stream event after {:?}. This might indicate the stream is not producing events.",
                    start.elapsed()
                );
            }
        }
    }
}
