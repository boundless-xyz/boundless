// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use std::sync::Arc;

use crate::{
    redis::{self},
    tasks::{read_image_id, serialize_obj, COPROC_CB_PATH, RECEIPT_PATH, SEGMENTS_PATH},
    Agent, Args, TaskType,
};
use anyhow::{bail, Context, Result};
use risc0_zkvm::{
    compute_image_id, sha::Digestible, CoprocessorCallback, ExecutorEnv, ExecutorImpl,
    InnerReceipt, Journal, NullSegmentRef, ProveKeccakRequest, Receipt, Segment,
};
use sqlx::postgres::PgPool;
use taskdb::planner::{
    task::{Command as TaskCmd, Task},
    Planner,
};
use tempfile::NamedTempFile;
use workflow_common::{
    s3::{
        ELF_BUCKET_DIR, EXEC_LOGS_BUCKET_DIR, INPUT_BUCKET_DIR, PREFLIGHT_JOURNALS_BUCKET_DIR,
        RECEIPT_BUCKET_DIR, STARK_BUCKET_DIR,
    },
    CompressType, ExecutorReq, ExecutorResp, FinalizeReq, JoinReq, KeccakReq, ProveReq, ResolveReq,
    SnarkReq, UnionReq, AUX_WORK_TYPE, COPROC_WORK_TYPE, JOIN_WORK_TYPE, PROVE_WORK_TYPE,
};
// use tempfile::NamedTempFile;
use tokio::task::JoinHandle;
use uuid::Uuid;

const V2_ELF_MAGIC: &[u8] = b"R0BF"; // const V1_ ELF_MAGIC: [u8; 4] = [0x7f, 0x45, 0x4c, 0x46];
const TASK_QUEUE_SIZE: usize = 100; // TODO: could be bigger, but requires testing IRL
const CONCURRENT_SEGMENTS: usize = 50; // This peaks around ~4GB

