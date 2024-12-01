use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path;
use std::result::Result;
use std::str::FromStr;

use crate::core::{self, error::MonorailError, file};

#[derive(Debug, Serialize)]
pub(crate) enum TargetRenderType {
    Dot,
}
impl FromStr for TargetRenderType {
    type Err = MonorailError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "dot" => Ok(TargetRenderType::Dot),
            x => Err(MonorailError::Generic(format!(
                "Unrecognized render type: {}",
                x
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetRenderInput {
    pub(crate) render_type: TargetRenderType,
    pub(crate) output_file: path::PathBuf,
}
#[derive(Debug, Serialize)]
pub(crate) struct TargetRenderOutput {
    pub(crate) output_file: path::PathBuf,
}

pub(crate) fn target_render(
    cfg: &core::Config,
    input: TargetRenderInput,
    work_path: &path::Path,
) -> Result<TargetRenderOutput, MonorailError> {
    let ths = cfg.get_target_path_set();
    let index = core::Index::new(cfg, &ths, work_path)?;
    match input.render_type {
        TargetRenderType::Dot => index.dag.render_dotfile(&input.output_file)?,
    }
    Ok(TargetRenderOutput {
        output_file: input.output_file,
    })
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetShowInput {
    pub(crate) show_target_groups: bool,
    pub(crate) show_commands: bool,
    pub(crate) show_argmaps: bool,
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
    pub(crate) commands: Option<HashMap<String, AppTargetFile>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) argmaps: Option<HashMap<String, AppTargetFile>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AppTargetFile {
    pub(crate) path: Option<path::PathBuf>,
    pub(crate) permissions: Option<String>,
}
impl AppTargetFile {
    pub(crate) fn new(
        name: &str,
        def: Option<&core::FileDefinition>,
        search_path: &path::Path,
        work_path: &path::Path,
    ) -> Self {
        let p = match def {
            Some(def) => {
                if def.path.is_empty() {
                    // if no path is provided, attempt to discover it
                    file::find_file_by_stem(name, search_path)
                } else {
                    // otherwise, use what was provided instead
                    Some(work_path.join(&def.path))
                }
            }
            None => file::find_file_by_stem(name, search_path),
        };
        let mut permissions = None;
        if let Some(ref p) = p {
            permissions = file::permissions(p);
        }
        Self {
            path: p,
            permissions,
        }
    }
    // Strip the internal path of the given prefix, mainly for display.
    pub(crate) fn strip_path_prefix(&mut self, prefix: &path::Path) -> Result<(), MonorailError> {
        if let Some(ref mut path) = self.path {
            *path = path
                .strip_prefix(prefix)
                .map_err(|e| MonorailError::Generic(format!("Error stripping path prefix: {}", e)))?
                .to_path_buf();
        }
        Ok(())
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
            argmaps: None,
        };
        if input.show_target_groups {
            target_set.insert(&t.path);
        }
        if input.show_commands {
            let target_command_path = work_path.join(t.commands.get_path(path::Path::new(&t.path)));
            let found =
                find_target_files(&t.commands.definitions, &target_command_path, work_path)?;
            if !found.is_empty() {
                out_target.commands = Some(found);
            }
        }
        if input.show_argmaps {
            let target_argmap_path = work_path.join(t.argmaps.get_path(path::Path::new(&t.path)));
            let found = find_target_files(&t.argmaps.definitions, &target_argmap_path, work_path)?;
            if !found.is_empty() {
                out_target.argmaps = Some(found);
            }
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
fn find_target_files(
    definitions: &Option<HashMap<String, core::FileDefinition>>,
    search_path: &path::Path,
    work_path: &path::Path,
) -> Result<HashMap<String, AppTargetFile>, MonorailError> {
    let mut o = HashMap::new();
    let mut def_paths = HashSet::new();

    // first, process any defined commands and note the paths we've seen
    if let Some(defs) = &definitions {
        for (name, def) in defs {
            // if def.path is specified, we will use that instead
            let def_path = if def.path.is_empty() {
                search_path.to_path_buf()
            } else {
                work_path.join(&def.path)
            };
            let mut atf = AppTargetFile::new(name, Some(def), search_path, work_path);
            atf.strip_path_prefix(work_path)?;
            o.insert(name.to_string(), atf);
            def_paths.insert(def_path);
        }
    }
    // now walk the target.commands.path looking for files that we haven't already seen
    if let Ok(entries) = std::fs::read_dir(search_path) {
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
                    let mut atf = AppTargetFile::new(&stem_str, None, search_path, work_path);
                    atf.strip_path_prefix(work_path)?;
                    o.insert(stem_str.clone(), atf);
                }
            }
        }
    }
    Ok(o)
}
