use std::{path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::os::unix::fs::PermissionsExt;

use std::result::Result;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::core::{self, error::MonorailError};

#[derive(Debug, Serialize)]
pub(crate) struct TargetShowInput {
    pub(crate) show_target_groups: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct TargetShowOutput {
    targets: Vec<core::Target>,
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
