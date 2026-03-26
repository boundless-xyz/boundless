// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, bail};
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

/// Object store client backed by the local filesystem.
pub struct S3Client {
    root: PathBuf,
}

impl S3Client {
    /// Initialize an object-store client backed by the local filesystem.
    pub fn from_local_dir(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root)
            .with_context(|| format!("Failed to create local storage root: {}", root.display()))?;
        Ok(Self { root: root.to_path_buf() })
    }

    fn local_object_path(&self, key: &str) -> Result<PathBuf> {
        let path = Path::new(key);
        if key.is_empty() || path.is_absolute() {
            bail!("Invalid object key: {key}");
        }
        for component in path.components() {
            if !matches!(component, Component::Normal(_)) {
                bail!("Invalid object key: {key}");
            }
        }
        Ok(self.root.join(path))
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

    fn write_local_bytes_atomic(&self, path: &Path, bytes: &[u8]) -> Result<()> {
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
        let path = self.local_object_path(key)?;
        fs::read(&path).with_context(|| {
            format!("Failed to read local object for key {key} at {}", path.display())
        })
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
        let path = self.local_object_path(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for key {key}: {}", parent.display())
            })?;
        }
        self.write_local_bytes_atomic(&path, &bytes).with_context(|| {
            format!("Failed to write local object for key {key} at {}", path.display())
        })
    }

    /// Writes a file from disk into the object store.
    pub async fn write_file_to_s3(&self, key: &str, in_path: &Path) -> Result<()> {
        let path = self.local_object_path(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for key {key}: {}", parent.display())
            })?;
        }
        Self::copy_local_file_atomic(in_path, &path).with_context(|| {
            format!(
                "Failed to copy local object for key {key} from {} to {}",
                in_path.display(),
                path.display()
            )
        })
    }

    /// Returns true if the object exists.
    pub async fn object_exists(&self, key: &str) -> Result<bool> {
        let path = self.local_object_path(key)?;
        Ok(path.is_file())
    }

    /// List objects with optional prefix.
    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<String>> {
        let mut objects = Vec::new();
        if self.root.exists() {
            Self::collect_local_objects(&self.root, &self.root, &mut objects)?;
        }
        if let Some(prefix) = prefix {
            objects.retain(|key| key.starts_with(prefix));
        }
        objects.sort();
        Ok(objects)
    }
}
