// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, bail};
use aws_sdk_s3::{
    Client,
    config::{Builder, Credentials, Region},
    error::ProvideErrorMetadata,
    operation::{create_bucket::CreateBucketError, head_object::HeadObjectError},
    primitives::ByteStream,
    types::CreateBucketConfiguration,
};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
};
use tempfile::NamedTempFile;

/// Object store elf dir
pub const ELF_BUCKET_DIR: &str = "elfs";

/// Object store input dir
pub const INPUT_BUCKET_DIR: &str = "inputs";

/// Guest executor logs dir
pub const EXEC_LOGS_BUCKET_DIR: &str = "exec_logs";

/// Object store receipts dir
pub const RECEIPT_BUCKET_DIR: &str = "receipts";

/// Object store preflight journals dir
pub const PREFLIGHT_JOURNALS_BUCKET_DIR: &str = "preflight_journals";

/// Object store stark receipt dir
pub const STARK_BUCKET_DIR: &str = "stark";

/// Object store receipts groth16 dir
pub const GROTH16_BUCKET_DIR: &str = "groth16";

/// Object store receipts blake3_groth16 dir
pub const BLAKE3_GROTH16_BUCKET_DIR: &str = "blake3_groth16";

/// Object store work receipts dir
pub const WORK_RECEIPTS_BUCKET_DIR: &str = "work_receipts";

enum StorageBackend {
    S3 { bucket: String, client: Client },
    Local { root: PathBuf },
}

/// Object store client object.
/// This supports either S3/MinIO or a local filesystem backend.
pub struct S3Client {
    backend: StorageBackend,
}

impl S3Client {
    /// Initialize an object-store client from a MinIO config.
    pub async fn from_minio(
        url: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
    ) -> Result<Self> {
        let cred = Credentials::new(access_key, secret_key, None, None, "loaded-from-custom-env");

        let s3_config = Builder::new()
            .endpoint_url(url)
            .credentials_provider(cred)
            .behavior_version_latest()
            .region(Region::new(region.to_string()))
            .force_path_style(true)
            .build();

        let client = aws_sdk_s3::Client::from_conf(s3_config);

        // Check if bucket exists first - only create if we're certain it doesn't exist
        let bucket_exists = match client.head_bucket().bucket(bucket).send().await {
            Ok(_) => true,
            Err(err) => {
                // Check if it's a NotFound error (bucket doesn't exist)
                // Check both the error code and HTTP status code
                let is_not_found = if let Some(service_err) = err.as_service_error() {
                    service_err.code() == Some("NotFound") || service_err.code() == Some("404")
                } else {
                    false
                };

                // Also check the raw response status code if available
                let status_404 = err
                    .raw_response()
                    .and_then(|r| Some(r.status().as_u16() == 404))
                    .unwrap_or(false);

                // Only if we're certain it's a 404/NotFound, the bucket doesn't exist
                // Otherwise, assume it exists (or we can't determine) and don't try to create
                !(is_not_found || status_404)
            }
        };

        // Only attempt to create the bucket if we're certain it doesn't exist
        if !bucket_exists {
            let cfg =
                CreateBucketConfiguration::builder().location_constraint(region.into()).build();
            let res =
                client.create_bucket().create_bucket_configuration(cfg).bucket(bucket).send().await;

            if let Err(err) = res {
                let Some(service_err) = err.as_service_error() else {
                    bail!(format!("Minio SDK error: {err:?}"));
                };
                match service_err {
                    CreateBucketError::BucketAlreadyOwnedByYou(_) => {
                        // Bucket was created by another process between check and create
                    }
                    _ => {
                        // Check if it's an OperationAborted error (transient conflict)
                        if service_err.code() == Some("OperationAborted") {
                            // Another process is creating the bucket - this is fine, assume success
                        } else {
                            bail!(format!("Failed to create bucket: {err:?}"));
                        }
                    }
                }
            }
        }

        Ok(Self { backend: StorageBackend::S3 { bucket: bucket.to_string(), client } })
    }

