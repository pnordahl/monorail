use crate::core::error::MonorailError;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::result::Result;
use std::{fs, io, path};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Run {
    #[serde(skip)]
    pub(crate) path: path::PathBuf,
    pub(crate) id: usize,
}
impl Run {
    pub(crate) fn new(file_path: &path::Path) -> Self {
        Self {
            path: file_path.to_path_buf(),
            id: 0,
        }
    }

    // Open the internal file and read its contents.
    pub(crate) fn open(file_path: &path::Path) -> Result<Self, MonorailError> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .open(file_path)
            .map_err(MonorailError::TrackingRunNotFound)?;
        let mut data = vec![];
        file.read_to_end(&mut data)?;
        let mut cp: Self = serde_json::from_slice(&data)?;
        cp.path = file_path.to_path_buf();
        Ok(cp)
    }

    // Copy all current state into the file.
    pub(crate) fn save(&mut self) -> Result<(), MonorailError> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&self.path)?;

        let data = serde_json::to_vec(self)?;
        file.write_all(&data)?;
        Ok(())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Checkpoint {
    #[serde(skip)]
    pub(crate) path: path::PathBuf,
    pub(crate) id: String,
    pub(crate) pending: Option<HashMap<String, String>>,
}
impl Checkpoint {
    pub(crate) fn new(file_path: &path::Path) -> Self {
        Self {
            path: file_path.to_path_buf(),
            id: String::new(),
            pending: None,
        }
    }
    // Open the internal file and read its contents.
    pub(crate) fn open(file_path: &path::Path) -> Result<Self, MonorailError> {
        let file = fs::OpenOptions::new()
            .read(true)
            .open(file_path)
            .map_err(MonorailError::TrackingCheckpointNotFound)?;
        let br = io::BufReader::new(file);
        let mut decoder = zstd::stream::read::Decoder::new(br)?;
        let mut cp: Checkpoint = serde_json::from_reader(&mut decoder)?;
        cp.path = file_path.to_path_buf();
        Ok(cp)
    }

    // Copy all current state into the file.
    pub(crate) fn save(&mut self) -> Result<(), MonorailError> {
        let file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&self.path)?;
        let bw = io::BufWriter::new(file);
        let mut encoder = zstd::stream::write::Encoder::new(bw, 3)?;
        serde_json::to_writer(&mut encoder, self)?;
        encoder.finish()?;
        Ok(())
    }
}
#[derive(Debug)]
pub(crate) struct Table {
    run_path: path::PathBuf,
    checkpoint_path: path::PathBuf,
    generated_config_checksum_path: path::PathBuf,
}
impl<'a> Table {
    // Prepare the tracking directory and return the table ready for use.
    pub(crate) fn new(dir_path: &'a path::Path) -> Result<Self, MonorailError> {
        std::fs::create_dir_all(dir_path)?;
        Ok(Self {
            run_path: dir_path.join("run.json"),
            checkpoint_path: dir_path.join("checkpoint.json.zst"),
            generated_config_checksum_path: dir_path.join("generated_config_checksum"),
        })
    }
    pub(crate) fn new_checkpoint(&'a self) -> Checkpoint {
        Checkpoint::new(&self.checkpoint_path)
    }
    pub(crate) fn open_checkpoint(&'a self) -> Result<Checkpoint, MonorailError> {
        Checkpoint::open(&self.checkpoint_path)
    }
    pub(crate) fn new_run(&'a self) -> Run {
        Run::new(&self.run_path)
    }
    pub(crate) fn open_run(&'a self) -> Result<Run, MonorailError> {
        Run::open(&self.run_path)
    }
    pub(crate) fn get_generated_config_checksum(&'a self) -> Result<String, MonorailError> {
        fs::read_to_string(&self.generated_config_checksum_path).map_err(|e| {
            MonorailError::Generic(format!(
                "Failed to read generated checksum at path {}, error: {}",
                &self.generated_config_checksum_path.display(),
                e
            ))
        })
    }
    pub(crate) fn save_generated_config_checksum(
        &'a self,
        checksum: String,
    ) -> Result<(), MonorailError> {
        fs::write(&self.generated_config_checksum_path, checksum).map_err(MonorailError::from)
    }
}
