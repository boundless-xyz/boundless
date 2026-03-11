// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use redis::AsyncCommands;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::{JobState, Priority, ReadyTask, TaskDbErr};

/// Redis/Valkey 7+ function library. Load with FUNCTION LOAD REPLACE; then invoke with FCALL <function> 0 <args...>.
const TASKDB_LIBRARY: &str = r#"#!lua name=taskdb
local function ready_queue_key(p, worker_type, priority)
  return p .. ':ready:' .. worker_type .. ':' .. priority
end

local function notify_worker(p, worker_type)
  if not worker_type then
    return
  end
  local notify_key = p .. ':notify:' .. worker_type
  redis.call('RPUSH', notify_key, '1')
  redis.call('LTRIM', notify_key, -4096, -1)
end

local function task_ready_queue_key(p, task_key)
  local worker_type = redis.call('HGET', task_key, 'worker_type')
  local priority = redis.call('HGET', task_key, 'priority')
  if not worker_type or not priority then
    return nil
  end
  return ready_queue_key(p, worker_type, priority)
end

local function enqueue_task(p, task_key, compound)
  local ready_key = task_ready_queue_key(p, task_key)
  local worker_type = redis.call('HGET', task_key, 'worker_type')
  local sort_seq = tonumber(redis.call('HGET', task_key, 'sort_seq') or '0')
  if not ready_key or not worker_type or sort_seq <= 0 then
    return 0
  end
  local was_added = redis.call('ZADD', ready_key, 'NX', sort_seq, compound)
  if was_added == 1 then
    notify_worker(p, worker_type)
  end
  return was_added
end

local function remove_task_from_ready(p, task_key, compound)
  local ready_key = task_ready_queue_key(p, task_key)
  if ready_key then
    redis.call('ZREM', ready_key, compound)
  end
end

redis.register_function('create_stream', function(keys, args)
  local p = args[1]
  local worker_type = args[2]
  local reserved = args[3]
  local be_mult = args[4]
  local user_id = args[5]
  local stream_id = args[6]
  local lookup_key = p .. ':stream:lookup:' .. user_id .. ':' .. worker_type
  local existing = redis.call('GET', lookup_key)
  if existing then
    return existing
  end
  redis.call('SET', lookup_key, stream_id)
  local stream_key = p .. ':stream:' .. stream_id
  redis.call('HSET', stream_key,
    'worker_type', worker_type,
    'reserved', reserved,
    'be_mult', be_mult,
    'user_id', user_id,
    'running', '0',
    'ready', '0',
    'priority', 'inf')
  redis.call('ZADD', p .. ':streams:priority:' .. worker_type, 'inf', stream_id)
  return stream_id
end)

redis.register_function('create_job', function(keys, args)
  local p = args[1]
  local stream_id = args[2]
  local task_def = args[3]
  local max_retries = args[4]
  local timeout_secs = args[5]
  local user_id = args[6]
  local job_id = args[7]
  local now = args[8]
  local priority = args[9]
  local stream_key = p .. ':stream:' .. stream_id
  if redis.call('EXISTS', stream_key) == 0 then
    return redis.error_reply('missing stream: ' .. stream_id)
  end
  local worker_type = redis.call('HGET', stream_key, 'worker_type')
  local sort_seq = tostring(redis.call('INCR', p .. ':task_sequence'))
  redis.call('HSET',
    p .. ':job:' .. job_id .. ':meta',
    'state', 'running',
    'error', '',
    'user_id', user_id,
    'reported', '0',
    'priority', priority)
  redis.call('SADD', p .. ':jobs:running:' .. user_id, job_id)
  local task_key = p .. ':task:' .. job_id .. ':init'
  redis.call('HSET',
    task_key,
    'stream_id', stream_id,
    'worker_type', worker_type,
    'priority', priority,
    'sort_seq', sort_seq,
    'task_def', task_def,
    'prerequisites', '[]',
    'state', 'ready',
    'created_at', now,
    'started_at', '',
    'updated_at', now,
    'waiting_on', '0',
    'progress', '0.0',
    'retries', '0',
    'max_retries', max_retries,
    'timeout_secs', timeout_secs,
    'output', '',
    'error', '')
  redis.call('SADD', p .. ':tasks:by_job:' .. job_id, 'init')
  enqueue_task(p, task_key, job_id .. '|init')
  return job_id
end)

