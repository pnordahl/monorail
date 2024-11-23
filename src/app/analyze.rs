use std::cmp::Ordering;
use std::collections::HashSet;
use std::path;
use std::result::Result;

use rayon::prelude::*;
use serde::Serialize;
use tracing::{info, trace};

use crate::core::error::MonorailError;
use crate::core::{self, Change, ChangeProviderKind};
use crate::core::{git, tracking};

#[derive(Debug)]
pub(crate) struct HandleAnalyzeInput<'a> {
    pub(crate) git_opts: git::GitOptions<'a>,
    pub(crate) analyze_input: AnalyzeInput,
}

#[derive(Debug)]
pub(crate) struct AnalyzeInput {
    pub(crate) show_changes: bool,
    pub(crate) show_change_targets: bool,
    pub(crate) show_target_groups: bool,
}
impl AnalyzeInput {
    pub(crate) fn new(
        show_changes: bool,
        show_change_targets: bool,
        show_target_groups: bool,
    ) -> Self {
        Self {
            show_changes,
            show_change_targets,
            show_target_groups,
        }
    }
}

#[derive(Serialize, Debug)]
pub(crate) struct AnalyzeOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) changes: Option<Vec<AnalyzedChange>>,
    pub(crate) targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_groups: Option<Vec<Vec<String>>>,
    pub(crate) checkpointed: bool,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub(crate) struct AnalyzedChange {
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) targets: Option<Vec<AnalyzedChangeTarget>>,
}

#[derive(Hash, Serialize, Debug, Eq, PartialEq)]
pub(crate) struct AnalyzedChangeTarget {
    pub(crate) path: String,
    pub(crate) reason: AnalyzedChangeTargetReason,
}
impl Ord for AnalyzedChangeTarget {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path.cmp(&other.path)
    }
}
impl PartialOrd for AnalyzedChangeTarget {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Hash, Serialize, Debug, Eq, PartialEq)]
pub(crate) enum AnalyzedChangeTargetReason {
    #[serde(rename = "target")]
    Target,
    #[serde(rename = "uses_target")]
    UsesTarget,
    #[serde(rename = "ignores")]
    Ignores,
}

pub(crate) async fn handle_analyze<'a>(
    cfg: &'a core::Config,
    input: &HandleAnalyzeInput<'a>,
    work_path: &'a path::Path,
) -> Result<AnalyzeOutput, MonorailError> {
    let changes = match cfg.change_provider.r#use {
        ChangeProviderKind::Git => match cfg.change_provider.r#use {
            ChangeProviderKind::Git => {
                let tracking = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
                let checkpoint = match tracking.open_checkpoint() {
                    Ok(checkpoint) => Some(checkpoint),
                    Err(MonorailError::TrackingCheckpointNotFound(_)) => None,
                    Err(e) => {
                        return Err(e);
                    }
                };
                // Only check the change provider if a checkpoint is informing us
                match checkpoint {
                    Some(checkpoint) => Some(
                        git::get_git_all_changes(&input.git_opts, &checkpoint, work_path).await?,
                    ),
                    None => None,
                }
            }
        },
    };
    let mut index = core::Index::new(cfg, &cfg.get_target_path_set(), work_path)?;

    analyze(&input.analyze_input, &mut index, changes)
}

