use core::sync::atomic::{AtomicUsize, Ordering};
use std::path;
use tokio::io::AsyncWriteExt;

// Using this in tests allows them to execute concurrently with a clean repo in each case.
static GLOBAL_REPO_ID: AtomicUsize = AtomicUsize::new(0);

// Git utils
pub fn test_path() -> path::PathBuf {
    path::Path::new(&"/tmp/monorail".to_string()).to_path_buf()
}

pub fn repo_path(test_path: &path::Path, n: usize) -> path::PathBuf {
    test_path.join(format!("test-{}", n))
}

pub async fn init(bare: bool) -> path::PathBuf {
    let id = GLOBAL_REPO_ID.fetch_add(1, Ordering::SeqCst);
    let test_path = test_path();
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

pub async fn add(name: &str, repo_path: &path::Path) {
    let _ = tokio::process::Command::new("git")
        .arg("add")
        .arg(name)
        .current_dir(repo_path)
        .output()
        .await
        .unwrap();
}
pub async fn commit(repo_path: &path::Path) {
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
pub async fn get_head(repo_path: &path::Path) -> String {
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
pub async fn create_file(
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
