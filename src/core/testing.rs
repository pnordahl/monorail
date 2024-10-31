use crate::core;
use sha2::Digest;
use std::collections::HashMap;
use std::path;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::AsyncWriteExt;

pub(crate) const RAW_CONFIG: &str = r#"
{
    "targets": [
        {
            "path": "rust"
        },
        {
            "path": "rust/target",
            "ignores": [
                "rust/target/ignoreme.txt"
            ],
            "uses": [
                "rust/vendor",
                "common"
            ]
        }
    ]
}
"#;

// Using this in tests allows them to execute concurrently with a clean repo in each case.
static GLOBAL_REPO_ID: AtomicUsize = AtomicUsize::new(0);

// Git utils
pub(crate) fn test_path() -> path::PathBuf {
    path::Path::new(&"/tmp/monorail".to_string()).to_path_buf()
}

pub(crate) fn repo_path(test_path: &path::Path, n: usize) -> path::PathBuf {
    test_path.join(format!("test-{}", n))
}

pub(crate) async fn init(bare: bool) -> path::PathBuf {
    let id = GLOBAL_REPO_ID.fetch_add(1, Ordering::SeqCst);
    let test_path = test_path();
    tokio::fs::create_dir_all(&test_path).await.unwrap();
    let repo_path = repo_path(&test_path, id);
    let mut args = vec!["init", repo_path.to_str().unwrap()];
    if bare {
        args.push("--bare");
    }
    if repo_path.exists() {
        tokio::fs::remove_dir_all(&repo_path).await.unwrap_or(());
    }
    let _ = tokio::process::Command::new("git")
        .args(&args)
        .current_dir(&test_path)
        .output()
        .await
        .unwrap();
    repo_path
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
) -> tokio::fs::File {
    let dir_path = repo_path.join(dir);
    tokio::fs::create_dir_all(&dir_path).await.unwrap();
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&dir_path.join(file_name))
        .await
        .unwrap();
    file.write_all(content).await.unwrap();
    file
}

pub(crate) async fn write_with_checksum(
    path: &path::Path,
    data: &[u8],
) -> Result<String, tokio::io::Error> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    tokio::fs::write(path, &data).await?;
    Ok(hex::encode(hasher.finalize()).to_string())
}

pub(crate) fn get_pair_map(pairs: &[(&str, String)]) -> HashMap<String, String> {
    let mut pending = HashMap::new();
    for (fname, checksum) in pairs {
        pending.insert(fname.to_string(), checksum.clone());
    }
    pending
}

pub(crate) async fn prep_raw_config_repo() -> (core::Config, path::PathBuf) {
    let repo_path = init(false).await;
    let c: core::Config = serde_json::from_str(RAW_CONFIG).unwrap();

    create_file(
        &repo_path,
        "rust",
        "monorail.sh",
        b"command whoami { echo 'rust' }",
    )
    .await;
    create_file(
        &repo_path,
        "rust/target",
        "monorail.sh",
        b"command whoami { echo 'rust/target' }",
    )
    .await;
    (c, repo_path)
}
