use std::collections::HashMap;
use std::path;
use std::result::Result;

use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio_stream::StreamExt;

use crate::core::error::MonorailError;
use crate::core::{file, tracking, Change};

#[derive(Debug)]
pub(crate) struct GitOptions<'a> {
    pub(crate) begin: Option<&'a str>,
    pub(crate) end: Option<&'a str>,
    pub(crate) git_path: &'a str,
}

pub(crate) async fn get_filtered_changes(
    changes: Vec<Change>,
    pending: &HashMap<String, String>,
    work_path: &path::Path,
) -> Vec<Change> {
    tokio_stream::iter(changes)
        .then(|x| async {
            if file::checksum_is_equal(pending, work_path, &x.name).await {
                None
            } else {
                Some(x)
            }
        })
        .filter_map(|x| x)
        .collect()
        .await
}

// Fetch diff changes, using the tracking checkpoint commit if present.
pub(crate) async fn get_git_diff_changes<'a>(
    git_opts: &'a GitOptions<'a>,
    checkpoint: &'a Option<tracking::Checkpoint>,
    work_path: &path::Path,
) -> Result<Option<Vec<Change>>, MonorailError> {
    let begin = git_opts.begin.or_else(|| {
        // otherwise, check checkpoint.id; if provided, use that
        if let Some(checkpoint) = checkpoint {
            if checkpoint.id.is_empty() {
                None
            } else {
                Some(&checkpoint.id)
            }
        } else {
            // if not then there's nowhere to begin from, and we're done
            None
        }
    });

    let end = match begin {
        Some(_) => git_opts.end,
        None => None,
    };

    let diff_changes = git_cmd_diff_changes(git_opts.git_path, work_path, begin, end).await?;
    if begin.is_none() && end.is_none() && diff_changes.is_empty() {
        // no pending changes and diff range is ok, but signficant in that it
        // means change detection is impossible and other processes should consider
        // all targets changed
        Ok(None)
    } else {
        Ok(Some(diff_changes))
    }
}

pub(crate) async fn get_git_all_changes<'a>(
    git_opts: &'a GitOptions<'a>,
    checkpoint: &'a Option<tracking::Checkpoint>,
    work_path: &path::Path,
) -> Result<Option<Vec<Change>>, MonorailError> {
    let (diff_changes, mut other_changes) = tokio::try_join!(
        get_git_diff_changes(git_opts, checkpoint, work_path),
        git_cmd_other_changes(git_opts.git_path, work_path)
    )?;
    if let Some(diff_changes) = diff_changes {
        other_changes.extend(diff_changes);
    }
    let mut filtered_changes = match checkpoint {
        Some(checkpoint) => {
            if let Some(pending) = &checkpoint.pending {
                if !pending.is_empty() {
                    get_filtered_changes(other_changes, pending, work_path).await
                } else {
                    other_changes
                }
            } else {
                other_changes
            }
        }
        None => {
            // no changes and no checkpoint means change detection is not possible
            if other_changes.is_empty() {
                return Ok(None);
            }
            other_changes
        }
    };

    filtered_changes.sort();
    Ok(Some(filtered_changes))
}

pub(crate) async fn get_git_cmd_child(
    git_path: &str,
    work_path: &path::Path,
    args: &[&str],
) -> Result<tokio::process::Child, MonorailError> {
    tokio::process::Command::new(git_path)
        .args(args)
        .current_dir(work_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(MonorailError::from)
}

pub(crate) async fn git_cmd_rev_parse(
    git_path: &str,
    work_path: &path::Path,
    reference: &str,
) -> Result<String, MonorailError> {
    let mut child = get_git_cmd_child(git_path, work_path, &["rev-parse", reference]).await?;
    let mut stdout_string = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_string(&mut stdout_string).await?;
    }
    let mut stderr_string = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_string(&mut stderr_string).await?;
    }
    let status = child.wait().await.map_err(|e| {
        MonorailError::Generic(format!(
            "Error resolving git reference; error: {}, reason: {}",
            e, &stderr_string
        ))
    })?;
    if status.success() {
        Ok(stdout_string.trim().to_string())
    } else {
        Err(MonorailError::Generic(format!(
            "Error resolving git reference: {}",
            stderr_string
        )))
    }
}

