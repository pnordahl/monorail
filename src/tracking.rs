use crate::error::MonorailError;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path;
use std::result::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Checkpoint {
    #[serde(skip)]
    pub(crate) path: path::PathBuf,
    pub(crate) commit: String,
    pub(crate) pending: Option<HashMap<String, String>>,
}
impl Checkpoint {
    pub(crate) fn new(file_path: &path::Path) -> Self {
        Self {
            path: file_path.to_path_buf(),
            commit: String::new(),
            pending: None,
        }
    }
    // Open the internal file and read its contents.
    pub(crate) async fn open(file_path: &path::Path) -> Result<Self, MonorailError> {
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(file_path)
            .await
            .map_err(MonorailError::CheckpointNotFound)?;
        let mut data = vec![];
        file.read_to_end(&mut data).await?;
        let mut cp: Self = serde_json::from_slice(&data)?;
        cp.path = file_path.to_path_buf();
        Ok(cp)
    }

    // Copy all current state into the file.
    pub(crate) async fn save(&mut self) -> Result<(), MonorailError> {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&self.path)
            .await?;

        let data = serde_json::to_vec(self)?;
        file.write_all(&data).await?;
        Ok(())
    }
}
#[derive(Debug)]
pub(crate) struct Table {
    checkpoint_path: path::PathBuf,
    lock_path: path::PathBuf,
    // privately held for the lifetime of the Table
    _lock_file: tokio::fs::File,
}
impl<'a> Table {
    // Open a lockfile and return the table ready for use.
    pub(crate) async fn open(dir_path: &'a path::Path) -> Result<Self, MonorailError> {
        std::fs::create_dir_all(dir_path)?;
        let lock_path = dir_path.join("LOCKFILE");
        let lock_file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .await?;
        Ok(Self {
            checkpoint_path: dir_path.join("checkpoint.json"),
            lock_path,
            _lock_file: lock_file,
        })
    }
    pub(crate) fn new_checkpoint(&'a self) -> Checkpoint {
        Checkpoint::new(&self.checkpoint_path)
    }
    pub(crate) async fn open_checkpoint(&'a self) -> Result<Checkpoint, MonorailError> {
        Checkpoint::open(&self.checkpoint_path).await
    }
}
impl Drop for Table {
    fn drop(&mut self) {
        std::fs::remove_file(&self.lock_path)
            .unwrap_or_else(|_| panic!("Error removing lockfile {}", self.lock_path.display()));
    }
}