pub(crate) fn analyze(
    input: &AnalyzeInput,
    index: &mut core::Index<'_>,
    changes: Option<Vec<Change>>,
) -> Result<AnalyzeOutput, MonorailError> {
    info!("Starting analysis");
    let t1 = std::time::Instant::now();
    let mut checkpointed = false;
    let (analyzed_changes, changed_targets) = match changes {
        Some(ref changes) => changes
            .par_chunks(50)
            .map(|chunk| {
                let mut analyzed_changes = Vec::new();
                let mut chunk_targets = HashSet::new();
                for change in chunk {
                    let change_targets = analyze_change(
                        change,
                        index,
                        &mut chunk_targets,
                        input.show_change_targets,
                    );
                    if input.show_changes {
                        analyzed_changes.push(AnalyzedChange {
                            path: change.name.clone(),
                            targets: change_targets.map(|hs| {
                                let mut v: Vec<AnalyzedChangeTarget> = hs.into_iter().collect();
                                v.sort();
                                v
                            }),
                        });
                    }
                }
                (analyzed_changes, chunk_targets)
            })
            .reduce(
                || (Vec::new(), HashSet::new()),
                |mut acc, (chunk_changes, chunk_targets)| {
                    acc.0.extend(chunk_changes);
                    for target in chunk_targets {
                        acc.1.insert(target);
                    }
                    acc
                },
            ),
        None => (Vec::new(), HashSet::new()),
    };

    // build up output from analysis results
    let mut targets = vec![];
    let mut target_groups = None;
    if changes.is_some() {
        checkpointed = true;
        // copy the hashmap into the output vector
        for t in changed_targets.iter() {
            targets.push(t.clone());
        }
        if input.show_target_groups {
            let groups = index.dag.get_groups()?;

            // prune the groups to contain only affected targets
            let mut pruned_groups: Vec<Vec<String>> = vec![];
            for group in groups.iter().rev() {
                let mut pg: Vec<String> = vec![];
                for id in group {
                    let label = index.dag.get_label_by_node(id)?;
                    if changed_targets.contains::<String>(label) {
                        pg.push(label.to_owned());
                    }
                }
                if !pg.is_empty() {
                    pruned_groups.push(pg);
                }
            }
            target_groups = Some(pruned_groups);
        }
    } else {
        // use config targets and all target groups
        for t in &index.targets {
            targets.push(t.to_string());
        }
        if input.show_target_groups {
            target_groups = Some(index.dag.get_labeled_groups()?);
        }
    }
    targets.sort();
    info!(
        elapsed_secs = t1.elapsed().as_secs_f32(),
        num_changes = if let Some(c) = &changes { c.len() } else { 0 },
        num_targets = targets.len(),
        num_target_groups = if let Some(tg) = &target_groups {
            tg.len()
        } else {
            0
        },
        "Analysis complete"
    );
    Ok(AnalyzeOutput {
        changes: if input.show_changes {
            Some(analyzed_changes)
        } else {
            None
        },
        targets,
        target_groups,
        checkpointed,
    })
}

// Gets the targets that would be ignored by this change.
fn get_ignore_targets<'a>(index: &'a core::Index<'_>, name: &'a str) -> HashSet<&'a str> {
    let mut ignore_targets = HashSet::new();
    index
        .ignores
        .common_prefix_search(name)
        .for_each(|m: String| {
            if let Some(v) = index.ignore2targets.get(m.as_str()) {
                v.iter().for_each(|target| {
                    ignore_targets.insert(*target);
                });
            }
        });
    ignore_targets
}

// Search the index for targets affected by a change, updating the accumulator
// HashSet provided. Optionally, returns a HashSet containing the targets
// affected by this change - primarily for debugging and display purposes.
fn analyze_change<'a>(
    change: &Change,
    index: &'a core::Index<'a>,
    targets: &mut HashSet<String>,
    show_change_targets: bool,
) -> Option<HashSet<AnalyzedChangeTarget>> {
    let ignore_targets = get_ignore_targets(index, &change.name);
    let mut change_targets = if show_change_targets {
        Some(HashSet::new())
    } else {
        None
    };

    index
        .targets_trie
        .common_prefix_search(&change.name)
        .for_each(|target: String| {
            // find the target and its ancestors affected by this change
            if !ignore_targets.contains(target.as_str()) {
                targets.insert(target.to_string());
                update_change_targets(
                    &mut change_targets,
                    &target,
                    AnalyzedChangeTargetReason::Target,
                );
                trace!(target = &target, "Added target");
            } else {
                update_change_targets(
                    &mut change_targets,
                    &target,
                    AnalyzedChangeTargetReason::Ignores,
                );
                trace!(target = &target, "Ignored target");
            }
        });
    index
        .uses
        .common_prefix_search(&change.name)
        .for_each(|m: String| {
            // find any targets mapped to this use
            if !ignore_targets.contains(m.as_str()) {
                if let Some(use_targets) = index.use2targets.get(m.as_str()) {
                    use_targets.iter().for_each(|target| {
                        if !ignore_targets.contains(target) {
                            // each mapped target and its ancestors are added
                            index.targets_trie.common_prefix_search(target).for_each(
                                |target2: String| {
                                    if !ignore_targets.contains(target2.as_str()) {
                                        targets.insert(target.to_string());
                                        update_change_targets(
                                            &mut change_targets,
                                            &target2,
                                            AnalyzedChangeTargetReason::UsesTarget,
                                        );
                                        trace!(target = &target2, "Added uses target");
                                    } else {
                                        update_change_targets(
                                            &mut change_targets,
                                            &target2,
                                            AnalyzedChangeTargetReason::Ignores,
                                        );
                                        trace!(target = &target2, "Ignored uses target");
                                    }
                                },
                            );
                        }
                    });
                }
            }
        });
    change_targets
}

