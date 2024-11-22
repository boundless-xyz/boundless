// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use crate::{
    redis::{self},
    tasks::{read_image_id, serialize_obj, RECEIPT_PATH, SEGMENTS_PATH},
    Agent, Args, TaskType,
};
use anyhow::{bail, Context, Result};
use risc0_zkvm::{
    compute_image_id, sha::Digestible, ExecutorEnv, ExecutorImpl, Journal, NullSegmentRef, Receipt,
    Segment,
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
    CompressType, ExecutorReq, ExecutorResp, FinalizeReq, JoinReq, ProveReq, ResolveReq, SnarkReq,
    AUX_WORK_TYPE, JOIN_WORK_TYPE, PROVE_WORK_TYPE, SNARK_WORK_TYPE,
};
// use tempfile::NamedTempFile;
use tokio::task::{JoinHandle, JoinSet};
use uuid::Uuid;

const ELF_MAGIC: [u8; 4] = [0x7f, 0x45, 0x4c, 0x46];
const TASK_QUEUE_SIZE: usize = 100; // TODO: could be bigger, but requires testing IRL
const CONCURRENT_SEGMENTS: usize = 50; // This peaks around ~4GB

// TODO: cleanup arg count
#[allow(clippy::too_many_arguments)]
async fn process_task(
    args: &Args,
    pool: &PgPool,
    prove_stream: &Uuid,
    join_stream: &Uuid,
    aux_stream: &Uuid,
    snark_stream: &Uuid,
    job_id: &Uuid,
    tree_task: &Task,
    segment_index: Option<u32>,
    assumptions: &[String],
    compress_type: CompressType,
) -> Result<()> {
    match tree_task.command {
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
        TaskCmd::Finalize => {
            // Optionally create the Resolve task ahead of the finalize
            let dep_task = if assumptions.is_empty() {
                tree_task.depends_on[0].to_string()
            } else {
                let task_def = serde_json::to_value(TaskType::Resolve(ResolveReq {
                    max_idx: tree_task.depends_on[0],
                }))
                .context("Failed to serialize resolve req")?;

                let prereqs = serde_json::json!([format!("{}", tree_task.depends_on[0])]);
                let task_id = "resolve";

                let assumption_len: i32 = assumptions
                    .len()
                    .try_into()
                    .context("Failed to convert assumption count to i32")?;
                taskdb::create_task(
                    pool,
                    job_id,
                    task_id,
                    prove_stream,
                    &task_def,
                    &prereqs,
                    args.resolve_retries,
                    args.resolve_timeout * assumption_len,
                )
                .await
                .context("create_task (resolve) failure during finalize creation")?;

                task_id.to_string()
            };

            let task_def = serde_json::to_value(TaskType::Finalize(FinalizeReq {
                max_idx: tree_task.depends_on[0],
            }))
            .context("Failed to serialize finalize task-type")?;
            let prereqs = serde_json::json!([dep_task]);

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
                    snark_stream,
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

/// Run the executor emitting the segments and session to hot storage
///
/// Writes out all segments async using tokio tasks then waits for all
/// tasks to complete before exiting.
pub async fn executor(agent: &Agent, job_id: &Uuid, request: &ExecutorReq) -> Result<ExecutorResp> {
    let mut conn = redis::get_connection(&agent.redis_pool).await?;
    let job_prefix = format!("job:{job_id}");

    // Fetch ELF binary data
    let elf_key = format!("{ELF_BUCKET_DIR}/{}", request.image);
    tracing::info!("Downloading - {}", elf_key);
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
    if elf_data[0..ELF_MAGIC.len()] != ELF_MAGIC {
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

        let assumption_claim = receipt.inner.claim()?.digest().to_string();
        let assumption_key = format!("{receipts_key}:{assumption_claim}");
        redis::set_key_with_expiry(
            &mut conn,
            &assumption_key,
            receipt_bytes,
            Some(agent.args.redis_ttl),
        )
        .await
        .context("Failed to put assumption claim in redis")?;
        assumption_receipts.push(receipt);
    }

    // Set the exec limit in 1 million cycle increments
    let mut exec_limit = agent.args.exec_cycle_limit * 1024 * 1024;

    // Assign the requested exec limit if its lower than the global limit
    if let Some(req_exec_limit) = request.exec_limit {
        let req_exec_limit = req_exec_limit * 1024 * 1024;
        if req_exec_limit < exec_limit {
            tracing::info!(
                "Assigning a requested lower execution limit of: {req_exec_limit} cycles"
            );
            exec_limit = req_exec_limit;
        }
    }

    // set the segment prefix
    let segments_prefix = format!("{job_prefix}:{SEGMENTS_PATH}");

    let mut writer_tasks = JoinSet::new();
    // queue segments into a spmc queue
    let (segment_tx, segment_rx) = async_channel::bounded::<Segment>(CONCURRENT_SEGMENTS);
    let (task_tx, task_rx) = async_channel::bounded(TASK_QUEUE_SIZE);

    let mut writer_conn = redis::get_connection(&agent.redis_pool).await?;
    let segments_prefix_clone = segments_prefix.clone();
    let redis_ttl = agent.args.redis_ttl;

    writer_tasks.spawn(async move {
        while let Ok(segment) = segment_rx.recv().await {
            let index = segment.index;
            tracing::debug!("Starting write of index: {index}");
            let segment_key = format!("{segments_prefix_clone}:{index}");
            let segment_vec = serialize_obj(&segment).expect("Failed to serialize the segment");
            redis::set_key_with_expiry(
                &mut writer_conn,
                &segment_key,
                segment_vec,
                Some(redis_ttl),
            )
            .await
            .expect("Failed to set key with expiry");
            tracing::debug!("Completed write of {index}");

            task_tx.send(index).await.expect("failed to push task into task_tx");
        }
        // Once the segments wraps up, close the task channel to signal completion to the follow up
        // task
        task_tx.close();
    });

    let aux_stream = taskdb::get_stream(&agent.db_pool, &request.user_id, AUX_WORK_TYPE)
        .await
        .context("Failed to get AUX stream")?
        .with_context(|| format!("Customer {} missing aux stream", request.user_id))?;

    let prove_stream = taskdb::get_stream(&agent.db_pool, &request.user_id, PROVE_WORK_TYPE)
        .await
        .context("Failed to get GPU Prove stream")?
        .with_context(|| format!("Customer {} missing gpu stream", request.user_id))?;

    let join_stream = taskdb::get_stream(&agent.db_pool, &request.user_id, JOIN_WORK_TYPE)
        .await
        .context("Failed to get GPU Join stream")?
        .with_context(|| format!("Customer {} missing gpu stream", request.user_id))?;

    let snark_stream = taskdb::get_stream(&agent.db_pool, &request.user_id, SNARK_WORK_TYPE)
        .await
        .context("Failed to get SNARK stream")?
        .with_context(|| format!("Customer {} missing snark stream", request.user_id))?;

    let job_id_copy = *job_id;
    let pool_copy = agent.db_pool.clone();
    let assumptions = request.assumptions.clone();
    let assumption_count = assumptions.len();
    let args_copy = agent.args.clone();
    let compress_type = request.compress;
    let exec_only = request.execute_only;

    writer_tasks.spawn(async move {
        let mut planner = Planner::default();

        while let Ok(segment_index) = task_rx.recv().await {
            if exec_only {
                continue;
            }
            planner.enqueue_segment().expect("Failed to enqueue segment");

            while let Some(tree_task) = planner.next_task() {
                process_task(
                    &args_copy,
                    &pool_copy,
                    &prove_stream,
                    &join_stream,
                    &aux_stream,
                    &snark_stream,
                    &job_id_copy,
                    tree_task,
                    Some(segment_index),
                    &assumptions,
                    compress_type,
                )
                .await
                .expect("Failed to process task and insert into taskdb");
            }
        }

        if !exec_only {
            planner.finish().expect("Failed to finish planner");
            while let Some(tree_task) = planner.next_task() {
                process_task(
                    &args_copy,
                    &pool_copy,
                    &prove_stream,
                    &join_stream,
                    &aux_stream,
                    &snark_stream,
                    &job_id_copy,
                    tree_task,
                    None,
                    &assumptions,
                    compress_type,
                )
                .await
                .expect("Failed to process task and insert into taskdb");
            }
        }
    });

    tracing::info!("Starting execution of job: {}", job_id);

    // let file_stderr = NamedTempFile::new()?;
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
                // .stderr(file_stderr)
                .write_slice(&input_data)
                .session_limit(Some(exec_limit))
                .segment_limit_po2(segment_po2)
                .build()?;

            let mut exec = ExecutorImpl::from_elf(env, &elf_data)?;

            let mut segments = 0;
            let session = exec
                .run_with_callback(|segment| {
                    segments += 1;
                    // Send segments to write queue, blocking if the queue is full.
                    if !exec_only {
                        segment_tx.send_blocking(segment).unwrap();
                    }
                    Ok(Box::new(NullSegmentRef {}))
                })
                .context("Failed to run executor")?;

            // close the segment queue to trigger the workers to wrap up and exit
            segment_tx.close();

            Ok(SessionData {
                segment_count: session.segments.len(),
                user_cycles: session.user_cycles,
                total_cycles: session.total_cycles,
                journal: session.journal,
            })
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

    match session.journal {
        Some(journal) => {
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
        }
        None => {
            // Optionally handle the case where there is no journal
            tracing::error!("No journal to update.");
        }
    }

    // First join all tasks and collect results
    while let Some(res) = writer_tasks.join_next().await {
        match res {
            Ok(()) => continue,
            Err(err) => {
                tracing::error!("queue monitor sub task failed: {err:?}");
                bail!(err);
            }
        }
    }

    tracing::info!("Done with all IO tasks");

    let resp = ExecutorResp {
        segments: session.segment_count as u64,
        user_cycles: session.user_cycles,
        total_cycles: session.total_cycles,
        assumption_count: assumption_count as u64,
    };
    Ok(resp)
}
