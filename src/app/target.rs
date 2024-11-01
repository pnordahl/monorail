use std::collections::{HashMap, HashSet};
use std::path;
use std::result::Result;

use serde::Serialize;

use crate::core::{self, error::MonorailError, file};

#[derive(Debug, Serialize)]
pub(crate) struct TargetShowInput {
    pub(crate) show_target_groups: bool,
    pub(crate) show_commands: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct TargetShowOutput<'a> {
    targets: Vec<AppTarget<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_groups: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AppTarget<'a> {
    pub(crate) path: &'a str,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uses: &'a Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ignores: &'a Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) commands: Option<HashMap<String, AppTargetCommand<'a>>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AppTargetCommand<'a> {
    path: path::PathBuf,
    args: &'a [String],
    is_executable: bool,
}

pub(crate) fn target_show<'a>(
    cfg: &'a core::Config,
    input: TargetShowInput,
    work_path: &path::Path,
) -> Result<TargetShowOutput<'a>, MonorailError> {
    let mut target_groups = None;
    let mut targets = vec![];

    let mut target_set = HashSet::new();
    for t in &cfg.targets {
        let mut out_target = AppTarget {
            path: &t.path,
            uses: &t.uses,
            ignores: &t.ignores,
            commands: None,
        };
        if input.show_target_groups {
            target_set.insert(&t.path);
        }
        if input.show_commands {
            let found = find_target_commands(t, work_path)?;
            out_target.commands = Some(found);
        }
        targets.push(out_target);
    }

    // Merges the target's command definitions with a filesytem walk to
    // create a complete view of a targets available commands.
    fn find_target_commands<'a>(
        target: &'a core::Target,
        work_path: &path::Path,
    ) -> Result<HashMap<String, AppTargetCommand<'a>>, MonorailError> {
        let mut o = HashMap::new();
        let mut def_paths = HashSet::new();
        let target_command_path = work_path.join(&target.path).join(&target.commands.path);
        // first, process any defined commands and note the paths we've seen
        if let Some(defs) = &target.commands.definitions {
            for (name, def) in defs {
                let def_path = target_command_path.join(&def.path);
                o.insert(
                    name.to_string(),
                    AppTargetCommand {
                        path: path::Path::new(&target.commands.path).join(&def.path),
                        args: &def.args,
                        is_executable: file::is_executable(&def_path),
                    },
                );
                def_paths.insert(def_path);
            }
        }
        // now walk the target.commands.path looking for files that we haven't already seen
        if let Ok(entries) = std::fs::read_dir(&target_command_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && !def_paths.contains(&path) {
                    if let Some(stem) = path.file_stem() {
                        o.insert(
                            stem.to_str()
                                .ok_or(MonorailError::Generic(format!(
                                    "Bad string for stem {:?}",
                                    stem
                                )))?
                                .to_string(),
                            AppTargetCommand {
                                path: path::Path::new(&target.commands.path).join(
                                    path.file_name().ok_or(MonorailError::Generic(format!(
                                        "Bad file name {:?}",
                                        path.file_name()
                                    )))?,
                                ),
                                args: &[],
                                is_executable: file::is_executable(&path),
                            },
                        );
                    }
                }
            }
        }
        Ok(o)
    }

    if input.show_target_groups {
        let mut index = core::Index::new(cfg, &target_set, work_path)?;
        target_groups = Some(index.dag.get_labeled_groups()?);
    }

    Ok(TargetShowOutput {
        targets,
        target_groups,
    })
}
