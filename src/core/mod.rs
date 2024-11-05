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
    pub(crate) output_dir: String,
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
        let file = File::open(file_path)?;
        let mut buf_reader = BufReader::new(file);
        let buf = buf_reader.fill_buf()?;
        Ok(serde_json::from_str(std::str::from_utf8(buf)?)?)
    }
    pub(crate) fn get_target_path_set(&self) -> HashSet<&String> {
        let mut o = HashSet::new();
        for t in &self.targets {
            o.insert(&t.path);
        }
        o
    }
    pub(crate) fn get_tracking_path(&self, work_path: &path::Path) -> path::PathBuf {
        work_path.join(&self.output_dir).join("tracking")
    }
    pub(crate) fn get_log_path(&self, work_path: &path::Path) -> path::PathBuf {
        work_path.join(&self.output_dir).join("run")
    }
    fn default_output_path() -> String {
        "monorail-out".to_string()
    }
    fn default_max_retained_runs() -> usize {
        10
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandDefinition {
    #[serde(default)]
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
        "monorail".into()
    }
}

#[derive(Debug)]
pub(crate) struct Index<'a> {
    pub(crate) targets: Vec<String>,
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
        let mut targets_builder = TrieBuilder::new();
        let mut ignores_builder = TrieBuilder::new();
        let mut uses_builder = TrieBuilder::new();
        let mut use2targets = HashMap::<&str, Vec<&str>>::new();
        let mut ignore2targets = HashMap::<&str, Vec<&str>>::new();

        let mut dag = graph::Dag::new(cfg.targets.len());

        cfg.targets.iter().enumerate().try_for_each(|(i, target)| {
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
            dag.set_subtree_visibility(node, true);
        }

        targets.sort();
        Ok(Self {
            targets,
            targets_trie,
            ignores: ignores_builder.build(),
            uses: uses_builder.build(),
            use2targets,
            ignore2targets,
            dag,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;

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
        let (c, work_path) = prep_raw_config_repo().await;
        let l = Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();

        assert_eq!(
            l.targets_trie
                .common_prefix_search("rust/target/src/foo.rs")
                .collect::<Vec<String>>(),
            vec!["rust".to_string(), "rust/target".to_string()]
        );
        assert_eq!(
            l.uses
                .common_prefix_search("common/foo.txt")
                .collect::<Vec<String>>(),
            vec!["common".to_string()]
        );
        assert_eq!(
            l.ignores
                .common_prefix_search("rust/target/ignoreme.txt")
                .collect::<Vec<String>>(),
            vec!["rust/target/ignoreme.txt".to_string()]
        );
        // lies within `rust` target, so it's in the dag, not the map
        assert_eq!(*l.use2targets.get("common").unwrap(), vec!["rust/target"]);
        assert_eq!(
            *l.ignore2targets.get("rust/target/ignoreme.txt").unwrap(),
            vec!["rust/target"]
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