fn update_change_targets(
    change_targets: &mut Option<HashSet<AnalyzedChangeTarget>>,
    target: &str,
    reason: AnalyzedChangeTargetReason,
) {
    if let Some(change_targets) = change_targets {
        change_targets.insert(AnalyzedChangeTarget {
            path: target.to_owned(),
            reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;

    #[tokio::test]
    async fn test_analyze_changes_none() {
        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, false, false);
        let o = analyze(&ai, &mut index, None).unwrap();

        assert!(o.changes.unwrap().is_empty());
        // all targets because changes is None
        assert!(!o.targets.is_empty());
        assert!(!o.checkpointed);
    }

    #[tokio::test]
    async fn test_analyze_changes_empty() {
        let changes = vec![];
        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, false, false);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert!(o.changes.unwrap().is_empty());
        assert!(o.targets.is_empty());
        assert!(o.checkpointed);
    }

    #[tokio::test]
    async fn test_analyze_unknown() {
        let change1 = "not_a_target/file.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let expected_targets: Vec<String> = vec![];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![]),
        }];

        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![]));
        assert!(o.checkpointed);
    }

    #[tokio::test]
    async fn test_analyze_target_file() {
        let change1 = "target6/file.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "target6";
        let expected_targets = vec![target1.to_string()];
        let expected_target_groups = vec![vec![target1.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![AnalyzedChangeTarget {
                path: target1.to_string(),
                reason: AnalyzedChangeTargetReason::Target,
            }]),
        }];

        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
        assert!(o.checkpointed);
    }
    #[tokio::test]
    async fn test_analyze_parent_target() {
        let change1 = "target4/file.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "target4";
        let expected_targets = vec![target1.to_string()];
        let expected_target_groups = vec![vec![target1.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![AnalyzedChangeTarget {
                path: target1.to_string(),
                reason: AnalyzedChangeTargetReason::Target,
            }]),
        }];

        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
        assert!(o.checkpointed);
    }

    #[tokio::test]
    async fn test_analyze_target_ancestors() {
        let change1 = "target4/target5/file.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "target4";
        let target2 = "target4/target5";
        let expected_targets = vec![target1.to_string(), target2.to_string()];
        let expected_target_groups = vec![vec![target1.to_string()], vec![target2.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
            ]),
        }];

        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
        assert!(o.checkpointed);
    }

    #[tokio::test]
    async fn test_analyze_target_uses() {
        let change = "target2/file.txt";
        let changes = vec![Change {
            name: change.to_string(),
        }];
        let target2 = "target2";
        let target3 = "target3";
        let expected_targets = vec![target2.to_string(), target3.to_string()];
        let expected_target_groups = vec![vec![target2.to_string()], vec![target3.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change.to_string(),
            targets: Some(vec![
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
                AnalyzedChangeTarget {
                    path: target3.to_string(),
                    reason: AnalyzedChangeTargetReason::UsesTarget,
                },
            ]),
        }];

        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
        assert!(o.checkpointed);
    }

    #[tokio::test]
    async fn test_analyze_target_ignores_all() {
        let change1 = "target4/ignore.txt";
        let change2 = "target4/target5/ignore.txt";
        let changes = vec![
            Change {
                name: change1.to_string(),
            },
            Change {
                name: change2.to_string(),
            },
        ];
        let target4 = "target4";
        let target5 = "target4/target5";
        let expected_targets: Vec<String> = vec![target4.to_string()];
        let expected_changes = vec![
            AnalyzedChange {
                path: change1.to_string(),
                targets: Some(vec![AnalyzedChangeTarget {
                    path: target4.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                }]),
            },
            AnalyzedChange {
                path: change2.to_string(),
                targets: Some(vec![
                    AnalyzedChangeTarget {
                        path: target4.to_string(),
                        reason: AnalyzedChangeTargetReason::Target,
                    },
                    AnalyzedChangeTarget {
                        path: target5.to_string(),
                        reason: AnalyzedChangeTargetReason::Ignores,
                    },
                ]),
            },
        ];

        let td = new_testdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![vec![target4.to_string()]]));
        assert!(o.checkpointed);
    }
}