pub(crate) async fn git_cmd_other_changes(
    git_path: &str,
    work_path: &path::Path,
) -> Result<Vec<Change>, MonorailError> {
    let mut child = get_git_cmd_child(
        git_path,
        work_path,
        &["ls-files", "--others", "--exclude-standard"],
    )
    .await?;
    let mut out = vec![];
    if let Some(stdout) = child.stdout.take() {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            out.push(Change { name: line });
        }
    }
    let mut stderr_string = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_string(&mut stderr_string).await?;
    }
    let status = child.wait().await.map_err(|e| {
        MonorailError::Generic(format!(
            "Error getting git diff; error: {}, reason: {}",
            e, &stderr_string
        ))
    })?;
    if status.success() {
        Ok(out)
    } else {
        Err(MonorailError::Generic(format!(
            "Error getting git diff: {}",
            stderr_string
        )))
    }
}

pub(crate) async fn git_cmd_diff_changes(
    git_path: &str,
    work_path: &path::Path,
    begin: Option<&str>,
    end: Option<&str>,
) -> Result<Vec<Change>, MonorailError> {
    let mut args = vec!["diff", "--name-only", "--find-renames"];
    if let Some(begin) = begin {
        args.push(begin);
    }
    if let Some(end) = end {
        args.push(end);
    }
    let mut child = get_git_cmd_child(git_path, work_path, &args).await?;
    let mut out = vec![];
    if let Some(stdout) = child.stdout.take() {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            out.push(Change { name: line });
        }
    }
    let mut stderr_string = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_string(&mut stderr_string).await?;
    }
    let status = child.wait().await.map_err(|e| {
        MonorailError::Generic(format!(
            "Error getting git diff; error: {}, reason: {}",
            e, &stderr_string
        ))
    })?;
    if status.success() {
        Ok(out)
    } else {
        Err(MonorailError::Generic(format!(
            "Error getting git diff: {}",
            stderr_string
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;

    #[tokio::test]
    async fn test_get_git_diff_changes_ok() -> Result<(), Box<dyn std::error::Error>> {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            begin: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let begin = get_head(&repo_path).await;

        // no begin/end without a checkpoint or pending changes is ok(none)
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            None
        );

        // begin == end is ok
        git_opts.begin = Some(&begin);
        git_opts.end = Some(&begin);
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![])
        );

        // begin < end with changes is ok
        let foo_path = &repo_path.join("foo.txt");
        let _foo_checksum = write_with_checksum(foo_path, &[1]).await?;
        add("foo.txt", &repo_path).await;
        commit(&repo_path).await;
        let end = get_head(&repo_path).await;
        git_opts.begin = Some(&begin);
        git_opts.end = Some(&end);
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        // begin > end with changes is ok
        git_opts.begin = Some(&end);
        git_opts.end = Some(&begin);
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_diff_changes_ok_with_checkpoint() -> Result<(), Box<dyn std::error::Error>>
    {
        let repo_path = init(false).await;
        let git_opts = GitOptions {
            begin: None,
            end: None,
            git_path: "git",
        };

        // no changes with empty checkpoint commit is ok(none)
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: "".to_string(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            None
        );

        // get initial begin of repo
        commit(&repo_path).await;
        let first_head = get_head(&repo_path).await;

        // no changes for valid checkpoint commit is empty vector
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: first_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![])
        );

        // create first file and commit
        let foo_path = &repo_path.join("foo.txt");
        let _ = write_with_checksum(foo_path, &[1]).await?;
        add("foo.txt", &repo_path).await;
        commit(&repo_path).await;
        let second_head = get_head(&repo_path).await;

        // foo visible when checkpoint commit is first head
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: first_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );
        // foo invisble when checkpoint commit is updated to second head
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: second_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![])
        );
        // foo is visible if user passes begin, since it has higher priority over checkpoint commit
        assert_eq!(
            get_git_diff_changes(
                &GitOptions {
                    begin: Some(&first_head),
                    end: None,
                    git_path: "git",
                },
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: second_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_diff_changes_err() -> Result<(), Box<dyn std::error::Error>> {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            begin: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let begin = get_head(&repo_path).await;

        // no begin/end, with invalid checkpoint commit is err
        assert!(get_git_diff_changes(
            &git_opts,
            &Some(tracking::Checkpoint {
                path: path::Path::new("x").to_path_buf(),
                id: "test".to_string(),
                pending: Some(HashMap::from([(
                    "foo.txt".to_string(),
                    "blarp".to_string()
                )])),
            }),
            &repo_path
        )
        .await
        .is_err());

        // bad begin is err
        git_opts.begin = Some("foo");
        assert!(get_git_diff_changes(&git_opts, &None, &repo_path)
            .await
            .is_err());

        // bad end is err
        git_opts.begin = Some(&begin);
        git_opts.end = Some("foo");
        assert!(get_git_diff_changes(&git_opts, &None, &repo_path)
            .await
            .is_err());
        git_opts.begin = None;
        git_opts.end = None;

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_all_changes_ok1() {
        let repo_path = init(false).await;
        // no changes, no checkpoint is ok
        assert!(get_git_all_changes(
            &GitOptions {
                begin: None,
                end: None,
                git_path: "git",
            },
            &None,
            &repo_path
        )
        .await
        .unwrap()
        .is_none());
    }

    #[tokio::test]
    async fn test_get_git_all_changes_ok2() {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            begin: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let begin = get_head(&repo_path).await;
        git_opts.begin = Some(&begin);

        // no changes, with checkpoint is ok
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: begin.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        "dsfksl".to_string()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            0
        );
    }

    #[tokio::test]
    async fn test_get_git_all_changes_ok3() {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            begin: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let begin = get_head(&repo_path).await;
        git_opts.begin = Some(&begin);

        // create a new file and check that it is seen
        let foo_path = &repo_path.join("foo.txt");
        let foo_checksum = write_with_checksum(foo_path, &[1]).await.unwrap();
        add("foo.txt", &repo_path).await;
        commit(&repo_path).await;

        assert_eq!(
            get_git_all_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap()
                .unwrap()
                .len(),
            1
        );

        // update checkpoint to include file and check that it is no longer seen,
        // even though commit sha lags
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: begin.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        foo_checksum.clone()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            0
        );

        let end = get_head(&repo_path).await;
        git_opts.end = Some(&end);

        // create another file and check that it is seen, even though checkpoint
        // points to head
        let bar_path = &repo_path.join("bar.txt");
        let bar_checksum = write_with_checksum(bar_path, &[2]).await.unwrap();
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: end.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        foo_checksum.clone()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            1
        );

        // update checkpoint and verify second file is not seen
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    id: end.clone(),
                    pending: Some(HashMap::from([
                        ("foo.txt".to_string(), foo_checksum),
                        ("bar.txt".to_string(), bar_checksum)
                    ])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            0
        );
    }

    #[tokio::test]
    async fn test_get_filtered_changes() {
        let repo_path = init(false).await;
        let root_path = &repo_path;
        let fname1 = "test1.txt";
        let fname2 = "test2.txt";
        let change1 = Change {
            name: fname1.into(),
        };
        let change2 = Change {
            name: fname2.into(),
        };

        // changes, pending with change checksum match
        assert_eq!(
            get_filtered_changes(
                vec![change1.clone()],
                &get_pair_map(&[(
                    fname1,
                    write_with_checksum(&root_path.join(fname1), &[1, 2, 3])
                        .await
                        .unwrap(),
                )]),
                &repo_path
            )
            .await,
            vec![]
        );

        // changes, pending with change checksum mismatch
        tokio::fs::write(path::Path::new(&repo_path).join(fname1), &[1, 2, 3])
            .await
            .unwrap();
        assert_eq!(
            get_filtered_changes(
                vec![change1.clone()],
                &get_pair_map(&[(fname1, "foo".into(),)]),
                &repo_path
            )
            .await,
            vec![change1.clone()]
        );

        // empty changes, empty pending
        assert_eq!(
            get_filtered_changes(vec![], &HashMap::new(), &repo_path).await,
            vec![]
        );

        // changes, empty pending
        assert_eq!(
            get_filtered_changes(
                vec![change1.clone(), change2.clone()],
                &HashMap::new(),
                &repo_path
            )
            .await,
            vec![change1.clone(), change2.clone()]
        );
        // empty changes, pending
        assert_eq!(
            get_filtered_changes(
                vec![],
                &get_pair_map(&[(
                    fname1,
                    write_with_checksum(&root_path.join(fname1), &[1, 2, 3])
                        .await
                        .unwrap(),
                )]),
                &repo_path
            )
            .await,
            vec![]
        );
    }
}