redis.register_function('create_task', function(keys, args)
  local p = args[1]
  local job_id = args[2]
  local task_id = args[3]
  local stream_id = args[4]
  local task_def = args[5]
  local prereqs_json = args[6]
  local max_retries = args[7]
  local timeout_secs = args[8]
  local now = args[9]
  if redis.call('EXISTS', p .. ':job:' .. job_id .. ':meta') == 0 then
    return redis.error_reply('missing job: ' .. job_id)
  end
  if redis.call('EXISTS', p .. ':stream:' .. stream_id) == 0 then
    return redis.error_reply('missing stream: ' .. stream_id)
  end
  local stream_key = p .. ':stream:' .. stream_id
  local worker_type = redis.call('HGET', stream_key, 'worker_type')
  local priority = redis.call('HGET', p .. ':job:' .. job_id .. ':meta', 'priority') or '1'
  local sort_seq = tostring(redis.call('INCR', p .. ':task_sequence'))
  local task_key = p .. ':task:' .. job_id .. ':' .. task_id
  if redis.call('EXISTS', task_key) == 1 then
    return redis.error_reply('task already exists: ' .. task_id)
  end
  local prereqs = cjson.decode(prereqs_json)
  local waiting_on = 0
  for _, pre_task_id in ipairs(prereqs) do
    local pre_task_key = p .. ':task:' .. job_id .. ':' .. pre_task_id
    if redis.call('EXISTS', pre_task_key) == 0 then
      return redis.error_reply('missing prerequisite task: ' .. pre_task_id)
    end
    redis.call('SADD', p .. ':deps:' .. job_id .. ':' .. pre_task_id, task_id)
    local pre_state = redis.call('HGET', pre_task_key, 'state')
    if pre_state ~= 'done' then
      waiting_on = waiting_on + 1
    end
  end
  local state = 'pending'
  if waiting_on <= 0 then
    waiting_on = 0
    state = 'ready'
  end
  redis.call('HSET',
    task_key,
    'stream_id', stream_id,
    'worker_type', worker_type,
    'priority', priority,
    'sort_seq', sort_seq,
    'task_def', task_def,
    'prerequisites', prereqs_json,
    'state', state,
    'created_at', now,
    'started_at', '',
    'updated_at', now,
    'waiting_on', tostring(waiting_on),
    'progress', '0.0',
    'retries', '0',
    'max_retries', max_retries,
    'timeout_secs', timeout_secs,
    'output', '',
    'error', '')
  redis.call('SADD', p .. ':tasks:by_job:' .. job_id, task_id)
  if state == 'ready' then
    enqueue_task(p, task_key, job_id .. '|' .. task_id)
  end
  return 1
end)

redis.register_function('request_work', function(keys, args)
  local p = args[1]
  local worker_type = args[2]
  local now = tonumber(args[3])
  for priority = 0, 2 do
    local ready_key = ready_queue_key(p, worker_type, tostring(priority))
    while true do
      local ready = redis.call('ZPOPMIN', ready_key, 1)
      if #ready == 0 then
        break
      end
      local compound = ready[1]
      local sep = string.find(compound, '|', 1, true)
      if not sep then
        return redis.error_reply('invalid compound task id: ' .. compound)
      end
      local job_id = string.sub(compound, 1, sep - 1)
      local task_id = string.sub(compound, sep + 1)
      local task_key = p .. ':task:' .. job_id .. ':' .. task_id
      local state = redis.call('HGET', task_key, 'state')
      if state == 'ready' then
        redis.call('HSET', task_key,
          'state', 'running',
          'started_at', tostring(now),
          'updated_at', tostring(now))
        local timeout = tonumber(redis.call('HGET', task_key, 'timeout_secs') or '0')
        if timeout < 0 then
          timeout = 0
        end
        redis.call('ZADD', p .. ':tasks:running', now + timeout, compound)
        return {
          job_id,
          task_id,
          redis.call('HGET', task_key, 'task_def'),
          redis.call('HGET', task_key, 'prerequisites'),
          redis.call('HGET', task_key, 'max_retries')
        }
      end
    end
  end
  return nil
end)