// TODO: cleanup arg count
#[allow(clippy::too_many_arguments)]
async fn process_task(
    args: &Args,
    pool: &PgPool,
    prove_stream: &Uuid,
    join_stream: &Uuid,
    union_stream: &Uuid,
    aux_stream: &Uuid,
    job_id: &Uuid,
    tree_task: &Task,
    segment_index: Option<u32>,
    assumptions: &[String],
    compress_type: CompressType,
    keccak_req: Option<KeccakReq>,
) -> Result<()> {
    match tree_task.command {
        TaskCmd::Keccak => {
            let keccak_req = keccak_req.context("keccak_req returned None")?;
            let prereqs = serde_json::json!([]);
            let task_id = format!("{}", tree_task.task_number);
            let task_def = serde_json::to_value(TaskType::Keccak(keccak_req))
                .expect("Failed to serialize coproc (keccak) task-type");

            taskdb::create_task(
                pool,
                job_id,
                &task_id,
                prove_stream,
                &task_def,
                &prereqs,
                args.prove_retries,
                args.prove_timeout,
            )
            .await
            .expect("create_task failure during keccak task creation");
        }
        TaskCmd::Segment => {
            let task_def = serde_json::to_value(TaskType::Prove(ProveReq {
                // Use segment index here instead of task_id to prevent overlapping
                // because the planner is running after we are flushing segments we have to track
                // the segment indexes separately from the task_id counters coming out of the
                // planner TODO: it would be good unify these a little cleaner but
                // it feels like a order of operations issue with trying to keep the
                // executor unblocked as it flushes segments before knowing the
                // planners internal index counter.
                index: segment_index.context("INVALID STATE: segment task without segment index")?
                    as usize,
            }))
            .context("Failed to serialize prove task-type")?;

            // while this is running in the task tree, it has no pre-reqs
            // because it should be able to start asap since the segment should already
            // be flushed at this point
            let prereqs = serde_json::json!([]);
            let task_name = format!("{}", tree_task.task_number);

            taskdb::create_task(
                pool,
                job_id,
                &task_name,
                prove_stream,
                &task_def,
                &prereqs,
                args.prove_retries,
                args.prove_timeout,
            )
            .await
            .context("create_task failure during segment creation")?;
        }
        TaskCmd::Join => {
            let task_def = serde_json::to_value(TaskType::Join(JoinReq {
                idx: tree_task.task_number,
                left: tree_task.depends_on[0],
                right: tree_task.depends_on[1],
            }))
            .context("Failed to serialize join task-type")?;
            let prereqs = serde_json::json!([
                format!("{}", tree_task.depends_on[0]),
                format!("{}", tree_task.depends_on[1])
            ]);
            let task_name = format!("{}", tree_task.task_number);

            taskdb::create_task(
                pool,
                job_id,
                &task_name,
                join_stream,
                &task_def,
                &prereqs,
                args.join_retries,
                args.join_timeout,
            )
            .await
            .context("create_task failure during join creation")?;
        }
        TaskCmd::Union => {
            let task_def = serde_json::to_value(TaskType::Union(UnionReq {
                idx: tree_task.task_number,
                left: tree_task.keccak_depends_on[0],
                right: tree_task.keccak_depends_on[1],
            }))
            .context("Failed to serialize Union task-type")?;
            let prereqs = serde_json::json!([
                format!("{}", tree_task.keccak_depends_on[0]),
                format!("{}", tree_task.keccak_depends_on[1])
            ]);
            let task_id = format!("{}", tree_task.task_number);

            taskdb::create_task(
                pool,
                job_id,
                &task_id,
                union_stream,
                &task_def,
                &prereqs,
                args.join_retries,
                args.join_timeout,
            )
            .await
            .context("create_task failure during Union creation")?;
        }
        TaskCmd::Finalize => {
            let keccak_count = u64::from(!tree_task.keccak_depends_on.is_empty());
            // Optionally create the Resolve task ahead of the finalize
            let assumption_count = i32::try_from(assumptions.len() as u64 + keccak_count)
                .context("Invalid assumption count conversion")?;

            let mut prereqs = vec![tree_task.depends_on[0].to_string()];
            let mut union_max_idx: Option<usize> = None;

            if !tree_task.keccak_depends_on.is_empty() {
                prereqs.push(format!("{}", tree_task.keccak_depends_on[0]));
                union_max_idx = Some(tree_task.keccak_depends_on[0]);
            }

            let task_def = serde_json::to_value(TaskType::Resolve(ResolveReq {
                max_idx: tree_task.depends_on[0],
                union_max_idx,
            }))
            .context("Failed to serialize resolve req")?;
            let task_id = "resolve";

            taskdb::create_task(
                pool,
                job_id,
                task_id,
                join_stream,
                &task_def,
                &serde_json::json!(prereqs),
                args.resolve_retries,
                args.resolve_timeout * assumption_count,
            )
            .await
            .context("create_task (resolve) failure during resolve creation")?;

            let task_def = serde_json::to_value(TaskType::Finalize(FinalizeReq {
                max_idx: tree_task.depends_on[0],
            }))
            .context("Failed to serialize finalize task-type")?;
            let prereqs = serde_json::json!([task_id]);

            let finalize_name = "finalize";
            taskdb::create_task(
                pool,
                job_id,
                finalize_name,
                aux_stream,
                &task_def,
                &prereqs,
                args.finalize_retries,
                args.finalize_timeout,
            )
            .await
            .context("create_task failure during finalize creation")?;

            if compress_type != CompressType::None {
                let task_def = serde_json::to_value(TaskType::Snark(SnarkReq {
                    receipt: job_id.to_string(),
                    compress_type,
                }))
                .context("Failed to serialize snark task-type")?;

                taskdb::create_task(
                    pool,
                    job_id,
                    "snark",
                    prove_stream,
                    &task_def,
                    &serde_json::json!([finalize_name]),
                    args.snark_retries,
                    args.snark_timeout,
                )
                .await
                .context("create_task for snark compression failed")?;
            }
        }
    }

    Ok(())
}

