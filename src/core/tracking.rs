use crate::common::error::MonorailError;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path;
use std::result::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct LogInfo {
    #[serde(skip)]
    pub(crate) path: path::PathBuf,
    pub(crate) id: usize,
}
impl LogInfo {
    pub(crate) fn new(file_path: &path::Path) -> Self {
        Self {
            path: file_path.to_path_buf(),
            id: 0,
        }
    }
    // Open the internal file and read its contents.
    pub(crate) async fn open(file_path: &path::Path) -> Result<Self, MonorailError> {
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(file_path)
            .await
            .map_err(MonorailError::TrackingLogInfoNotFound)?;
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
            .map_err(MonorailError::TrackingCheckpointNotFound)?;
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
    log_info_path: path::PathBuf,
    checkpoint_path: path::PathBuf,
}
impl<'a> Table {
    // Prepare the tracking directory and return the table ready for use.
    pub(crate) fn new(dir_path: &'a path::Path) -> Result<Self, MonorailError> {
        std::fs::create_dir_all(dir_path)?;
        Ok(Self {
            log_info_path: dir_path.join("log_info.json"),
            checkpoint_path: dir_path.join("checkpoint.json"),
        })
    }
    pub(crate) fn new_checkpoint(&'a self) -> Checkpoint {
        Checkpoint::new(&self.checkpoint_path)
    }
    pub(crate) async fn open_checkpoint(&'a self) -> Result<Checkpoint, MonorailError> {
        Checkpoint::open(&self.checkpoint_path).await
    }
    pub(crate) fn new_log_info(&'a self) -> LogInfo {
        LogInfo::new(&self.log_info_path)
    }
    pub(crate) async fn open_log_info(&'a self) -> Result<LogInfo, MonorailError> {
        LogInfo::open(&self.log_info_path).await
    }
}
