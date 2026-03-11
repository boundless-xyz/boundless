// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use taskdb::ReadyTask;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct ApiClient {
    base_url: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct WorkerTask {
    job_id: String,
    task_id: String,
    task_def: Value,
    prereqs: Value,
    max_retries: i32,
}

#[derive(Debug, Deserialize)]
struct TaskUpdateRes {
    updated: bool,
}

#[derive(Debug, Deserialize)]
struct TaskRetriesRunningRes {
    retries: Option<i32>,
}

#[derive(Debug, Serialize)]
struct TaskDoneReq {
    output: Value,
}

#[derive(Debug, Serialize)]
struct TaskFailedReq<'a> {
    error: &'a str,
}

#[derive(Debug, Serialize)]
struct WorkerClaimQuery {
    wait_timeout_secs: u64,
}

impl WorkerTask {
    fn into_ready_task(self) -> Result<ReadyTask> {
        Ok(ReadyTask {
            job_id: Uuid::parse_str(&self.job_id)
                .with_context(|| format!("Invalid worker job_id {}", self.job_id))?,
            task_id: self.task_id,
            task_def: self.task_def,
            prereqs: self.prereqs,
            max_retries: self.max_retries,
        })
    }
}

impl ApiClient {
    pub(crate) fn new(base_url: &str) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            bail!("API URL must not be empty");
        }
        Url::parse(&base_url).with_context(|| format!("Failed to parse API URL: {base_url}"))?;

        Ok(Self { base_url, client: Client::new() })
    }

    fn asset_url(&self, key: &str) -> Result<String> {
        if key.is_empty() || key.starts_with('/') {
            bail!("Invalid asset key: {key}");
        }

        Ok(format!("{}/assets/{key}", self.base_url))
    }

    fn worker_task_claim_url(&self, task_stream: &str) -> Result<String> {
        if task_stream.is_empty() || task_stream.starts_with('/') {
            bail!("Invalid task stream: {task_stream}");
        }

        Ok(format!("{}/worker/gpu/tasks/claim/{task_stream}", self.base_url))
    }

    fn worker_task_url(&self, job_id: &Uuid, task_id: &str, action: &str) -> Result<String> {
        if task_id.is_empty() || task_id.starts_with('/') {
            bail!("Invalid task id: {task_id}");
        }
        if action.is_empty() || action.starts_with('/') {
            bail!("Invalid task action: {action}");
        }

        Ok(format!("{}/worker/gpu/tasks/{job_id}/{task_id}/{action}", self.base_url))
    }

    fn worker_hot_url(&self, key: &str) -> Result<String> {
        if key.is_empty() || key.starts_with('/') {
            bail!("Invalid hot-store key: {key}");
        }

        Ok(format!("{}/worker/hot/{key}", self.base_url))
    }

    pub(crate) async fn read_asset_buf(&self, key: &str) -> Result<Vec<u8>> {
        let url = self.asset_url(key)?;
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to request asset {key} from {url}"))?;

        let response = response.error_for_status().map_err(|err| {
            anyhow!(err).context(format!("Asset request failed for key {key} at {url}"))
        })?;

        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("Failed to read asset body for key {key} from {url}"))?;
        Ok(bytes.to_vec())
    }

    pub(crate) async fn read_asset<T>(&self, key: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let bytes = self.read_asset_buf(key).await?;
        bincode::deserialize(&bytes)
            .with_context(|| format!("Failed to deserialize asset payload for key {key}"))
    }

    pub(crate) async fn claim_gpu_work(
        &self,
        task_stream: &str,
        wait_timeout_secs: u64,
    ) -> Result<Option<ReadyTask>> {
        let url = self.worker_task_claim_url(task_stream)?;
        let response = self
            .client
            .post(&url)
            .query(&WorkerClaimQuery { wait_timeout_secs })
            .send()
            .await
            .with_context(|| format!("Failed to claim GPU work for stream {task_stream}"))?;
        let response = response.error_for_status().map_err(|err| {
            anyhow!(err).context(format!("GPU work claim failed for stream {task_stream} at {url}"))
        })?;
        let task = response
            .json::<Option<WorkerTask>>()
            .await
            .with_context(|| format!("Failed to decode GPU work claim response from {url}"))?;
        task.map(WorkerTask::into_ready_task).transpose()
    }

    pub(crate) async fn update_task_done(
        &self,
        job_id: &Uuid,
        task_id: &str,
        output: Value,
    ) -> Result<bool> {
        let url = self.worker_task_url(job_id, task_id, "done")?;
        let response = self
            .client
            .post(&url)
            .json(&TaskDoneReq { output })
            .send()
            .await
            .with_context(|| format!("Failed to mark task {job_id}:{task_id} done"))?;
        let response = response.error_for_status().map_err(|err| {
            anyhow!(err).context(format!("Task done update failed for {job_id}:{task_id}"))
        })?;
        Ok(response
            .json::<TaskUpdateRes>()
            .await
            .with_context(|| format!("Failed to decode task done response for {job_id}:{task_id}"))?
            .updated)
    }

    pub(crate) async fn update_task_failed(
        &self,
        job_id: &Uuid,
        task_id: &str,
        error: &str,
    ) -> Result<bool> {
        let url = self.worker_task_url(job_id, task_id, "failed")?;
        let response = self
            .client
            .post(&url)
            .json(&TaskFailedReq { error })
            .send()
            .await
            .with_context(|| format!("Failed to mark task {job_id}:{task_id} failed"))?;
        let response = response.error_for_status().map_err(|err| {
            anyhow!(err).context(format!("Task failed update failed for {job_id}:{task_id}"))
        })?;
        Ok(response
            .json::<TaskUpdateRes>()
            .await
            .with_context(|| {
                format!("Failed to decode task failure response for {job_id}:{task_id}")
            })?
            .updated)
    }

    pub(crate) async fn update_task_retry(&self, job_id: &Uuid, task_id: &str) -> Result<bool> {
        let url = self.worker_task_url(job_id, task_id, "retry")?;
        let response = self
            .client
            .post(&url)
            .send()
            .await
            .with_context(|| format!("Failed to retry task {job_id}:{task_id}"))?;
        let response = response.error_for_status().map_err(|err| {
            anyhow!(err).context(format!("Task retry update failed for {job_id}:{task_id}"))
        })?;
        Ok(response
            .json::<TaskUpdateRes>()
            .await
            .with_context(|| {
                format!("Failed to decode task retry response for {job_id}:{task_id}")
            })?
            .updated)
    }

    pub(crate) async fn get_task_retries_running(
        &self,
        job_id: &Uuid,
        task_id: &str,
    ) -> Result<Option<i32>> {
        let url = self.worker_task_url(job_id, task_id, "retries-running")?;
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch retries for task {job_id}:{task_id}"))?;
        let response = response.error_for_status().map_err(|err| {
            anyhow!(err).context(format!("Task retries fetch failed for {job_id}:{task_id}"))
        })?;
        Ok(response
            .json::<TaskRetriesRunningRes>()
            .await
            .with_context(|| {
                format!("Failed to decode retries-running response for {job_id}:{task_id}")
            })?
            .retries)
    }

    pub(crate) async fn hot_get_bytes(&self, key: &str) -> Result<Vec<u8>> {
        let url = self.worker_hot_url(key)?;
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch hot-store key {key}"))?;
        let response = response.error_for_status().map_err(|err| {
            anyhow!(err).context(format!("Hot-store fetch failed for key {key} at {url}"))
        })?;
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("Failed to read hot-store body for key {key}"))?;
        Ok(bytes.to_vec())
    }

    pub(crate) async fn hot_set_bytes(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl_secs: Option<u64>,
    ) -> Result<()> {
        let url = self.worker_hot_url(key)?;
        let request = self.client.put(&url);
        let request = if let Some(ttl_secs) = ttl_secs {
            request.query(&[("ttl_secs", ttl_secs)])
        } else {
            request
        };

        request
            .body(value)
            .send()
            .await
            .with_context(|| format!("Failed to write hot-store key {key}"))?
            .error_for_status()
            .map_err(|err| anyhow!(err).context(format!("Hot-store write failed for key {key}")))?;
        Ok(())
    }

    pub(crate) async fn hot_delete(&self, key: &str) -> Result<()> {
        let url = self.worker_hot_url(key)?;
        self.client
            .delete(&url)
            .send()
            .await
            .with_context(|| format!("Failed to delete hot-store key {key}"))?
            .error_for_status()
            .map_err(|err| {
                anyhow!(err).context(format!("Hot-store delete failed for key {key}"))
            })?;
        Ok(())
    }
}