redis.register_function('update_task_done', function(keys, args)
  local p = args[1]
  local job_id = args[2]
  local task_id = args[3]
  local output = args[4]
  local now = tonumber(args[5])
  local task_key = p .. ':task:' .. job_id .. ':' .. task_id
  local state = redis.call('HGET', task_key, 'state')
  if state ~= 'ready' and state ~= 'running' then
    return 0
  end
  local compound = job_id .. '|' .. task_id
  redis.call('HSET', task_key,
    'state', 'done',
    'output', output,
    'progress', '1.0',
    'updated_at', tostring(now))
  redis.call('ZREM', p .. ':tasks:running', compound)
  if state == 'ready' then
    remove_task_from_ready(p, task_key, compound)
  end
  local dependents = redis.call('SMEMBERS', p .. ':deps:' .. job_id .. ':' .. task_id)
  for _, dep_tid in ipairs(dependents) do
    local dep_key = p .. ':task:' .. job_id .. ':' .. dep_tid
    local dep_state = redis.call('HGET', dep_key, 'state')
    if dep_state and dep_state ~= 'failed' and dep_state ~= 'done' then
      local remaining = redis.call('HINCRBY', dep_key, 'waiting_on', -1)
      if remaining <= 0 then
        redis.call('HSET', dep_key, 'waiting_on', '0', 'state', 'ready', 'updated_at', tostring(now))
        local dep_compound = job_id .. '|' .. dep_tid
        enqueue_task(p, dep_key, dep_compound)
      end
    end
  end
  local all_done = true
  local all_tasks = redis.call('SMEMBERS', p .. ':tasks:by_job:' .. job_id)
  for _, tid in ipairs(all_tasks) do
    local current = redis.call('HGET', p .. ':task:' .. job_id .. ':' .. tid, 'state')
    if current ~= 'done' then
      all_done = false
      break
    end
  end
  if all_done then
    local job_key = p .. ':job:' .. job_id .. ':meta'
    redis.call('HSET', job_key, 'state', 'done')
    local job_user = redis.call('HGET', job_key, 'user_id')
    if job_user then
      redis.call('SREM', p .. ':jobs:running:' .. job_user, job_id)
    end
  end
  return 1
end)

redis.register_function('update_task_failed', function(keys, args)
  local p = args[1]
  local job_id = args[2]
  local task_id = args[3]
  local err = args[4]
  local now = tonumber(args[5])
  local task_key = p .. ':task:' .. job_id .. ':' .. task_id
  local state = redis.call('HGET', task_key, 'state')
  if state ~= 'ready' and state ~= 'running' and state ~= 'pending' then
    return 0
  end
  redis.call('HSET', task_key,
    'state', 'failed',
    'error', err,
    'progress', '1.0',
    'updated_at', tostring(now))
  local compound = job_id .. '|' .. task_id
  if state == 'running' then
    redis.call('ZREM', p .. ':tasks:running', compound)
  elseif state == 'ready' then
    remove_task_from_ready(p, task_key, compound)
  end
  local job_key = p .. ':job:' .. job_id .. ':meta'
  redis.call('HSET', job_key, 'state', 'failed', 'error', err)
  local job_user = redis.call('HGET', job_key, 'user_id')
  if job_user then
    redis.call('SREM', p .. ':jobs:running:' .. job_user, job_id)
  end
  return 1
end)

redis.register_function('update_task_progress', function(keys, args)
  local p = args[1]
  local job_id = args[2]
  local task_id = args[3]
  local progress = tonumber(args[4])
  local now = tonumber(args[5])
  local task_key = p .. ':task:' .. job_id .. ':' .. task_id
  local state = redis.call('HGET', task_key, 'state')
  if state ~= 'ready' and state ~= 'running' then
    return 0
  end
  local current = tonumber(redis.call('HGET', task_key, 'progress') or '0')
  if progress > current then
    redis.call('HSET', task_key, 'progress', tostring(progress))
  end
  redis.call('HSET', task_key, 'updated_at', tostring(now))
  return 1
end)