    /// Initialize an object-store client backed by the local filesystem.
    pub fn from_local_dir(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root)
            .with_context(|| format!("Failed to create local storage root: {}", root.display()))?;

        Ok(Self { backend: StorageBackend::Local { root: root.to_path_buf() } })
    }

    fn local_object_path(root: &Path, key: &str) -> Result<PathBuf> {
        let path = Path::new(key);
        if key.is_empty() || path.is_absolute() {
            bail!("Invalid object key: {key}");
        }

        for component in path.components() {
            if !matches!(component, Component::Normal(_)) {
                bail!("Invalid object key: {key}");
            }
        }

        Ok(root.join(path))
    }

    fn collect_local_objects(root: &Path, dir: &Path, objects: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(dir)
            .with_context(|| format!("Failed to list local storage path: {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::collect_local_objects(root, &path, objects)?;
                continue;
            }

            if path.is_file() {
                let relative = path.strip_prefix(root).with_context(|| {
                    format!(
                        "Failed to strip local storage root {} from path {}",
                        root.display(),
                        path.display()
                    )
                })?;
                objects.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }

        Ok(())
    }

    fn persist_local_tempfile(temp: NamedTempFile, path: &Path) -> Result<()> {
        match temp.persist(path) {
            Ok(_) => Ok(()),
            Err(err) => {
                let tempfile::PersistError { error, file } = err;
                let can_replace_existing =
                    matches!(error.kind(), ErrorKind::AlreadyExists | ErrorKind::PermissionDenied)
                        && path.exists();
                if !can_replace_existing {
                    return Err(error).with_context(|| {
                        format!("Failed to atomically publish local object at {}", path.display())
                    });
                }

                fs::remove_file(path).with_context(|| {
                    format!("Failed to replace existing local object at {}", path.display())
                })?;
                file.persist(path).map_err(|persist_err| persist_err.error).with_context(|| {
                    format!("Failed to atomically publish local object at {}", path.display())
                })?;
                Ok(())
            }
        }
    }

    fn write_local_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path.parent().context("Local object path is missing a parent directory")?;
        let mut temp =
            tempfile::Builder::new().prefix(".tmp-object-").tempfile_in(parent).with_context(
                || format!("Failed to create temporary local object file in {}", parent.display()),
            )?;
        temp.write_all(bytes).with_context(|| {
            format!("Failed to write temporary local object file for {}", path.display())
        })?;
        temp.as_file().sync_all().with_context(|| {
            format!("Failed to sync temporary local object file for {}", path.display())
        })?;
        Self::persist_local_tempfile(temp, path)
    }

    fn copy_local_file_atomic(in_path: &Path, path: &Path) -> Result<()> {
        let parent = path.parent().context("Local object path is missing a parent directory")?;
        let temp =
            tempfile::Builder::new().prefix(".tmp-object-").tempfile_in(parent).with_context(
                || format!("Failed to create temporary local object file in {}", parent.display()),
            )?;
        fs::copy(in_path, temp.path()).with_context(|| {
            format!(
                "Failed to copy local object from {} to temporary path for {}",
                in_path.display(),
                path.display()
            )
        })?;
        temp.as_file().sync_all().with_context(|| {
            format!("Failed to sync temporary local object file for {}", path.display())
        })?;
        Self::persist_local_tempfile(temp, path)
    }

    /// Reads an object encoded with bincode.
    pub async fn read_from_s3<T>(&self, key: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let encoded = self.read_buf_from_s3(key).await?;
        bincode::deserialize(&encoded).context("Failed to deserialize s3 object")
    }

    /// Reads an object to byte buffer.
    pub async fn read_buf_from_s3(&self, key: &str) -> Result<Vec<u8>> {
        match &self.backend {
            StorageBackend::S3 { bucket, client } => {
                let result = client.get_object().bucket(bucket).key(key).send().await?;
                Ok(result.body.collect().await?.to_vec())
            }
            StorageBackend::Local { root } => {
                let path = Self::local_object_path(root, key)?;
                fs::read(&path).with_context(|| {
                    format!("Failed to read local object for key {key} at {}", path.display())
                })
            }
        }
    }

    /// Writes a bincode serializable object.
    pub async fn write_to_s3<T>(&self, key: &str, obj: T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let bytes = bincode::serialize(&obj)?;
        self.write_buf_to_s3(key, bytes).await
    }

    /// Writes a byte buffer object.
    pub async fn write_buf_to_s3(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        match &self.backend {
            StorageBackend::S3 { bucket, client } => {
                let data_stream = ByteStream::from(bytes);
                client.put_object().bucket(bucket).key(key).body(data_stream).send().await?;
                Ok(())
            }
            StorageBackend::Local { root } => {
                let path = Self::local_object_path(root, key)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!(
                            "Failed to create parent directory for key {key}: {}",
                            parent.display()
                        )
                    })?;
                }
                Self::write_local_bytes_atomic(&path, &bytes).with_context(|| {
                    format!("Failed to write local object for key {key} at {}", path.display())
                })?;
                Ok(())
            }
        }
    }

    pub async fn write_file_to_s3(&self, key: &str, in_path: &Path) -> Result<()> {
        match &self.backend {
            StorageBackend::S3 { bucket, client } => {
                let data_stream = ByteStream::read_from().path(in_path).build().await?;
                client.put_object().bucket(bucket).key(key).body(data_stream).send().await?;
                Ok(())
            }
            StorageBackend::Local { root } => {
                let path = Self::local_object_path(root, key)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!(
                            "Failed to create parent directory for key {key}: {}",
                            parent.display()
                        )
                    })?;
                }
                Self::copy_local_file_atomic(in_path, &path).with_context(|| {
                    format!(
                        "Failed to copy local object for key {key} from {} to {}",
                        in_path.display(),
                        path.display()
                    )
                })?;
                Ok(())
            }
        }
    }

    pub async fn object_exists(&self, key: &str) -> Result<bool> {
        match &self.backend {
            StorageBackend::S3 { bucket, client } => {
                match client.head_bucket().bucket(bucket).send().await {
                    Ok(_) => match client.head_object().bucket(bucket).key(key).send().await {
                        Ok(_) => Ok(true),
                        Err(err) => match err.into_service_error() {
                            HeadObjectError::NotFound(_) => Ok(false),
                            err => Err(err.into()),
                        },
                    },
                    Err(err) => Err(err.into()),
                }
            }
            StorageBackend::Local { root } => {
                let path = Self::local_object_path(root, key)?;
                Ok(path.is_file())
            }
        }
    }

    /// List objects with optional prefix.
    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<String>> {
        match &self.backend {
            StorageBackend::S3 { bucket, client } => {
                let mut objects = Vec::new();
                let mut continuation_token: Option<String> = None;

                loop {
                    let mut request = client.list_objects_v2().bucket(bucket);

                    if let Some(prefix) = prefix {
                        request = request.prefix(prefix);
                    }

                    if let Some(token) = continuation_token.as_deref() {
                        request = request.continuation_token(token);
                    }

                    let response = request.send().await?;

                    for object in response.contents() {
                        if let Some(key) = object.key() {
                            objects.push(key.to_string());
                        }
                    }

                    continuation_token =
                        response.next_continuation_token().map(std::string::ToString::to_string);
                    if continuation_token.is_none() {
                        break;
                    }
                }

                Ok(objects)
            }
            StorageBackend::Local { root } => {
                let mut objects = Vec::new();
                if root.exists() {
                    Self::collect_local_objects(root, root, &mut objects)?;
                }

                if let Some(prefix) = prefix {
                    objects.retain(|key| key.starts_with(prefix));
                }

                objects.sort();
                Ok(objects)
            }
        }
    }
}
