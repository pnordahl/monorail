use std::collections::HashMap;
use std::path;
use std::result::Result;

use serde::Serialize;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::git;
    use crate::core::testing::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_handle_checkpoint_delete_success() {
        let td2 = tempdir().unwrap();
        let rp = &td2.path();
        let cfg = new_test_repo2(rp).await;
        let tracking_path = cfg.get_tracking_path(rp);
        let tt = tracking::Table::new(&tracking_path).unwrap();
        let mut cp = tt.new_checkpoint();
        cp.save().unwrap();

        assert!(cp.path.exists(), "Checkpoint should exist");

        // Run the function
        let result = handle_checkpoint_delete(&cfg, rp).await;

        // Check the output and file deletion
        assert!(result.is_ok());
        assert!(!cp.path.exists(), "Checkpoint file should be deleted");
    }

    #[tokio::test]
    async fn test_handle_checkpoint_show_success() {
        let td2 = tempdir().unwrap();
        let rp = &td2.path();
        let cfg = new_test_repo2(rp).await;
        let tracking_path = cfg.get_tracking_path(rp);
        let tt = tracking::Table::new(&tracking_path).unwrap();
        let mut cp = tt.new_checkpoint();
        cp.id = "test_id".to_string();
        cp.save().unwrap();

        assert!(cp.path.exists(), "Checkpoint should exist");

        let result = handle_checkpoint_show(&cfg, rp).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.checkpoint.id, "test_id");
    }

    #[tokio::test]
    async fn test_handle_checkpoint_update_with_pending_changes() {
        let td2 = tempdir().unwrap();
        let rp = &td2.path();
        let cfg = new_test_repo2(rp).await;
        let head = get_head(rp).await;
        let git_opts: git::GitOptions = Default::default();

        let input = CheckpointUpdateInput {
            id: None,
            pending: true,
            git_opts: git_opts,
        };

        let result = handle_checkpoint_update(&cfg, &input, rp).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            // number of files in the test config
            output.checkpoint.pending.unwrap().len() == 17,
            "Pending changes should be populated"
        );
        assert_eq!(output.checkpoint.id, head);
    }

    #[tokio::test]
    async fn test_handle_checkpoint_update_no_pending_changes() {
        let td2 = tempdir().unwrap();
        let rp = &td2.path();
        let cfg = new_test_repo2(rp).await;
        let git_opts: git::GitOptions = Default::default();

        let input = CheckpointUpdateInput {
            id: Some("test_id"),
            pending: false,
            git_opts: git_opts,
        };

        let result = handle_checkpoint_update(&cfg, &input, rp).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.checkpoint.id, "test_id");
        assert!(
            output.checkpoint.pending.is_none(),
            "Pending changes should be None when not requested"
        );
    }
}