redis.register_function('update_task_retry', function(keys, args)
  local p = args[1]
  local job_id = args[2]
  local task_id = args[3]
  local now = tonumber(args[4])
  local task_key = p .. ':task:' .. job_id .. ':' .. task_id
  local state = redis.call('HGET', task_key, 'state')
  if state ~= 'running' then
    return 0
  end
  local retries = redis.call('HINCRBY', task_key, 'retries', 1)
  local max_retries = tonumber(redis.call('HGET', task_key, 'max_retries') or '0')
  local compound = job_id .. '|' .. task_id
  redis.call('ZREM', p .. ':tasks:running', compound)
  if retries > max_retries then
    redis.call('HSET',
      task_key,
      'state', 'failed',
      'error', 'retry max hit',
      'progress', '1.0',
      'updated_at', tostring(now))
    local job_key = p .. ':job:' .. job_id .. ':meta'
    redis.call('HSET', job_key, 'state', 'failed', 'error', 'retry max hit')
    local job_user = redis.call('HGET', job_key, 'user_id')
    if job_user then
      redis.call('SREM', p .. ':jobs:running:' .. job_user, job_id)
    end
    return 0
  end
  redis.call('HSET',
    task_key,
    'state', 'ready',
    'error', '',
    'progress', '0.0',
    'updated_at', tostring(now))
  enqueue_task(p, task_key, compound)
  return 1
end)

redis.register_function('delete_job', function(keys, args)
  local p = args[1]
  local job_id = args[2]
  local job_key = p .. ':job:' .. job_id .. ':meta'
  local user_id = redis.call('HGET', job_key, 'user_id')
  if user_id then
    redis.call('SREM', p .. ':jobs:running:' .. user_id, job_id)
  end
  local all_tasks = redis.call('SMEMBERS', p .. ':tasks:by_job:' .. job_id)
  for _, tid in ipairs(all_tasks) do
    local tkey = p .. ':task:' .. job_id .. ':' .. tid
    local state = redis.call('HGET', tkey, 'state')
    local compound = job_id .. '|' .. tid
    if state == 'ready' then
      remove_task_from_ready(p, tkey, compound)
    elseif state == 'running' then
      redis.call('ZREM', p .. ':tasks:running', compound)
    end
    local prereqs_json = redis.call('HGET', tkey, 'prerequisites') or '[]'
    local prereqs = cjson.decode(prereqs_json)
    for _, pre in ipairs(prereqs) do
      redis.call('SREM', p .. ':deps:' .. job_id .. ':' .. pre, tid)
    end
    redis.call('DEL', p .. ':deps:' .. job_id .. ':' .. tid)
    redis.call('DEL', tkey)
  end
  redis.call('DEL', p .. ':tasks:by_job:' .. job_id)
  redis.call('DEL', job_key)
  return 1
end)
"#;

static TASKDB_LIBRARY_LOADED: AtomicBool = AtomicBool::new(false);

async fn ensure_functions_loaded(
    conn: &mut redis::aio::MultiplexedConnection,
) -> Result<(), TaskDbErr> {
    if TASKDB_LIBRARY_LOADED.load(Ordering::Acquire) {
        return Ok(());
    }
    redis::cmd("FUNCTION")
        .arg("LOAD")
        .arg("REPLACE")
        .arg(TASKDB_LIBRARY)
        .query_async::<_, String>(conn)
        .await?;
    TASKDB_LIBRARY_LOADED.store(true, Ordering::Release);
    Ok(())
}

async fn record<T, Fut>(op: &str, f: Fut) -> Result<T, TaskDbErr>
where
    Fut: std::future::Future<Output = Result<T, TaskDbErr>>,
{
    let start = Instant::now();
    let result = f.await;
    let status = if result.is_ok() { "success" } else { "error" };
    workflow_common::metrics::helpers::record_db_operation(
        op,
        status,
        start.elapsed().as_secs_f64(),
    );
    result
}

#[derive(Clone, Debug)]
pub struct RedisTaskDb {
    client: redis::Client,
    namespace: String,
}

impl RedisTaskDb {
    pub fn new(redis_url: &str) -> Result<Self, TaskDbErr> {
        Self::with_namespace(redis_url, "taskdb")
    }