struct SessionData {
    segment_count: usize,
    user_cycles: u64,
    total_cycles: u64,
    journal: Option<Journal>,
}

struct Coprocessor {
    tx: tokio::sync::mpsc::Sender<ProveKeccakRequest>,
}

impl Coprocessor {
    fn new(tx: tokio::sync::mpsc::Sender<ProveKeccakRequest>) -> Self {
        Self { tx }
    }
}

impl CoprocessorCallback for Coprocessor {
    fn prove_keccak(&mut self, request: ProveKeccakRequest) -> Result<()> {
        self.tx.blocking_send(request)?;
        Ok(())
    }

    fn prove_zkr(&mut self, _request: risc0_zkvm::ProveZkrRequest) -> Result<()> {
        // TODO: Implement ZKR proving when needed
        Ok(())
    }
}

/// Run the executor emitting the segments and session to hot storage
///
/// Collects all segments in memory, then writes them to Redis in batch,
/// and finally updates the database with all tasks.
pub async fn executor(agent: &Agent, job_id: &Uuid, request: &ExecutorReq) -> Result<ExecutorResp> {
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");

    // Fetch ELF binary data
    let elf_key = format!("{ELF_BUCKET_DIR}/{}", request.image);
    tracing::debug!("Downloading - {}", elf_key);
    let elf_data = agent.s3_client.read_buf_from_s3(&elf_key).await?;

    // Write the image_id for pulling later
    let image_key = format!("{job_prefix}:image_id");
    redis::set_key_with_expiry(
        &mut conn,
        &image_key,
        request.image.clone(),
        Some(agent.args.redis_ttl),
    )
    .await?;
    let image_id = read_image_id(&request.image)?;

    // Fetch input data
    let input_key = format!("{INPUT_BUCKET_DIR}/{}", request.input);
    let input_data = agent.s3_client.read_buf_from_s3(&input_key).await?;

    // validate elf
    if elf_data[0..V2_ELF_MAGIC.len()] != *V2_ELF_MAGIC {
        bail!("ELF MAGIC mismatch");
    };

    // validate image id
    let computed_id = compute_image_id(&elf_data)?;
    if image_id != computed_id {
        bail!("User supplied imageId does not match generated ID: {image_id} - {computed_id}");
    }

    // Fetch array of Receipts
    let mut assumption_receipts = vec![];
    let receipts_key = format!("{job_prefix}:{RECEIPT_PATH}");

    for receipt_id in request.assumptions.iter() {
        let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{receipt_id}.bincode");
        let receipt_bytes = agent
            .s3_client
            .read_buf_from_s3(&receipt_key)
            .await
            .context("Failed to download receipt from obj store")?;
        let receipt: Receipt =
            bincode::deserialize(&receipt_bytes).context("Failed to decode assumption Receipt")?;

        assumption_receipts.push(receipt.clone());

        let assumption_claim = receipt.inner.claim()?.digest().to_string();

        let succinct_receipt = match receipt.inner {
            InnerReceipt::Succinct(inner) => inner,
            _ => bail!("Invalid assumption receipt, not succinct"),
        };
        let succinct_receipt = succinct_receipt.into_unknown();
        let succinct_receipt_bytes = serialize_obj(&succinct_receipt)
            .context("Failed to serialize succinct assumption receipt")?;

        let assumption_key = format!("{receipts_key}:{assumption_claim}");
        redis::set_key_with_expiry(
            &mut conn,
            &assumption_key,
            succinct_receipt_bytes,
            Some(agent.args.redis_ttl),
        )
        .await
        .context("Failed to put assumption claim in redis")?;
    }

    // Set the exec limit in 1 million cycle increments
    let mut exec_limit = agent.args.exec_cycle_limit * 1024 * 1024;

    // Assign the requested exec limit if its lower than the global limit
    if let Some(req_exec_limit) = request.exec_limit {
        let req_exec_limit = req_exec_limit * 1024 * 1024;
        if req_exec_limit < exec_limit {
            tracing::debug!(
                "Assigning a requested lower execution limit of: {req_exec_limit} cycles"
            );
            exec_limit = req_exec_limit;
        }
    }

    // set the segment prefix
    let segments_prefix = format!("{job_prefix}:{SEGMENTS_PATH}");

    // Collect segments and keccak requests in memory
    let (segment_tx, mut segment_rx) = tokio::sync::mpsc::channel::<Segment>(CONCURRENT_SEGMENTS);
    let (keccak_tx, mut keccak_rx) = tokio::sync::mpsc::channel::<ProveKeccakRequest>(100);

    let aux_stream = taskdb::get_stream(&agent.db_pool, &request.user_id, AUX_WORK_TYPE)
        .await
        .context("Failed to get AUX stream")?
        .with_context(|| format!("Customer {} missing aux stream", request.user_id))?;

    let prove_stream = taskdb::get_stream(&agent.db_pool, &request.user_id, PROVE_WORK_TYPE)
        .await
        .context("Failed to get GPU Prove stream")?
        .with_context(|| format!("Customer {} missing gpu prove stream", request.user_id))?;

    let join_stream = if std::env::var("JOIN_STREAM").is_ok() {
        taskdb::get_stream(&agent.db_pool, &request.user_id, JOIN_WORK_TYPE)
            .await
            .context("Failed to get GPU Join stream")?
            .with_context(|| format!("Customer {} missing gpu join stream", request.user_id))?
    } else {
        prove_stream
    };

    let union_stream = if std::env::var("UNION_STREAM").is_ok() {
        taskdb::get_stream(&agent.db_pool, &request.user_id, JOIN_WORK_TYPE)
            .await
            .context("Failed to get GPU Union stream")?
            .with_context(|| format!("Customer {} missing gpu union stream", request.user_id))?
    } else {
        prove_stream
    };

    let coproc_stream = if std::env::var("COPROC_STREAM").is_ok() {
        taskdb::get_stream(&agent.db_pool, &request.user_id, COPROC_WORK_TYPE)
            .await
            .context("Failed to get GPU Coproc stream")?
            .with_context(|| format!("Customer {} missing gpu coproc stream", request.user_id))?
    } else {
        prove_stream
    };

    let job_id_copy = *job_id;
    let pool_copy = agent.db_pool.clone();
    let assumptions = request.assumptions.clone();
    let assumption_count = assumptions.len();
    let args_copy = agent.args.clone();
    let compress_type = request.compress;
    let exec_only = request.execute_only;

    // Collect segments and keccak requests in memory
    let mut segments = Vec::new();
    let mut keccak_requests = Vec::new();
    let mut guest_fault = false;

    // Spawn task to collect segments and keccak requests
    let collect_task = tokio::spawn(async move {
        while let Some(segment) = segment_rx.recv().await {
            segments.push(segment);
        }

        while let Some(keccak_req) = keccak_rx.recv().await {
            keccak_requests.push(keccak_req);
        }

        (segments, keccak_requests)
    });

    // Spawn task to collect keccak requests from coprocessor
    let coproc = Coprocessor::new(keccak_tx.clone());
    let coproc_task = tokio::spawn(async move {
        // This will be handled by the executor callback
    });

    tracing::info!("Starting execution of job: {}", job_id);

    let log_file = Arc::new(NamedTempFile::new()?);
    let log_file_copy = log_file.clone();
    let guest_log_path = log_file.path().to_path_buf();
    let segment_po2 = agent.args.segment_po2;

    let exec_task: JoinHandle<anyhow::Result<SessionData>> =
        tokio::task::spawn_blocking(move || {
            let mut env = ExecutorEnv::builder();
            for receipt in assumption_receipts {
                env.add_assumption(receipt);
            }

            let env = env
                .stdout(log_file_copy.as_file())
                .write_slice(&input_data)
                .session_limit(Some(exec_limit))
                .coprocessor_callback(coproc)
                .segment_limit_po2(segment_po2)
                .build()?;

            let mut exec = ExecutorImpl::from_elf(env, &elf_data)?;

            let mut segments = 0;
            let res = match exec.run_with_callback(|segment| {
                segments += 1;
                // Send segments to collect queue, blocking if the queue is full.
                if !exec_only {
                    segment_tx.blocking_send(segment).unwrap();
                }
                Ok(Box::new(NullSegmentRef {}))
            }) {
                Ok(session) => Ok(SessionData {
                    segment_count: session.segments.len(),
                    user_cycles: session.user_cycles,
                    total_cycles: session.total_cycles,
                    journal: session.journal,
                }),
                Err(err) => {
                    tracing::error!("Failed to run executor");
                    Err(err)
                }
            };

            // close the segment queue to trigger the workers to wrap up and exit
            drop(segment_tx);
            drop(keccak_tx);

            res
        });

    let session = exec_task
        .await
        .context("Failed to join executor run_with_callback task")?
        .context("execution failed failed")?;

    tracing::info!(
        "execution {} completed with {} segments and {} user-cycles",
        job_id,
        session.segment_count,
        session.user_cycles,
    );

    // Write the guest stdout/stderr logs to object store after completing exec
    agent
        .s3_client
        .write_file_to_s3(&format!("{EXEC_LOGS_BUCKET_DIR}/{job_id}.log"), &guest_log_path)
        .await
        .context("Failed to upload guest logs to object store")?;

    let journal_key = format!("{job_prefix}:journal");

    if let Some(journal) = session.journal {
        if exec_only {
            agent
                .s3_client
                .write_buf_to_s3(
                    &format!("{PREFLIGHT_JOURNALS_BUCKET_DIR}/{job_id}.bin"),
                    journal.bytes,
                )
                .await
                .context("Failed to write journal to obj store")?;
        } else {
            let serialized_journal =
                serialize_obj(&journal).context("Failed to serialize journal")?;

            redis::set_key_with_expiry(
                &mut conn,
                &journal_key,
                serialized_journal,
                Some(agent.args.redis_ttl),
            )
            .await?;
        }
    } else {
        // Optionally handle the case where there is no journal
        tracing::warn!("No journal to update.");
    }

    // Wait for collection to complete
    let (segments, keccak_requests) = collect_task
        .await
        .context("Failed to collect segments and keccak requests")?;

    tracing::info!("Collected {} segments and {} keccak requests", segments.len(), keccak_requests.len());

    // Batch write all segments to Redis
    if !exec_only && !segments.is_empty() {
        tracing::info!("Batch writing {} segments to Redis", segments.len());

        // Prepare batch data
        let mut segment_batch = Vec::new();
        for segment in &segments {
            let segment_key = format!("{segments_prefix}:{}", segment.index);
            let segment_vec = serialize_obj(segment).expect("Failed to serialize the segment");
            segment_batch.push((segment_key, segment_vec));
        }

        // Execute true batch write
        redis::batch_set_keys_with_expiry(
            &mut conn,
            segment_batch,
            Some(agent.args.redis_ttl),
        )
        .await
        .context("Failed to batch write segments to Redis")?;

        tracing::info!("Successfully batch wrote {} segments to Redis", segments.len());
    }

    // Batch write keccak requests to Redis
    if !exec_only && !keccak_requests.is_empty() {
        tracing::info!("Batch writing {} keccak requests to Redis", keccak_requests.len());
        let coproc_prefix = format!("{job_prefix}:{COPROC_CB_PATH}");

        // Prepare batch data
        let mut keccak_batch = Vec::new();
        for keccak_req in &keccak_requests {
            let redis_key = format!("{coproc_prefix}:{}", keccak_req.claim_digest);
            let input_data = bytemuck::cast_slice::<_, u8>(&keccak_req.input).to_vec();
            keccak_batch.push((redis_key, input_data));
        }

        // Execute true batch write
        redis::batch_set_keys_with_expiry(
            &mut conn,
            keccak_batch,
            Some(agent.args.redis_ttl),
        )
        .await
        .context("Failed to batch write keccak requests to Redis")?;

        tracing::info!("Successfully batch wrote {} keccak requests to Redis", keccak_requests.len());
    }

    // Now create all tasks in the database
    if !exec_only {
        tracing::info!("Creating tasks in database");

        // Create a single planner for all operations
        let mut planner = Planner::default();

        // Add all segments and keccaks to planner
        for _ in &segments {
            planner.enqueue_segment().expect("Failed to enqueue segment");
        }
        for _ in &keccak_requests {
            planner.enqueue_keccak().expect("Failed to enqueue keccak");
        }

        // Finish planning to get all tasks
        planner.finish().expect("Planner failed to finish()");

        // Process all tasks in batch
        let mut all_tasks = Vec::new();
        while let Some(tree_task) = planner.next_task() {
            all_tasks.push(tree_task);
        }

        tracing::info!("Planned {} total tasks, creating in database", all_tasks.len());

        // Create all tasks in parallel batches
        let mut task_futures = Vec::new();
        let batch_size = 50; // Process in batches of 50

        for chunk in all_tasks.chunks(batch_size) {
            let chunk_tasks = chunk.to_vec();
            let args_clone = args_copy.clone();
            let pool_clone = pool_copy.clone();
            let prove_stream_clone = prove_stream.clone();
            let join_stream_clone = join_stream.clone();
            let union_stream_clone = union_stream.clone();
            let aux_stream_clone = aux_stream.clone();
            let job_id_clone = job_id_copy;
            let assumptions_clone = assumptions.clone();
            let compress_type_clone = compress_type.clone();
            let coproc_stream_clone = coproc_stream.clone();
            let segments_clone = segments.clone();
            let keccak_requests_clone = keccak_requests.clone();

            let future = tokio::spawn(async move {
                for tree_task in chunk_tasks {
                    // Determine if this is a segment task or keccak task
                    let segment_index = if tree_task.command == TaskCmd::Segment {
                        // Find the corresponding segment index
                        segments_clone.iter().find(|s| s.index as u32 == tree_task.task_number)
                            .map(|s| s.index as u32)
                    } else {
                        None
                    };

                    // Determine if this is a keccak task
                    let keccak_req = if tree_task.command == TaskCmd::Keccak {
                        // Find the corresponding keccak request
                        keccak_requests_clone.iter().find(|k| k.claim_digest.to_string() == tree_task.task_number.to_string())
                            .map(|k| KeccakReq {
                                claim_digest: k.claim_digest,
                                control_root: k.control_root,
                                po2: k.po2,
                            })
                    } else {
                        None
                    };

                    if let Err(err) = process_task(
                        &args_clone,
                        &pool_clone,
                        &prove_stream_clone,
                        &join_stream_clone,
                        &union_stream_clone,
                        &aux_stream_clone,
                        &job_id_clone,
                        &tree_task,
                        segment_index,
                        &assumptions_clone,
                        compress_type_clone,
                        keccak_req,
                    ).await {
                        tracing::error!("Failed to process task: {:?}", err);
                        return Err(err);
                    }
                }
                Ok::<(), anyhow::Error>(())
            });

            task_futures.push(future);
        }

        // Wait for all batches to complete
        for future in task_futures {
            future.await
                .context("Task batch failed to complete")?
                .context("Task batch processing failed")?;
        }

        tracing::info!("Successfully created all {} tasks in database", all_tasks.len());
    }

    tracing::debug!("Done with all IO tasks");

    let resp = ExecutorResp {
        segments: session.segment_count as u64,
        user_cycles: session.user_cycles,
        total_cycles: session.total_cycles,
        assumption_count: assumption_count as u64,
    };
    Ok(resp)
}
