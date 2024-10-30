use std::cmp::Ordering;
use std::{io, path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::PermissionsExt;

use std::result::Result;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio_stream::StreamExt;
use tracing::{debug, error, info, instrument};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{fmt, Registry};
use trie_rs::{Trie, TrieBuilder};

use crate::core::{error::MonorailError, log};

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

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Config {
    #[serde(default = "Config::default_output_path")]
    pub(crate) output_dir: String,
    #[serde(default = "Config::default_max_retained_runs")]
    pub(crate) max_retained_runs: usize,
    #[serde(default)]
    pub(crate) change_provider: ChangeProvider,
    #[serde(default)]
    pub(crate) targets: Vec<Target>,
    #[serde(default)]
    pub(crate) log: log::Config,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct CommandDefinition {
    pub(crate) exec: String,
    pub(crate) args: Vec<String>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
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
pub(crate) struct TargetCommands {
    // Relative path from this target's `path` to a directory containing
    // commands that can be executed by `monorail run`.
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
            path: "monorail".into(),
            definitions: None,
        }
    }
}