    pub fn with_namespace(
        redis_url: &str,
        namespace: impl Into<String>,
    ) -> Result<Self, TaskDbErr> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self { client, namespace: namespace.into() })
    }

    async fn conn(&self) -> Result<redis::aio::MultiplexedConnection, TaskDbErr> {
        Ok(self.client.get_multiplexed_async_connection().await?)
    }

    fn prefixed(&self, key: &str) -> String {
        format!("{}:{key}", self.namespace)
    }

    fn notify_key(&self, worker_type: &str) -> String {
        self.prefixed(&format!("notify:{worker_type}"))
    }

    async fn wait_for_notification(
        &self,
        worker_type: &str,
        timeout_secs: f64,
    ) -> Result<bool, TaskDbErr> {
        let mut conn = self.conn().await?;
        let popped: Option<[String; 2]> = redis::cmd("BRPOP")
            .arg(self.notify_key(worker_type))
            .arg(timeout_secs)
            .query_async(&mut conn)
            .await?;
        Ok(popped.is_some())
    }

    pub async fn ping(&self) -> Result<(), TaskDbErr> {
        let mut conn = self.conn().await?;
        let _: String = redis::cmd("PING").query_async(&mut conn).await?;
        Ok(())
    }

    pub async fn flush_namespace(&self) -> Result<(), TaskDbErr> {
        let mut conn = self.conn().await?;
        let pattern = self.prefixed("*");
        let mut cursor = 0_u64;

        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(500)
                .query_async(&mut conn)
                .await?;

            if !keys.is_empty() {
                let _: usize = redis::cmd("DEL").arg(keys).query_async(&mut conn).await?;
            }

            if next == 0 {
                break;
            }
            cursor = next;
        }

        Ok(())
    }

    pub async fn create_stream(
        &self,
        worker_type: &str,
        reserved: i32,
        be_mult: f32,
        user_id: &str,
    ) -> Result<Uuid, TaskDbErr> {
        record("redis:create_stream", async {
            if be_mult == 0.0 {
                return Err(TaskDbErr::InvalidBeMult);
            }

            let stream_id = Uuid::new_v4();
            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let created_id: String = redis::cmd("FCALL")
                .arg("create_stream")
                .arg(0)
                .arg(&self.namespace)
                .arg(worker_type)
                .arg(reserved)
                .arg(be_mult)
                .arg(user_id)
                .arg(stream_id.to_string())
                .query_async(&mut conn)
                .await?;

            Ok(parse_uuid(&created_id, "stream id")?)
        })
        .await
    }

    pub async fn create_job(
        &self,
        stream_id: &Uuid,
        task_def: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
        user_id: &str,
    ) -> Result<Uuid, TaskDbErr> {
        self.create_job_with_priority(
            stream_id,
            task_def,
            max_retries,
            timeout_secs,
            user_id,
            Priority::Medium,
        )
        .await
    }

    pub async fn create_job_with_priority(
        &self,
        stream_id: &Uuid,
        task_def: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
        user_id: &str,
        priority: Priority,
    ) -> Result<Uuid, TaskDbErr> {
        record("redis:create_job", async {
            let now = now_seconds();
            let job_id = Uuid::new_v4();
            let task_def_str = serde_json::to_string(task_def)?;

            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let created_id: String = redis::cmd("FCALL")
                .arg("create_job")
                .arg(0)
                .arg(&self.namespace)
                .arg(stream_id.to_string())
                .arg(task_def_str)
                .arg(max_retries)
                .arg(timeout_secs)
                .arg(user_id)
                .arg(job_id.to_string())
                .arg(now)
                .arg(priority.bucket())
                .query_async(&mut conn)
                .await?;

            Ok(parse_uuid(&created_id, "job id")?)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_task(
        &self,
        job_id: &Uuid,
        task_id: &str,
        stream_id: &Uuid,
        task_def: &JsonValue,
        prereqs: &JsonValue,
        max_retries: i32,
        timeout_secs: i32,
    ) -> Result<(), TaskDbErr> {
        record("redis:create_task", async {
            let now = now_seconds();
            let task_def_str = serde_json::to_string(task_def)?;
            let prereqs_str = serde_json::to_string(prereqs)?;

            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let _: i32 = redis::cmd("FCALL")
                .arg("create_task")
                .arg(0)
                .arg(&self.namespace)
                .arg(job_id.to_string())
                .arg(task_id)
                .arg(stream_id.to_string())
                .arg(task_def_str)
                .arg(prereqs_str)
                .arg(max_retries)
                .arg(timeout_secs)
                .arg(now)
                .query_async(&mut conn)
                .await?;

            Ok(())
        })
        .await
    }

    async fn request_work_inner(&self, worker_type: &str) -> Result<Option<ReadyTask>, TaskDbErr> {
        let now = now_seconds();
        let mut conn = self.conn().await?;
        ensure_functions_loaded(&mut conn).await?;

        let raw: Option<(String, String, String, String, i32)> = redis::cmd("FCALL")
            .arg("request_work")
            .arg(0)
            .arg(&self.namespace)
            .arg(worker_type)
            .arg(now)
            .query_async(&mut conn)
            .await?;

        let Some((job_id, task_id, task_def, prereqs, max_retries)) = raw else {
            return Ok(None);
        };

        Ok(Some(ReadyTask {
            job_id: parse_uuid(&job_id, "job_id")?,
            task_id,
            task_def: serde_json::from_str(&task_def)?,
            prereqs: serde_json::from_str(&prereqs)?,
            max_retries,
        }))
    }

    pub async fn request_work(&self, worker_type: &str) -> Result<Option<ReadyTask>, TaskDbErr> {
        record("redis:request_work", async { self.request_work_inner(worker_type).await }).await
    }

    pub async fn request_work_blocking(
        &self,
        worker_type: &str,
        timeout_secs: u64,
    ) -> Result<Option<ReadyTask>, TaskDbErr> {
        record("redis:request_work_blocking", async {
            if let Some(task) = self.request_work_inner(worker_type).await? {
                return Ok(Some(task));
            }

            if timeout_secs == 0 {
                return Ok(None);
            }

            let deadline = Instant::now() + std::time::Duration::from_secs(timeout_secs);

            loop {
                let now = Instant::now();
                if now >= deadline {
                    return Ok(None);
                }

                let remaining_secs =
                    deadline.saturating_duration_since(now).as_secs_f64().max(0.001);
                let notified = self.wait_for_notification(worker_type, remaining_secs).await?;

                if let Some(task) = self.request_work_inner(worker_type).await? {
                    return Ok(Some(task));
                }

                if !notified {
                    return Ok(None);
                }
            }
        })
        .await
    }

    pub async fn update_task_done(
        &self,
        job_id: &Uuid,
        task_id: &str,
        output: JsonValue,
    ) -> Result<bool, TaskDbErr> {
        record("redis:update_task_done", async {
            let now = now_seconds();
            let output_str = serde_json::to_string(&output)?;

            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let updated: i32 = redis::cmd("FCALL")
                .arg("update_task_done")
                .arg(0)
                .arg(&self.namespace)
                .arg(job_id.to_string())
                .arg(task_id)
                .arg(output_str)
                .arg(now)
                .query_async(&mut conn)
                .await?;

            Ok(updated == 1)
        })
        .await
    }

    pub async fn update_task_failed(
        &self,
        job_id: &Uuid,
        task_id: &str,
        error: &str,
    ) -> Result<bool, TaskDbErr> {
        record("redis:update_task_failed", async {
            let now = now_seconds();
            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let updated: i32 = redis::cmd("FCALL")
                .arg("update_task_failed")
                .arg(0)
                .arg(&self.namespace)
                .arg(job_id.to_string())
                .arg(task_id)
                .arg(error)
                .arg(now)
                .query_async(&mut conn)
                .await?;

            Ok(updated == 1)
        })
        .await
    }

    pub async fn update_task_progress(
        &self,
        job_id: &Uuid,
        task_id: &str,
        progress: f32,
    ) -> Result<bool, TaskDbErr> {
        record("redis:update_task_progress", async {
            let now = now_seconds();
            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let updated: i32 = redis::cmd("FCALL")
                .arg("update_task_progress")
                .arg(0)
                .arg(&self.namespace)
                .arg(job_id.to_string())
                .arg(task_id)
                .arg(progress)
                .arg(now)
                .query_async(&mut conn)
                .await?;

            Ok(updated == 1)
        })
        .await
    }

    pub async fn update_task_retry(&self, job_id: &Uuid, task_id: &str) -> Result<bool, TaskDbErr> {
        record("redis:update_task_retry", async {
            let now = now_seconds();
            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let updated: i32 = redis::cmd("FCALL")
                .arg("update_task_retry")
                .arg(0)
                .arg(&self.namespace)
                .arg(job_id.to_string())
                .arg(task_id)
                .arg(now)
                .query_async(&mut conn)
                .await?;

            Ok(updated == 1)
        })
        .await
    }

    pub async fn requeue_tasks(&self, limit: i64) -> Result<usize, TaskDbErr> {
        record("redis:requeue_tasks", async {
            let now = now_seconds();
            let mut conn = self.conn().await?;

            let expired: Vec<String> = redis::cmd("ZRANGEBYSCORE")
                .arg(self.prefixed("tasks:running"))
                .arg("-inf")
                .arg(now)
                .arg("LIMIT")
                .arg(0)
                .arg(limit)
                .query_async(&mut conn)
                .await?;

            drop(conn);
            for compound in &expired {
                let (job_id, task_id) = split_compound(compound)?;
                let _ = self.update_task_retry(&job_id, &task_id).await?;
            }

            Ok(expired.len())
        })
        .await
    }

    pub async fn get_job_state(&self, job_id: &Uuid, user_id: &str) -> Result<JobState, TaskDbErr> {
        record("redis:get_job_state", async {
            let mut conn = self.conn().await?;
            let key = self.prefixed(&format!("job:{job_id}:meta"));
            let job_user: Option<String> = conn.hget(&key, "user_id").await?;

            if job_user.as_deref() != Some(user_id) {
                return Err(TaskDbErr::NotFound(format!(
                    "job {job_id} not found for user {user_id}"
                )));
            }

            let raw: String = conn.hget(&key, "state").await?;
            match raw.as_str() {
                "running" => Ok(JobState::Running),
                "done" => Ok(JobState::Done),
                "failed" => Ok(JobState::Failed),
                _ => Err(TaskDbErr::InternalErr(format!("invalid job state: {raw}"))),
            }
        })
        .await
    }

    pub async fn get_stream(
        &self,
        user_id: &str,
        worker_type: &str,
    ) -> Result<Option<Uuid>, TaskDbErr> {
        record("redis:get_stream", async {
            let mut conn = self.conn().await?;
            let key = self.prefixed(&format!("stream:lookup:{user_id}:{worker_type}"));
            let stream_id: Option<String> = conn.get(&key).await?;
            stream_id.map(|s| parse_uuid(&s, "stream id")).transpose()
        })
        .await
    }

    pub async fn get_job_unresolved(&self, job_id: &Uuid) -> Result<i64, TaskDbErr> {
        record("redis:get_job_unresolved", async {
            let mut conn = self.conn().await?;
            let task_ids: Vec<String> =
                conn.smembers(self.prefixed(&format!("tasks:by_job:{job_id}"))).await?;

            let mut unresolved = 0_i64;
            for task_id in task_ids {
                let state: Option<String> =
                    conn.hget(self.prefixed(&format!("task:{job_id}:{task_id}")), "state").await?;
                if state.as_deref() != Some("done") {
                    unresolved += 1;
                }
            }

            Ok(unresolved)
        })
        .await
    }

    pub async fn get_concurrent_jobs(&self, user_id: &str) -> Result<i64, TaskDbErr> {
        record("redis:get_concurrent_jobs", async {
            let mut conn = self.conn().await?;
            let count: i64 = conn.scard(self.prefixed(&format!("jobs:running:{user_id}"))).await?;
            Ok(count)
        })
        .await
    }

    pub async fn get_task_retries_running(
        &self,
        job_id: &Uuid,
        task_id: &str,
    ) -> Result<Option<i32>, TaskDbErr> {
        record("redis:get_task_retries_running", async {
            let mut conn = self.conn().await?;
            let key = self.prefixed(&format!("task:{job_id}:{task_id}"));
            let state: Option<String> = conn.hget(&key, "state").await?;
            if state.as_deref() != Some("running") {
                return Ok(None);
            }

            let retries: i32 = conn.hget(&key, "retries").await?;
            Ok(Some(retries))
        })
        .await
    }

    pub async fn get_job_failure(&self, job_id: &Uuid) -> Result<String, TaskDbErr> {
        record("redis:get_job_failure", async {
            let mut conn = self.conn().await?;
            let task_ids: Vec<String> =
                conn.smembers(self.prefixed(&format!("tasks:by_job:{job_id}"))).await?;

            for task_id in task_ids {
                let key = self.prefixed(&format!("task:{job_id}:{task_id}"));
                let state: Option<String> = conn.hget(&key, "state").await?;
                if state.as_deref() == Some("failed") {
                    let error: Option<String> = conn.hget(&key, "error").await?;
                    if let Some(err) = error {
                        if !err.is_empty() {
                            return Ok(err);
                        }
                    }
                }
            }

            Err(TaskDbErr::NotFound(format!("no failed task for job {job_id}")))
        })
        .await
    }

    pub async fn get_task_output<T>(&self, job_id: &Uuid, task_id: &str) -> Result<T, TaskDbErr>
    where
        T: DeserializeOwned,
    {
        record("redis:get_task_output", async {
            let mut conn = self.conn().await?;
            let key = self.prefixed(&format!("task:{job_id}:{task_id}"));
            let state: Option<String> = conn.hget(&key, "state").await?;
            if state.as_deref() != Some("done") {
                return Err(TaskDbErr::NotFound(format!(
                    "task {task_id} for job {job_id} is not done"
                )));
            }

            let output: Option<String> = conn.hget(&key, "output").await?;
            let output = output
                .filter(|value| !value.is_empty())
                .ok_or_else(|| TaskDbErr::InternalErr(format!("task {task_id} has no output")))?;

            let json: JsonValue = serde_json::from_str(&output)?;
            Ok(serde_json::from_value(json)?)
        })
        .await
    }

    pub async fn delete_job(&self, job_id: &Uuid) -> Result<(), TaskDbErr> {
        record("redis:delete_job", async {
            let mut conn = self.conn().await?;
            ensure_functions_loaded(&mut conn).await?;

            let _: i32 = redis::cmd("FCALL")
                .arg("delete_job")
                .arg(0)
                .arg(&self.namespace)
                .arg(job_id.to_string())
                .query_async(&mut conn)
                .await?;
            Ok(())
        })
        .await
    }

    pub async fn clear_completed_jobs(&self) -> Result<i32, TaskDbErr> {
        record("redis:clear_completed_jobs", async {
            let mut conn = self.conn().await?;
            let pattern = self.prefixed("job:*:meta");
            let mut cursor = 0_u64;
            let mut to_delete: Vec<Uuid> = Vec::new();

            loop {
                let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(200)
                    .query_async(&mut conn)
                    .await?;

                for key in keys {
                    let state: Option<String> = conn.hget(&key, "state").await?;
                    if state.as_deref() != Some("done") {
                        continue;
                    }

                    if let Some(job_id) = extract_job_id_from_meta_key(&self.namespace, &key)? {
                        to_delete.push(job_id);
                    }
                }

                if next == 0 {
                    break;
                }
                cursor = next;
            }

            drop(conn);
            for job_id in &to_delete {
                self.delete_job(job_id).await?;
            }

            Ok(to_delete.len() as i32)
        })
        .await
    }
}

fn now_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("time moved backwards").as_secs_f64()
}

fn parse_uuid(raw: &str, label: &str) -> Result<Uuid, TaskDbErr> {
    Uuid::parse_str(raw)
        .map_err(|err| TaskDbErr::InternalErr(format!("failed to parse {label} '{raw}': {err}")))
}

fn split_compound(compound: &str) -> Result<(Uuid, String), TaskDbErr> {
    let Some((job_id_raw, task_id)) = compound.split_once('|') else {
        return Err(TaskDbErr::InternalErr(format!("invalid task compound id: {compound}")));
    };

    Ok((parse_uuid(job_id_raw, "job_id")?, task_id.to_string()))
}

fn extract_job_id_from_meta_key(namespace: &str, key: &str) -> Result<Option<Uuid>, TaskDbErr> {
    let prefix = format!("{namespace}:job:");
    let suffix = ":meta";

    if !key.starts_with(&prefix) || !key.ends_with(suffix) {
        return Ok(None);
    }

    let trimmed = &key[prefix.len()..key.len() - suffix.len()];
    Ok(Some(parse_uuid(trimmed, "job id")?))
}
