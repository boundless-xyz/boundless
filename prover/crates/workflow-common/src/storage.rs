// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
};
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;

/// Shared storage elf dir
pub const ELF_BUCKET_DIR: &str = "elfs";

/// Shared storage input dir
pub const INPUT_BUCKET_DIR: &str = "inputs";

/// Guest executor logs dir
pub const EXEC_LOGS_BUCKET_DIR: &str = "exec_logs";

/// Shared storage receipts dir
pub const RECEIPT_BUCKET_DIR: &str = "receipts";

/// Shared storage preflight journals dir
pub const PREFLIGHT_JOURNALS_BUCKET_DIR: &str = "preflight_journals";

/// Shared storage stark receipt dir
pub const STARK_BUCKET_DIR: &str = "stark";

/// Shared storage groth16 receipt dir
pub const GROTH16_BUCKET_DIR: &str = "groth16";

/// Shared storage blake3_groth16 receipt dir
pub const BLAKE3_GROTH16_BUCKET_DIR: &str = "blake3_groth16";

/// Shared storage work receipts dir
pub const WORK_RECEIPTS_BUCKET_DIR: &str = "work_receipts";

/// Shared filesystem storage client.
pub struct SharedFs {
    root: PathBuf,
}

impl SharedFs {
    /// Initialize the shared filesystem storage root.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root)
            .with_context(|| format!("Failed to create shared storage root: {}", root.display()))?;
        Ok(Self { root: root.to_path_buf() })
    }

    fn object_path(&self, key: &str) -> Result<PathBuf> {
        let path = Path::new(key);
        if key.is_empty() || path.is_absolute() {
            bail!("Invalid storage key: {key}");
        }
        for component in path.components() {
            if !matches!(component, Component::Normal(_)) {
                bail!("Invalid storage key: {key}");
            }
        }
        Ok(self.root.join(path))
    }

    fn collect_objects(root: &Path, dir: &Path, objects: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(dir)
            .with_context(|| format!("Failed to list shared storage path: {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::collect_objects(root, &path, objects)?;
                continue;
            }
            if path.is_file() {
                let relative = path.strip_prefix(root).with_context(|| {
                    format!(
                        "Failed to strip shared storage root {} from path {}",
                        root.display(),
                        path.display()
                    )
                })?;
                objects.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }

    fn persist_tempfile(temp: NamedTempFile, path: &Path) -> Result<()> {
        match temp.persist(path) {
            Ok(_) => Ok(()),
            Err(err) => {
                let tempfile::PersistError { error, file } = err;
                let can_replace_existing =
                    matches!(error.kind(), ErrorKind::AlreadyExists | ErrorKind::PermissionDenied)
                        && path.exists();
                if !can_replace_existing {
                    return Err(error).with_context(|| {
                        format!(
                            "Failed to atomically publish shared storage object at {}",
                            path.display()
                        )
                    });
                }
                fs::remove_file(path).with_context(|| {
                    format!(
                        "Failed to replace existing shared storage object at {}",
                        path.display()
                    )
                })?;
                file.persist(path).map_err(|persist_err| persist_err.error).with_context(|| {
                    format!(
                        "Failed to atomically publish shared storage object at {}",
                        path.display()
                    )
                })?;
                Ok(())
            }
        }
    }

    fn write_bytes_atomic(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path.parent().context("Shared storage path is missing a parent directory")?;
        let mut temp = tempfile::Builder::new()
            .prefix(".tmp-object-")
            .tempfile_in(parent)
            .with_context(|| {
                format!("Failed to create temporary shared storage file in {}", parent.display())
            })?;
        temp.write_all(bytes).with_context(|| {
            format!("Failed to write temporary shared storage file for {}", path.display())
        })?;
        temp.as_file().sync_all().with_context(|| {
            format!("Failed to sync temporary shared storage file for {}", path.display())
        })?;
        Self::persist_tempfile(temp, path)
    }

    fn copy_file_atomic(in_path: &Path, path: &Path) -> Result<()> {
        let parent = path.parent().context("Shared storage path is missing a parent directory")?;
        let temp = tempfile::Builder::new()
            .prefix(".tmp-object-")
            .tempfile_in(parent)
            .with_context(|| {
                format!("Failed to create temporary shared storage file in {}", parent.display())
            })?;
        fs::copy(in_path, temp.path()).with_context(|| {
            format!(
                "Failed to copy shared storage file from {} to temporary path for {}",
                in_path.display(),
                path.display()
            )
        })?;
        temp.as_file().sync_all().with_context(|| {
            format!("Failed to sync temporary shared storage file for {}", path.display())
        })?;
        Self::persist_tempfile(temp, path)
    }

    fn create_tempfile(path: &Path) -> Result<NamedTempFile> {
        let parent = path.parent().context("Shared storage path is missing a parent directory")?;
        tempfile::Builder::new().prefix(".tmp-object-").tempfile_in(parent).with_context(|| {
            format!("Failed to create temporary shared storage file in {}", parent.display())
        })
    }

    /// Reads a bincode-encoded object.
    pub async fn read_object<T>(&self, key: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let encoded = self.read_bytes(key).await?;
        bincode::deserialize(&encoded).context("Failed to deserialize shared storage object")
    }

    /// Reads raw bytes from shared storage.
    pub async fn read_bytes(&self, key: &str) -> Result<Vec<u8>> {
        let path = self.object_path(key)?;
        fs::read(&path).with_context(|| {
            format!("Failed to read shared storage key {key} at {}", path.display())
        })
    }

    /// Writes a bincode-serializable object.
    pub async fn write_object<T>(&self, key: &str, obj: T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let bytes = bincode::serialize(&obj)?;
        self.write_bytes(key, bytes).await
    }

    /// Writes raw bytes to shared storage.
    pub async fn write_bytes(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        let path = self.object_path(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for key {key}: {}", parent.display())
            })?;
        }
        self.write_bytes_atomic(&path, &bytes).with_context(|| {
            format!("Failed to write shared storage key {key} at {}", path.display())
        })
    }

    /// Streams content into shared storage.
    pub async fn write_stream<S, B, E>(&self, key: &str, mut stream: S) -> Result<()>
    where
        S: futures_util::stream::Stream<Item = std::result::Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let path = self.object_path(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for key {key}: {}", parent.display())
            })?;
        }

        let temp = Self::create_tempfile(&path)?;
        let std_file = temp.reopen().with_context(|| {
            format!("Failed to reopen temporary shared storage file for {}", path.display())
        })?;
        let mut file = tokio::fs::File::from_std(std_file);

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| {
                format!(
                    "Failed to read streamed shared storage data for key {key} at {}",
                    path.display()
                )
            })?;
            file.write_all(chunk.as_ref()).await.with_context(|| {
                format!(
                    "Failed to write streamed shared storage data for key {key} at {}",
                    path.display()
                )
            })?;
        }

        file.sync_all().await.with_context(|| {
            format!(
                "Failed to sync streamed shared storage data for key {key} at {}",
                path.display()
            )
        })?;
        drop(file);

        Self::persist_tempfile(temp, &path).with_context(|| {
            format!("Failed to publish streamed shared storage key {key} at {}", path.display())
        })
    }

    /// Copies a file from disk into shared storage.
    pub async fn write_file(&self, key: &str, in_path: &Path) -> Result<()> {
        let path = self.object_path(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for key {key}: {}", parent.display())
            })?;
        }
        Self::copy_file_atomic(in_path, &path).with_context(|| {
            format!(
                "Failed to copy shared storage key {key} from {} to {}",
                in_path.display(),
                path.display()
            )
        })
    }

    /// Returns true if the object exists.
    pub async fn object_exists(&self, key: &str) -> Result<bool> {
        let path = self.object_path(key)?;
        Ok(path.is_file())
    }

    /// List objects with optional prefix.
    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<String>> {
        let mut objects = Vec::new();
        if self.root.exists() {
            Self::collect_objects(&self.root, &self.root, &mut objects)?;
        }
        if let Some(prefix) = prefix {
            objects.retain(|key| key.starts_with(prefix));
        }
        objects.sort();
        Ok(objects)
    }
}
