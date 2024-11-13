pub(crate) mod error;
pub(crate) mod file;
pub(crate) mod git;
pub(crate) mod graph;
pub(crate) mod tracking;

#[cfg(test)]
pub(crate) mod testing;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path;
use std::result::Result;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use trie_rs::{Trie, TrieBuilder};

use crate::core::error::{GraphError, MonorailError};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Change {
    pub(crate) name: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub(crate) enum ChangeProviderKind {
    #[serde(rename = "git")]
    #[default]
    Git,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangeProvider {
    pub(crate) r#use: ChangeProviderKind,
}

impl FromStr for ChangeProviderKind {
    type Err = MonorailError;
    fn from_str(s: &str) -> Result<ChangeProviderKind, Self::Err> {
        match s {
            "git" => Ok(ChangeProviderKind::Git),
            _ => Err(MonorailError::Generic(format!(
                "Unrecognized change provider kind: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogConfig {
    // Tick frequency for flushing accumulated logs to stream
    // and compression tasks
    pub(crate) flush_interval_ms: u64,
}
impl Default for LogConfig {
    fn default() -> Self {
        Self {
            flush_interval_ms: 500,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default = "Config::default_output_path")]
    pub(crate) out_dir: String,
    #[serde(default = "Config::default_max_retained_runs")]
    pub(crate) max_retained_runs: usize,
    #[serde(default)]
    pub(crate) change_provider: ChangeProvider,
    #[serde(default)]
    pub(crate) targets: Vec<Target>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sequences: Option<HashMap<String, Vec<String>>>,
    #[serde(default)]
    pub(crate) log: LogConfig,
}
impl Config {
    pub(crate) fn new(file_path: &path::Path) -> Result<Config, MonorailError> {
        let file = File::open(file_path).map_err(|e| {
            MonorailError::Generic(format!(
                "Could not open configuration file at {}; {}",
                file_path.display(),
                e
            ))
        })?;
        let mut buf_reader = BufReader::new(file);
        let buf = buf_reader.fill_buf().map_err(|e| {
            MonorailError::Generic(format!(
                "Could not read configuration file data at {}; {}",
                file_path.display(),
                e
            ))
        })?;
        serde_json::from_str(std::str::from_utf8(buf).map_err(|e| {
            MonorailError::Generic(format!(
                "Configuration file at {} contains invalid UTF-8; {}",
                file_path.display(),
                e
            ))
        })?)
        .map_err(|e| {
            MonorailError::Generic(format!(
                "Configuration file at {} contains invalid JSON; {}",
                file_path.display(),
                e
            ))
        })
    }
    pub(crate) fn get_target_path_set(&self) -> HashSet<&String> {
        let mut o = HashSet::new();
        for t in &self.targets {
            o.insert(&t.path);
        }
        o
    }
    pub(crate) fn get_tracking_path(&self, work_path: &path::Path) -> path::PathBuf {
        work_path.join(&self.out_dir).join("tracking")
    }
    pub(crate) fn get_run_path(&self, work_path: &path::Path) -> path::PathBuf {
        work_path.join(&self.out_dir).join("run")
    }
    fn default_output_path() -> String {
        "monorail-out".to_string()
    }
    fn default_max_retained_runs() -> usize {
        10
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandDefinition {
    #[serde(default)]
    pub(crate) path: String,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    // The filesystem path, relative to the repository root.
    pub(crate) path: String,
    // Out-of-path directories that should affect this target. If this
    // path lies within a target, then a dependency for this target
    // on the other target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uses: Option<Vec<String>>,
    // Paths that should not affect this target; has the highest
    // precedence when evaluating a change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ignores: Option<Vec<String>>,
    // Configuration and optional overrides for commands.
    #[serde(default)]
    pub(crate) commands: TargetCommands,

    // Configuration and optional overrides for argmaps.
    #[serde(default)]
    pub(crate) argmaps: TargetArgMaps,
}
impl Target {
    pub(crate) fn get_argmap_base_path(&self, work_path: &path::Path) -> path::PathBuf {
        work_path
            .join(&self.path)
            .join(&self.argmaps.path)
            .join(&self.argmaps.base)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TargetArgMaps {
    // Relative path from this target's `path` to a directory containing
    // base argmap files to be used when this target is involved in
    // `monorail run`. These argmaps are the first loaded, so any runtime
    // instances of --args, --target-argmap, and/or --target-argmap-files are merged
    // into
    #[serde(default = "TargetArgMaps::default_path")]
    pub(crate) path: String,

    // A default argmap to load for this target.
    #[serde(default = "TargetArgMaps::default_base")]
    pub(crate) base: String,
}

impl Default for TargetArgMaps {
    fn default() -> Self {
        Self {
            path: Self::default_path(),
            base: Self::default_base(),
        }
    }
}
impl TargetArgMaps {
    fn default_path() -> String {
        "monorail/argmap".into()
    }
}
impl TargetArgMaps {
    fn default_base() -> String {
        "base.json".into()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TargetCommands {
    // Relative path from this target's `path` to a directory containing
    // commands that can be executed by `monorail run`. Used for
    // any commands that are not mapped to other paths in CommandDefinition.
    #[serde(default = "TargetCommands::default_path")]
    pub(crate) path: String,
    // Mappings of command names to executable statements; these
    // statements will be used when spawning tasks, and if unspecified
    // monorail will try to use an executable named {{command}}*.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) definitions: Option<HashMap<String, CommandDefinition>>,
}
impl Default for TargetCommands {
    fn default() -> Self {
        Self {
            path: Self::default_path(),
            definitions: None,
        }
    }
}
impl TargetCommands {
    fn default_path() -> String {
        "monorail/cmd".into()
    }
}

#[derive(Debug)]
pub(crate) struct Index<'a> {
    pub(crate) targets: Vec<String>,
    pub(crate) target2index: HashMap<&'a str, usize>,
    pub(crate) targets_trie: Trie<u8>,
    pub(crate) ignores: Trie<u8>,
    pub(crate) uses: Trie<u8>,
    pub(crate) use2targets: HashMap<&'a str, Vec<&'a str>>,
    pub(crate) ignore2targets: HashMap<&'a str, Vec<&'a str>>,
    pub(crate) dag: graph::Dag,
}
impl<'a> Index<'a> {
    pub(crate) fn new(
        cfg: &'a Config,
        visible_targets: &HashSet<&String>,
        work_path: &path::Path,
    ) -> Result<Self, MonorailError> {
        let mut targets = vec![];
        let mut target2index = HashMap::new();
        let mut targets_builder = TrieBuilder::new();
        let mut ignores_builder = TrieBuilder::new();
        let mut uses_builder = TrieBuilder::new();
        let mut use2targets = HashMap::<&str, Vec<&str>>::new();
        let mut ignore2targets = HashMap::<&str, Vec<&str>>::new();

        let mut dag = graph::Dag::new(cfg.targets.len());

        cfg.targets.iter().enumerate().try_for_each(|(i, target)| {
            target2index.insert(target.path.as_str(), i);
            targets.push(target.path.to_owned());
            let target_path_str = target.path.as_str();
            file::contains_file(&work_path.join(target_path_str))?;
            if dag.label2node.contains_key(target_path_str) {
                return Err(MonorailError::DependencyGraph(GraphError::DuplicateLabel(
                    target.path.to_owned(),
                )));
            }
            dag.set_label(&target.path, i);
            targets_builder.push(&target.path);

            if let Some(ignores) = target.ignores.as_ref() {
                ignores.iter().for_each(|s| {
                    ignores_builder.push(s);
                    ignore2targets
                        .entry(s.as_str())
                        .or_default()
                        .push(target_path_str);
                });
            }
            Ok(())
        })?;

        let targets_trie = targets_builder.build();

        // process target uses and build up both the dependency graph, and the direct mapping of non-target uses to the affected targets
        cfg.targets.iter().enumerate().try_for_each(|(i, target)| {
            let target_path_str = target.path.as_str();
            // if this target is under an existing target, add it as a dep
            let mut nodes = targets_trie
                .common_prefix_search(target_path_str)
                .filter(|t: &String| t != &target.path)
                .map(|t| {
                    dag.label2node.get(t.as_str()).copied().ok_or_else(|| {
                        MonorailError::DependencyGraph(GraphError::LabelNodeNotFound(t))
                    })
                })
                .collect::<Result<Vec<usize>, MonorailError>>()?;

            if let Some(uses) = &target.uses {
                for s in uses {
                    let uses_path_str = s.as_str();
                    uses_builder.push(uses_path_str);
                    let matching_targets: Vec<String> =
                        targets_trie.common_prefix_search(uses_path_str).collect();
                    use2targets.entry(s).or_default().push(target_path_str);
                    // a dependency has been established between this target and some
                    // number of targets, so we update the graph
                    nodes.extend(
                        matching_targets
                            .iter()
                            .filter(|&t| t != &target.path)
                            .map(|t| {
                                dag.label2node.get(t.as_str()).copied().ok_or_else(|| {
                                    MonorailError::DependencyGraph(GraphError::LabelNodeNotFound(
                                        t.to_owned(),
                                    ))
                                })
                            })
                            .collect::<Result<Vec<usize>, MonorailError>>()?,
                    );
                }
            }
            nodes.sort();
            nodes.dedup();
            dag.set(i, nodes);
            Ok::<(), MonorailError>(())
        })?;

        // now that the graph is fully constructed, set subtree visibility
        for t in visible_targets {
            let node = dag.label2node.get(t.as_str()).copied().ok_or_else(|| {
                MonorailError::DependencyGraph(GraphError::LabelNodeNotFound(t.to_owned().clone()))
            })?;
            dag.set_subtree_visibility(node, true)?;
        }

        targets.sort();
        Ok(Self {
            targets,
            target2index,
            targets_trie,
            ignores: ignores_builder.build(),
            uses: uses_builder.build(),
            use2targets,
            ignore2targets,
            dag,
        })
    }
    pub(crate) fn get_target_index(&self, target: &str) -> Result<&usize, MonorailError> {
        self.target2index
            .get(target)
            .ok_or(MonorailError::from("Target not found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;
    use tempfile::tempdir;

    #[test]
    fn test_trie() {
        let mut builder = TrieBuilder::new();
        builder.push("rust/target/project1/README.md");
        builder.push("common/log");
        builder.push("common/error");
        builder.push("rust/foo/log");

        let trie = builder.build();

        assert!(trie.exact_match("rust/target/project1/README.md"));
        let matches = trie
            .common_prefix_search("common/log/bar.rs")
            .collect::<Vec<String>>();
        assert_eq!(String::from_utf8_lossy(matches[0].as_bytes()), "common/log");
    }

    #[tokio::test]
    async fn test_index() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let l = Index::new(&c, &c.get_target_path_set(), work_path).unwrap();

        assert_eq!(
            l.targets_trie
                .common_prefix_search("target4/target5/src/foo.rs")
                .collect::<Vec<String>>(),
            vec!["target4".to_string(), "target4/target5".to_string()]
        );
        assert_eq!(
            l.uses
                .common_prefix_search("target3/foo.txt")
                .collect::<Vec<String>>(),
            vec!["target3".to_string()]
        );
        assert_eq!(
            l.ignores
                .common_prefix_search("target4/ignore.txt")
                .collect::<Vec<String>>(),
            vec!["target4/ignore.txt".to_string()]
        );
        // lies within `target3` target, so it's in the dag, not the map
        assert_eq!(*l.use2targets.get("target3").unwrap(), vec!["target4"]);
        assert_eq!(
            *l.ignore2targets.get("target4/target5/ignore.txt").unwrap(),
            vec!["target4/target5"]
        );
    }

    #[test]
    fn test_err_duplicate_target_path() {
        let config_str: &str = r#"
{
    "targets": [
        { "path": "rust" },
        { "path": "rust" }
    ]
}
"#;
        let c: Config = serde_json::from_str(config_str).unwrap();
        let work_path = std::env::current_dir().unwrap();
        assert!(Index::new(&c, &c.get_target_path_set(), &work_path).is_err());
    }
}
