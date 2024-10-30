pub(crate) mod analyze;
pub(crate) mod log;
pub(crate) mod out;
pub(crate) mod run;

use std::{path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::os::unix::fs::PermissionsExt;

use std::result::Result;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tracing::{debug, error, info, instrument};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{fmt, Registry};

use crate::core::error::{GraphError, MonorailError};
use crate::core::{self, file, git, tracking, ChangeProviderKind, Target};

const RESULT_OUTPUT_FILE_NAME: &str = "result.json.zst";

// Custom formatter to match chrono's strict RFC3339 compliance.
struct UtcTimestampWithOffset;

impl fmt::time::FormatTime for UtcTimestampWithOffset {
    fn format_time(&self, w: &mut fmt::format::Writer<'_>) -> std::fmt::Result {
        let now = chrono::Utc::now();
        // Format the timestamp as "YYYY-MM-DDTHH:MM:SS.f+00:00"
        write!(w, "{}", now.format("%Y-%m-%dT%H:%M:%S%.f+00:00"))
    }
}

pub(crate) fn setup_tracing(format: &str, level: u8) -> Result<(), MonorailError> {
    let env_filter = EnvFilter::default();
    let level_filter = match level {
        0 => LevelFilter::OFF,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    let registry = Registry::default().with(env_filter.add_directive(level_filter.into()));
    match format {
        "json" => {
            let json_fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .with_target(false)
                .flatten_event(true)
                .with_timer(UtcTimestampWithOffset);
            tracing::subscriber::set_global_default(registry.with(json_fmt_layer))
                .map_err(|e| MonorailError::from(e.to_string()))?;
        }
        _ => {
            let plain_fmt_layer = tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_names(true)
                .with_timer(UtcTimestampWithOffset);
            tracing::subscriber::set_global_default(registry.with(plain_fmt_layer))
                .map_err(|e| MonorailError::from(e.to_string()))?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct ResultShowInput {}

pub(crate) fn result_show<'a>(
    cfg: &'a core::Config,
    work_path: &'a path::Path,
    _input: &'a ResultShowInput,
) -> Result<RunOutput, MonorailError> {
    // open tracking and get run
    let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    // use run to get results.json file in id dir
    let run = tracking_table.open_run()?;
    let log_dir = cfg.get_log_path(work_path).join(format!("{}", run.id));
    let run_output_file = std::fs::OpenOptions::new()
        .read(true)
        .open(log_dir.join(RESULT_OUTPUT_FILE_NAME))
        .map_err(|e| MonorailError::Generic(e.to_string()))?;
    let br = BufReader::new(run_output_file);
    let mut decoder = zstd::stream::read::Decoder::new(br)?;

    Ok(serde_json::from_reader(&mut decoder)?)
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetShowInput {
    pub(crate) show_target_groups: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct TargetShowOutput {
    targets: Vec<Target>,
    target_groups: Option<Vec<Vec<String>>>,
}

pub(crate) fn target_show(
    cfg: &core::Config,
    input: TargetShowInput,
    work_path: &path::Path,
) -> Result<TargetShowOutput, MonorailError> {
    let mut target_groups = None;
    if input.show_target_groups {
        let mut index = core::Index::new(cfg, &cfg.get_target_path_set(), work_path)?;
        target_groups = Some(index.dag.get_labeled_groups()?);
    }
    Ok(TargetShowOutput {
        targets: cfg.targets.clone(),
        target_groups,
    })
}

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
        ChangeProviderKind::Git => checkpoint_update_git(cfg, input, work_path).await,
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
