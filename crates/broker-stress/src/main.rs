// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    node_bindings::Anvil,
    primitives::{utils, U256},
    providers::Provider,
};
use anyhow::{Context, Result};
use axum::{routing::get, Router};
use boundless_market::{
    contracts::{
        hit_points::default_allowance, test_utils::TestCtx, Input, InputType, Offer, Predicate,
        PredicateType, ProofRequest, Requirements,
    },
    input::InputBuilder,
};
use broker::test_utils::BrokerBuilder;
use clap::Parser;
use guest_assessor::ASSESSOR_GUEST_ID;
use guest_set_builder::SET_BUILDER_ID;
use guest_util::{ECHO_ELF, ECHO_ID};
use rand::{rngs::StdRng, Rng, SeedableRng};
use risc0_zkp::core::digest::Digest;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tempfile::NamedTempFile;
use tokio::{
    task::JoinSet,
    time::{sleep, Duration},
};
use tracing_subscriber::filter::EnvFilter;
use url::Url;

mod toxiproxy;

#[derive(Parser, Clone, Debug)]
#[command(author, version, about, long_about = None)]
struct StressTestArgs {
    /// Number of concurrent request spawners
    #[arg(long, default_value_t = 1)]
    spawners: u32,

    /// Time between starting new requests (in ms)
    #[arg(long, default_value_t = 5000)]
    request_speed: u64,

    /// Database URL to use for the sqlite db of the broker.
    #[arg(long, default_value_t = String::from("sqlite::memory:"))]
    database_url: String,

    /// RNG seed
    #[arg(long, default_value_t = 41)]
    rng_seed: u64,

    /// RPC Toxicity - the probability that the RPC connection will be reset
    #[arg(long, default_value_t = 0.0)]
    rpc_reset_toxicity: f32,
}

async fn request_spawner(
    shutdown: Arc<AtomicBool>,
    ctx: Arc<TestCtx>,
    elf_url: &str,
    args: StressTestArgs,
    spawner_id: u32,
) -> Result<()> {
    let mut r = StdRng::seed_from_u64(args.rng_seed + u64::from(spawner_id));

    while !shutdown.load(Ordering::Relaxed) {
        let request = ProofRequest::new(
            ctx.customer_market.index_from_nonce().await?,
            &ctx.customer_signer.address(),
            Requirements::new(
                Digest::from(ECHO_ID),
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            elf_url,
            Input {
                inputType: InputType::Inline,
                data: InputBuilder::new()
                    .write_slice(&vec![0x41u8; r.random_range(1..32)])
                    .build_vec()
                    .unwrap()
                    .into(),
            },
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                biddingStart: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
            },
        );

        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await?;
        tracing::info!("Spawner {} submitted request {}", spawner_id, request.id);

        sleep(Duration::from_millis(args.request_speed)).await;
    }

    Ok(())
}

async fn spawn_broker(
    ctx: &TestCtx,
    rpc_url: Url,
    db_url: &str,
) -> Result<(tokio::task::JoinHandle<()>, NamedTempFile)> {
    // Setup initial balances
    ctx.prover_market.deposit_stake_with_permit(default_allowance(), &ctx.prover_signer).await?;
    ctx.customer_market.deposit(utils::parse_ether("10.0")?).await?;

    // Start broker
    let (broker, config_file) =
        BrokerBuilder::new_test(ctx, rpc_url).await.with_db_url(db_url.to_string()).build().await?;
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    Ok((broker_task, config_file))
}

/// Basic handler that responds with a static string.
async fn serve_elf() -> &'static [u8] {
    ECHO_ELF
}

#[tokio::main]
async fn main() -> Result<()> {
    std::env::set_var("RISC0_DEV_MODE", "true");
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let args = StressTestArgs::parse();

    let app = Router::new().route("/", get(serve_elf));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let elf_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Setup anvil and test environment
    let anvil = Anvil::new().spawn();

    // Setup toxiproxy to anvil
    toxiproxy::up().await?;
    let rpc_url = toxiproxy::proxy_rpc(&anvil.endpoint(), args.rng_seed).await?;

    // Setup test context
    let ctx = Arc::new(
        TestCtx::new_with_rpc_url(
            &anvil,
            &rpc_url,
            SET_BUILDER_ID.into(),
            ASSESSOR_GUEST_ID.into(),
        )
        .await
        .context("Failed to create test context")?,
    );
    let (broker_task, _config_file) =
        spawn_broker(&ctx, Url::parse(&rpc_url).unwrap(), &args.database_url).await?;

    let mut tasks = JoinSet::new();
    let shutdown = Arc::new(AtomicBool::new(false));

    // Spawn request generators
    for i in 0..args.spawners {
        // TODO fund a new key for each spawner to avoid collisions.
        let ctx_copy = ctx.clone();
        let args_copy = args.clone();
        let shutdown_copy = shutdown.clone();
        let elf_url = elf_url.clone();
        tasks.spawn(async move {
            request_spawner(shutdown_copy, ctx_copy, &elf_url, args_copy, i).await
        });
    }

    // Add reset toxicity to RPC connection
    toxiproxy::add_reset_toxic(args.rpc_reset_toxicity).await?;

    // Setup ctrl-c handler
    let shutdown_copy = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Failed to register ctrl+c handler");
        tracing::warn!("Starting graceful shutdown...");
        shutdown_copy.store(true, Ordering::Relaxed);
        toxiproxy::down().await.unwrap();
    });

    // Monitor tasks
    loop {
        while let Some(res) = tasks.try_join_next() {
            res.unwrap().unwrap();
        }

        if shutdown.load(Ordering::Relaxed) {
            tasks.abort_all();
            while let Some(res) = tasks.try_join_next() {
                let _ = res.unwrap();
            }
            break;
        }

        sleep(Duration::from_millis(100)).await;
    }

    // Check for broker panic
    if broker_task.is_finished() {
        broker_task.await.unwrap();
    } else {
        broker_task.abort();
    }

    tracing::info!("Completed stress test");
    Ok(())
}
