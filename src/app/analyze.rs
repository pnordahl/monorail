use std::cmp::Ordering;
use std::collections::HashSet;
use std::path;
use std::result::Result;

use serde::Serialize;

use crate::core::error::{GraphError, MonorailError};
use crate::core::{self, Change, ChangeProviderKind};
use crate::core::{git, tracking};

#[derive(Debug)]
pub(crate) struct HandleAnalyzeInput<'a> {
    pub(crate) git_opts: git::GitOptions<'a>,
    pub(crate) analyze_input: AnalyzeInput,
    pub(crate) targets: HashSet<&'a String>,
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
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub(crate) struct AnalyzedChange {
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) targets: Option<Vec<AnalyzedChangeTarget>>,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
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

#[derive(Serialize, Debug, Eq, PartialEq)]
pub(crate) enum AnalyzedChangeTargetReason {
    #[serde(rename = "target")]
    Target,
    #[serde(rename = "target_parent")]
    TargetParent,
    #[serde(rename = "uses")]
    Uses,
    #[serde(rename = "uses_target_parent")]
    UsesTargetParent,
    #[serde(rename = "ignores")]
    Ignores,
}

pub(crate) async fn handle_analyze<'a>(
    cfg: &'a core::Config,
    input: &HandleAnalyzeInput<'a>,
    work_path: &'a path::Path,
) -> Result<AnalyzeOutput, MonorailError> {
    let (mut index, changes) = match input.targets.len() {
        0 => {
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
                        git::get_git_all_changes(&input.git_opts, &checkpoint, work_path).await?
                    }
                },
            };
            (
                core::Index::new(cfg, &cfg.get_target_path_set(), work_path)?,
                changes,
            )
        }
        _ => (core::Index::new(cfg, &input.targets, work_path)?, None),
    };

    analyze(&input.analyze_input, &mut index, changes)
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

// // todo various stuff aroudn porting analysis show to analyze, factor out some of the internals of analyze

// fn analyze<'a>(
//     input: &AnalyzeInput<'a>,
//     index: &mut core::Index<'_>,
//     changes: Option<Vec<Change>>,
// ) -> Result<AnalyzeOutput, MonorailError> {
//     let mut output = AnalyzeOutput {
//         changes: if input.show_changes {
//             Some(vec![])
//         } else {
//             None
//         },
//         targets: vec![],
//         target_groups: None,
//     };

//     // determine which targets are affected by this change
//     let change_targets = match changes {
//         Some(changes) => {
//             let mut targets = HashSet::new();
//             changes.iter().for_each(|c| {
//                 let mut change_targets = if input.show_change_targets {
//                     Some(vec![])
//                 } else {
//                     None
//                 };
//                 let ignore_targets = get_ignore_targets(index, &c.name);
//                 analyze_change_targets_and_ancestors(
//                     &c.name,
//                     &index,
//                     &ignore_targets,
//                     &mut targets,
//                     &mut output,
//                 );

//                 let use_matches = index.uses_trie.common_prefix_search(&c.name);
//                 use_matches.for_each(|u: String| {
//                     if !ignore_targets.contains(u.as_str()) {
//                         if let Some(v) = index.use2targets.get(u.as_str()) {
//                             v.iter().for_each(|target| {
//                                 analyze_change_targets_and_ancestors(
//                                     target,
//                                     &index,
//                                     &ignore_targets,
//                                     &mut targets,
//                                     &mut output,
//                                 );
//                             });
//                         }
//                     }
//                 });
//             });
//             Some(targets)
//         }
//         None => None,
//     };
// }

// fn analyze_change_targets_and_ancestors(
//     change: &Change,
//     index: &core::Index<'_>,
//     ignore_targets: &HashSet<&str>,
//     targets: &mut HashSet<&str>,
//     output: &mut AnalyzeOutput,
// ) {
//     let target_matches = index.targets_trie.common_prefix_search(&change.name);
//     target_matches.for_each(|target: String| {
//         if !ignore_targets.contains(target.as_str()) {
//             targets.insert(target);
//             trace!(target = &target, "Added");
//             // additionally check any target path ancestors
//             let ancestor_target_matches = index.targets_trie.common_prefix_search(&target);
//             ancestor_target_matches.for_each(|target2: String| {
//                 if target2 != target && !ignore_targets.contains(target2.as_str()) {
//                     targets.insert(target2);
//                     trace!(target = &target, ancestor = &target2, "Added ancestor");
//                 } else {
//                     trace!(target = &target, ancestor = &target2, "Ignored ancestor");
//                 }
//             });
//         } else {
//             trace!(target = &target, "Ignored");
//         }
//     });
// }

