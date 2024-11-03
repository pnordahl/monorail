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
    pub(crate) name: String,
    pub(crate) path: Option<path::PathBuf>,
    pub(crate) args: &'a [String],
    pub(crate) is_executable: bool,
}
impl<'a> AppTargetCommand<'a> {
    pub(crate) fn new(
        name: &'a str,
        def: &'a core::CommandDefinition,
        target_command_path: &path::Path,
        work_path: &path::Path,
    ) -> Self {
        // if no path is provided, attempt to discover it
        let p = if def.path.is_empty() {
            file::find_file_by_stem(name, target_command_path)
        } else {
            // otherwise, use what was provided instead
            Some(work_path.join(&def.path))
        };
        let is_executable = if let Some(ref p) = p {
            file::is_executable(p)
        } else {
            false
        };
        Self {
            name: name.to_owned(),
            path: p,
            args: &def.args,
            is_executable,
        }
    }
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
            let target_command_path = work_path.join(&t.path).join(&t.commands.path);
            let found = find_target_commands(t, &target_command_path, work_path)?;
            out_target.commands = Some(found);
        }
        targets.push(out_target);
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

// Merges the target's command definitions with a filesytem walk to
// create a complete view of a targets available commands.
fn find_target_commands<'a>(
    target: &'a core::Target,
    target_command_path: &path::Path,
    work_path: &path::Path,
) -> Result<HashMap<String, AppTargetCommand<'a>>, MonorailError> {
    let mut o = HashMap::new();
    let mut def_paths = HashSet::new();

    // first, process any defined commands and note the paths we've seen
    if let Some(defs) = &target.commands.definitions {
        for (name, def) in defs {
            // if def.path is specified, we will use that instead
            let def_path = if def.path.is_empty() {
                target_command_path.to_path_buf()
            } else {
                work_path.join(&def.path)
            };
            o.insert(
                name.to_string(),
                AppTargetCommand::new(name, def, target_command_path, work_path),
            );
            def_paths.insert(def_path);
        }
    }
    // now walk the target.commands.path looking for files that we haven't already seen
    if let Ok(entries) = std::fs::read_dir(target_command_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && !def_paths.contains(&path) {
                if let Some(ref stem) = path.file_stem().as_ref() {
                    let stem_str = stem
                        .to_str()
                        .ok_or(MonorailError::Generic(format!(
                            "Bad string for stem {:?}",
                            stem
                        )))?
                        .to_string();
                    // use the args defined for this command, if any
                    let args = match &target.commands.definitions {
                        Some(defs) => match defs.get(&stem_str) {
                            Some(def) => def.args.as_slice(),
                            None => &[],
                        },
                        None => &[],
                    };
                    o.insert(
                        stem_str.clone(),
                        AppTargetCommand {
                            name: stem_str.clone(),
                            path: Some(path::Path::new(&target.commands.path).join(
                                path.file_name().ok_or(MonorailError::Generic(format!(
                                    "Bad file name {:?}",
                                    path.file_name()
                                )))?,
                            )),
                            args,
                            is_executable: file::is_executable(&path),
                        },
                    );
                }
            }
        }
    }
    Ok(o)
}
