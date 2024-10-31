use std::{path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::os::unix::fs::PermissionsExt;

use std::result::Result;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::core::{self, error::MonorailError, file, git, tracking};

#[derive(Debug, Serialize)]
pub(crate) struct CheckpointDeleteOutput {
    checkpoint: tracking::Checkpoint,
}

pub(crate) async fn handle_checkpoint_delete(
    cfg: &core::Config,
    work_path: &path::Path,
) -> Result<CheckpointDeleteOutput, MonorailError> {
    let tracking = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    let mut checkpoint = tracking.open_checkpoint()?;
    checkpoint.id = "".to_string();
    checkpoint.pending = None;

    tokio::fs::remove_file(&checkpoint.path).await?;

    Ok(CheckpointDeleteOutput { checkpoint })
}

#[derive(Debug, Serialize)]
pub(crate) struct CheckpointShowOutput {
    checkpoint: tracking::Checkpoint,
}

pub(crate) async fn handle_checkpoint_show(
    cfg: &core::Config,
    work_path: &path::Path,
) -> Result<CheckpointShowOutput, MonorailError> {
    let tracking = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    Ok(CheckpointShowOutput {
        checkpoint: tracking.open_checkpoint()?,
    })
}

#[derive(Debug)]
pub(crate) struct CheckpointUpdateInput<'a> {
    pub(crate) id: Option<&'a str>,
    pub(crate) pending: bool,
    pub(crate) git_opts: git::GitOptions<'a>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CheckpointUpdateOutput {
    checkpoint: tracking::Checkpoint,
}

pub(crate) async fn handle_checkpoint_update(
    cfg: &core::Config,
    input: &CheckpointUpdateInput<'_>,
    work_path: &path::Path,
) -> Result<CheckpointUpdateOutput, MonorailError> {
    match cfg.change_provider.r#use {
        core::ChangeProviderKind::Git => checkpoint_update_git(cfg, input, work_path).await,
    }
}

async fn checkpoint_update_git<'a>(
    cfg: &core::Config,
    input: &CheckpointUpdateInput<'a>,
    work_path: &path::Path,
) -> Result<CheckpointUpdateOutput, MonorailError> {
    let tracking = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    let mut checkpoint = match tracking.open_checkpoint() {
        Ok(cp) => cp,
        Err(MonorailError::TrackingCheckpointNotFound(_)) => tracking.new_checkpoint(),
        // TODO: need to set path on checkpoint tho; don't use default
        Err(e) => {
            return Err(e);
        }
    };

    checkpoint.id = match input.id {
        Some(id) => id.to_string(),
        None => git::git_cmd_rev_parse(input.git_opts.git_path, work_path, "HEAD").await?,
    };

    if input.pending {
        // get all changes with no checkpoint, so diff will return [HEAD, staging area]
        let pending_changes = git::get_git_all_changes(&input.git_opts, &None, work_path).await?;
        if let Some(pending_changes) = pending_changes {
            if !pending_changes.is_empty() {
                let mut pending = HashMap::new();
                for change in pending_changes.iter() {
                    let p = work_path.join(&change.name);

                    pending.insert(change.name.clone(), file::get_file_checksum(&p).await?);
                }
                checkpoint.pending = Some(pending);
            }
        }
    }
    checkpoint.save()?;

    Ok(CheckpointUpdateOutput { checkpoint })
}
