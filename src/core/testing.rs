use crate::core;
use sha2::Digest;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path;
use tokio::io::AsyncWriteExt;

pub(crate) const TEST_CONFIG: &str = r#"
{
    "targets": [
        {
            "path": "target1",
            "commands": {
                "path": "target1/commands",
                "definitions": {
                    "cmd1": {},
                    "cmd2": {
                        "path": "target1/commands/cmd2.sh"
                    },
                    "cmd3": {},
                    "cmd4": {
                        "path": "target1/commands/cmd4.sh"
                    }
                }
            }
        },
        {
            "path": "target2"
        },
        {
            "path": "target3",
            "uses": [
                "target1",
                "target2"
            ]
        },
        {
            "path": "target4",
            "uses": [
                "target3"
            ]
        },
        {
            "path": "target4/target5",
            "ignores": [
                "target4/ignore.txt",
                "target4/target5/ignore.txt"
            ]
        },
        {
            "path": "target6",
            "uses": [
                "target4/target5/use.txt",
                "not_a_target/commands"
            ],
            "commands": {
                "definitions": {
                    "cmd0": {
                        "path": "not_a_target/commands/cmd0.sh"
                    }
                }
            }
        }
    ],
    "sequences": {
        "seq1": [
            "cmd0",
            "cmd1",
            "cmd2",
            "cmd3",
            "cmd4"
        ]
    }
}
"#;

pub(crate) fn new_testdir() -> Result<tempfile::TempDir, std::io::Error> {
    match std::env::var("MONORAIL_TEST_DIR") {
        Ok(v) => tempfile::tempdir_in(v),
        Err(_) => tempfile::tempdir(),
    }
}

pub(crate) async fn init(repo_path: &path::Path, bare: bool) {
    let mut args = vec!["init", repo_path.to_str().unwrap()];
    if bare {
        args.push("--bare");
    }
    let _ = tokio::process::Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .await
        .unwrap();
}

pub(crate) async fn add(name: &str, repo_path: &path::Path) {
    let _ = tokio::process::Command::new("git")
        .arg("add")
        .arg(name)
        .current_dir(repo_path)
        .output()
        .await
        .unwrap();
}
pub(crate) async fn commit(repo_path: &path::Path) {
    let _ = tokio::process::Command::new("git")
        .arg("commit")
        .arg("-a")
        .arg("-m")
        .arg("test")
        .arg("--allow-empty")
        .current_dir(repo_path)
        .output()
        .await
        .unwrap();
}
pub(crate) async fn get_head(repo_path: &path::Path) -> String {
    let end = String::from_utf8(
        tokio::process::Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(repo_path)
            .output()
            .await
            .unwrap()
            .stdout,
    )
    .unwrap();
    String::from(end.trim())
}

pub(crate) async fn create_file(
    repo_path: &path::Path,
    dir: &str,
    file_name: &str,
    content: &[u8],
    executable: bool,
) -> tokio::fs::File {
    let dir_path = repo_path.join(dir);
    tokio::fs::create_dir_all(&dir_path).await.unwrap();
    let fpath = &dir_path.join(file_name);
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&fpath)
        .await
        .unwrap();
    file.write_all(content).await.unwrap();
    if executable {
        let mut permissions = file.metadata().await.unwrap().permissions();
        permissions.set_mode(0o755);
        tokio::fs::set_permissions(&fpath, permissions)
            .await
            .unwrap();
    }
    file
}

pub(crate) async fn write_with_checksum(
    path: &path::Path,
    data: &[u8],
) -> Result<String, tokio::io::Error> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    tokio::fs::write(path, &data).await?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn get_pair_map(pairs: &[(&str, String)]) -> HashMap<String, String> {
    let mut pending = HashMap::new();
    for (fname, checksum) in pairs {
        pending.insert(fname.to_string(), checksum.clone());
    }
    pending
}

pub(crate) async fn new_test_repo(rp: &path::Path) -> core::Config {
    init(rp, false).await;
    // make initial head
    commit(rp).await;
    let c: core::Config = serde_json::from_str(TEST_CONFIG).unwrap();

    // make all directories
    tokio::fs::create_dir_all(rp.join("not_a_target/commands"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(rp.join("target1/commands"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(rp.join("target2")).await.unwrap();
    tokio::fs::create_dir_all(rp.join("target3/monorail/cmd"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(rp.join("target4/monorail/cmd"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(rp.join("target4/target5/monorail/cmd"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(rp.join("target6/monorail/cmd"))
        .await
        .unwrap();

    // fill with files
    create_file(rp, "not_a_target", "file.txt", b"1", false).await;
    create_file(
        rp,
        "not_a_target/commands",
        "cmd0.sh",
        b"#!/usr/bin/env bash\necho 'not_a_target cmd0'",
        true,
    )
    .await;

    // target1
    create_file(
        rp,
        "target1/commands",
        "cmd0.sh",
        b"#!/usr/bin/env bash\necho 'target1 cmd0'",
        true,
    )
    .await;
    create_file(
        rp,
        "target1/commands",
        "cmd1.sh",
        b"#!/usr/bin/env bash\nexit 1",
        true,
    )
    .await;
    create_file(
        rp,
        "target1/commands",
        "cmd2.sh",
        b"#!/usr/bin/env bash\necho 'target1 cmd2'",
        true,
    )
    .await;
    create_file(
        rp,
        "target1/commands",
        "cmd3.sh",
        b"#!/usr/bin/env bash\necho \"target1 cmd3\"",
        true,
    )
    .await;
    create_file(
        rp,
        "target1/commands",
        "cmd4.sh",
        b"#!/usr/bin/env bash\necho 'target1 cmd4'",
        true,
    )
    .await;

    // target2
    create_file(
        rp,
        "target2/monorail/cmd",
        "cmd0.sh",
        b"#!/usr/bin/env bash\necho 'target2 cmd0'",
        true,
    )
    .await;
    create_file(
        rp,
        "target2/monorail/cmd",
        "cmd1.sh",
        b"#!/usr/bin/env bash\necho 'target2 cmd1'",
        true,
    )
    .await;

    // target3
    create_file(
        rp,
        "target3/monorail/cmd",
        "cmd0.sh",
        r#"#!/usr/bin/env bash

if [[ "$1" != 'base1' ]]; then
    exit 1
fi
if [[ "$2" != 'base2' ]]; then
    exit 1
fi

        "#
        .as_bytes(),
        true,
    )
    .await;
    create_file(
        rp,
        "target3/monorail/argmap",
        "base.json",
        r#"{"cmd0":["base1", "base2"]}"#.as_bytes(),
        false,
    )
    .await;

    // target4
    create_file(rp, "target4", "ignore.txt", b"1", false).await;
    create_file(
        rp,
        "target4/monorail/cmd",
        "cmd0.sh",
        b"#!/usr/bin/env bash\necho 'target4 cmd0'",
        true,
    )
    .await;

    // target4/target5
    create_file(rp, "target4/target5", "ignore.txt", b"1", false).await;
    create_file(rp, "target4/target5", "use.txt", b"1", false).await;
    create_file(
        rp,
        "target4/target5/monorail/cmd",
        "cmd0.sh",
        b"#!/usr/bin/env bash\necho 'target4/target5 cmd0'",
        true,
    )
    .await;

    // target6
    create_file(rp, "target6", "file.txt", b"1", false).await;

    c
}