pub(crate) fn analyze(
    input: &AnalyzeInput,
    index: &mut core::Index<'_>,
    changes: Option<Vec<Change>>,
) -> Result<AnalyzeOutput, MonorailError> {
    let mut analyzed_changes = if input.show_changes {
        Some(vec![])
    } else {
        None
    };
    let mut targets = vec![];
    let mut target_groups = None;
    let output_targets = match changes {
        Some(changes) => {
            let mut output_targets = HashSet::new();
            changes.iter().for_each(|c| {
                let mut change_targets = if input.show_change_targets {
                    Some(vec![])
                } else {
                    None
                };
                let ignore_targets = get_ignore_targets(index, &c.name);
                index
                    .targets_trie
                    .common_prefix_search(&c.name)
                    .for_each(|target: String| {
                        if !ignore_targets.contains(target.as_str()) {
                            // additionally, add any targets that lie further up from this target
                            index.targets_trie.common_prefix_search(&target).for_each(
                                |target2: String| {
                                    if target2 != target {
                                        if !ignore_targets.contains(target2.as_str()) {
                                            if let Some(change_targets) = change_targets.as_mut() {
                                                change_targets.push(AnalyzedChangeTarget {
                                                    path: target2.to_owned(),
                                                    reason:
                                                        AnalyzedChangeTargetReason::TargetParent,
                                                });
                                            }
                                            output_targets.insert(target2);
                                        } else if let Some(change_targets) = change_targets.as_mut() {
                                            change_targets.push(AnalyzedChangeTarget {
                                                path: target.to_owned(),
                                                reason: AnalyzedChangeTargetReason::Ignores,
                                            });
                                        }
                                    }
                                },
                            );
                            if let Some(change_targets) = change_targets.as_mut() {
                                change_targets.push(AnalyzedChangeTarget {
                                    path: target.to_owned(),
                                    reason: AnalyzedChangeTargetReason::Target,
                                });
                            }
                            output_targets.insert(target);
                        } else if let Some(change_targets) = change_targets.as_mut() {
                            change_targets.push(AnalyzedChangeTarget {
                                path: target.to_owned(),
                                reason: AnalyzedChangeTargetReason::Ignores,
                            });
                        }
                    });
                index
                    .uses
                    .common_prefix_search(&c.name)
                    .for_each(|m: String| {
                        if !ignore_targets.contains(m.as_str()) {
                            if let Some(v) = index.use2targets.get(m.as_str()) {
                                v.iter().for_each(|target| {
                                    if !ignore_targets.contains(target) {
                                        // additionally, add any targets that lie further up from this target
                                        index.targets_trie.common_prefix_search(target).for_each(
                                            |target2: String| {
                                                if &target2 != target {
                                                    if !ignore_targets.contains(target2.as_str()) {
                                                        if let Some(change_targets) = change_targets.as_mut() {
                                                            change_targets.push(AnalyzedChangeTarget {
                                                                path: target2.to_owned(),
                                                                reason: AnalyzedChangeTargetReason::UsesTargetParent,
                                                            });
                                                        }
                                                        output_targets.insert(target2);
                                                } else if let Some(change_targets) = change_targets.as_mut() {
                                                        change_targets.push(AnalyzedChangeTarget {
                                                            path: target.to_string(),
                                                            reason: AnalyzedChangeTargetReason::Ignores,
                                                        });
                                                    }
                                                }
                                            },
                                        );
                                        if let Some(change_targets) = change_targets.as_mut() {
                                            change_targets.push(AnalyzedChangeTarget {
                                                path: target.to_string(),
                                                reason: AnalyzedChangeTargetReason::Uses,
                                            });
                                        }
                                        output_targets.insert(target.to_string());
                                    } else if let Some(change_targets) = change_targets.as_mut() {
                                        change_targets.push(AnalyzedChangeTarget {
                                            path: target.to_string(),
                                            reason: AnalyzedChangeTargetReason::Ignores,
                                        });
                                    }
                                });
                            }
                        }
                    });

                if let Some(change_targets) = change_targets.as_mut() {
                    change_targets.sort();
                }

                if let Some(output_changes) = analyzed_changes.as_mut() {
                    output_changes.push(AnalyzedChange {
                        path: c.name.to_owned(),
                        targets: change_targets,
                    });
                }
            });
            Some(output_targets)
        }
        None => None,
    };

    // todo; copy this into analyze factored
    if let Some(output_targets) = output_targets {
        // copy the hashmap into the output vector
        for t in output_targets.iter() {
            targets.push(t.clone());
        }
        if input.show_target_groups {
            let groups = index.dag.get_groups()?;

            // prune the groups to contain only affected targets
            let mut pruned_groups: Vec<Vec<String>> = vec![];
            for group in groups.iter().rev() {
                let mut pg: Vec<String> = vec![];
                for id in group {
                    let label = index.dag.node2label.get(id).ok_or_else(|| {
                        MonorailError::DependencyGraph(GraphError::LabelNotFound(*id))
                    })?;
                    if output_targets.contains::<String>(label) {
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

    Ok(AnalyzeOutput {
        changes: analyzed_changes,
        targets,
        target_groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;

    #[tokio::test]
    async fn test_analyze_empty() {
        let changes = vec![];
        let (c, work_path) = new_test_repo().await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();
        let ai = AnalyzeInput::new(true, false, false);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert!(o.changes.unwrap().is_empty());
        assert!(o.targets.is_empty());
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

        let (c, work_path) = new_test_repo().await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![]));
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

        let (c, work_path) = new_test_repo().await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
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

        let (c, work_path) = new_test_repo().await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
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
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::TargetParent,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
            ]),
        }];

        let (c, work_path) = new_test_repo().await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[tokio::test]
    async fn test_analyze_target_uses() {
        let change1 = "target2/file.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "target2";
        let target2 = "target3";
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
                    reason: AnalyzedChangeTargetReason::Uses,
                },
            ]),
        }];

        let (c, work_path) = new_test_repo().await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
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
        let target1 = "target4";
        let target2 = "target4/target5";
        let expected_targets: Vec<String> = vec![target1.to_string()];
        let expected_changes = vec![
            AnalyzedChange {
                path: change1.to_string(),
                targets: Some(vec![AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                }]),
            },
            AnalyzedChange {
                path: change2.to_string(),
                targets: Some(vec![
                    AnalyzedChangeTarget {
                        path: target1.to_string(),
                        reason: AnalyzedChangeTargetReason::Target,
                    },
                    AnalyzedChangeTarget {
                        path: target2.to_string(),
                        reason: AnalyzedChangeTargetReason::Ignores,
                    },
                ]),
            },
        ];

        let (c, work_path) = new_test_repo().await;
        let mut index = core::Index::new(&c, &c.get_target_path_set(), &work_path).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut index, Some(changes)).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![vec![target1.to_string()]]));
    }
}
