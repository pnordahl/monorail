use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::result::Result;
use std::{io, path};

use sha2::Digest;
use tokio::io::AsyncReadExt;
use tracing::debug;

use crate::core::error::MonorailError;

// Open a file from the provided path, and compute its checksum
// TODO: allow hasher to be passed in?
// TODO: configure buffer size based on file size?
// TODO: pass in open file instead of path?
pub(crate) async fn get_file_checksum(p: &path::Path) -> Result<String, MonorailError> {
    let mut file = match tokio::fs::File::open(p).await {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok("".to_string()),
        Err(e) => {
            return Err(MonorailError::from(e));
        }
    };
    let mut hasher = sha2::Sha256::new();

    let mut buffer = [0u8; 64 * 1024];
    loop {
        let num = file.read(&mut buffer).await?;
        if num == 0 {
            break;
        }

        hasher.update(&buffer[..num]);
    }

    Ok(hex::encode(hasher.finalize()).to_string())
}

pub(crate) async fn checksum_is_equal(
    pending: &HashMap<String, String>,
    work_path: &path::Path,
    name: &str,
) -> bool {
    match pending.get(name) {
        Some(checksum) => {
            // compute checksum of x.name and check not equal
            get_file_checksum(&work_path.join(name))
                .await
                // Note that any error here is ignored; the reason for this is that checkpoints
                // can reference stale or deleted paths, and it's not in our best interest to
                // short-circuit change detection when this occurs.
                .map_or(false, |x_checksum| checksum == &x_checksum)
        }
        None => false,
    }
}

pub(crate) fn find_file_by_stem(name: &str, dir: &path::Path) -> Option<path::PathBuf> {
    debug!(
        name = name,
        dir = dir.display().to_string(),
        "Executable search"
    );
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(stem) = path.file_stem() {
                    if stem == name {
                        return Some(path.to_path_buf());
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn is_executable(p: &path::Path) -> bool {
    if let Ok(metadata) = std::fs::metadata(p) {
        let permissions = metadata.permissions();
        return permissions.mode() & 0o111 != 0;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;
    use std::fs::{set_permissions, File};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_checksum_is_equal() {
        let repo_path = init(false).await;
        let fname1 = "test1.txt";

        let root_path = &repo_path;
        let pending = get_pair_map(&[(
            fname1,
            write_with_checksum(&root_path.join(fname1), &[1, 2, 3])
                .await
                .unwrap(),
        )]);

        // checksums must match
        assert!(checksum_is_equal(&pending, &repo_path, fname1).await);
        // file error (such as dne) interpreted as checksum mismatch
        assert!(!checksum_is_equal(&pending, &repo_path, "dne.txt").await);

        // write a file and use a pending entry with a mismatched checksum
        let fname2 = "test2.txt";
        tokio::fs::write(root_path.join(fname2), &[1])
            .await
            .unwrap();
        let pending2 = get_pair_map(&[(fname2, "foobar".into())]);
        // checksums don't match
        assert!(!checksum_is_equal(&pending2, &repo_path, fname2).await);
    }

    #[tokio::test]
    async fn test_get_file_checksum() {
        let repo_path = init(false).await;

        // files that don't exist have an empty checksum
        let p = &repo_path.join("test.txt");
        assert_eq!(get_file_checksum(p).await.unwrap(), "".to_string());

        // write file and compare checksums
        let checksum = write_with_checksum(p, &[1, 2, 3]).await.unwrap();
        assert_eq!(get_file_checksum(p).await.unwrap(), checksum);
    }

    #[test]
    fn test_executable_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("executable_file");

        File::create(&file_path).expect("Failed to create file");
        let mut permissions = std::fs::metadata(&file_path)
            .expect("Failed to get metadata")
            .permissions();
        permissions.set_mode(0o755); // rwxr-xr-x
        set_permissions(&file_path, permissions).expect("Failed to set permissions");

        assert!(is_executable(&file_path));
    }

    #[test]
    fn test_non_executable_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("non_executable_file");

        File::create(&file_path).expect("Failed to create file");
        let mut permissions = std::fs::metadata(&file_path)
            .expect("Failed to get metadata")
            .permissions();
        permissions.set_mode(0o644); // rw-r--r--
        set_permissions(&file_path, permissions).expect("Failed to set permissions");

        assert!(!is_executable(&file_path));
    }

    #[test]
    fn test_nonexistent_path() {
        let non_existent_path = Path::new("/non_existent_path");

        assert!(!is_executable(non_existent_path));
    }

    #[test]
    fn test_find_existing_file_by_stem() {
        let dir = tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("example.txt");

        File::create(&file_path).expect("Failed to create file");

        let result = find_file_by_stem("example", dir.path());
        assert_eq!(result, Some(file_path));
    }

    #[test]
    fn test_find_nonexistent_file_by_stem() {
        let dir = tempdir().expect("Failed to create temp dir");

        let result = find_file_by_stem("nonexistent", dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_find_file_with_different_stem() {
        let dir = tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("example.txt");

        File::create(file_path).expect("Failed to create file");

        let result = find_file_by_stem("different_stem", dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_find_multiple_files_same_stem() {
        let dir = tempdir().expect("Failed to create temp dir");
        let file1 = dir.path().join("example.txt");
        let file2 = dir.path().join("example.log");

        File::create(&file1).expect("Failed to create file1");
        File::create(&file2).expect("Failed to create file2");

        let result = find_file_by_stem("example", dir.path());
        assert!(result == Some(file1) || result == Some(file2));
    }

    #[test]
    fn test_find_ignores_directories() {
        let dir = tempdir().expect("Failed to create temp dir");
        let subdir = dir.path().join("example");

        std::fs::create_dir(subdir).expect("Failed to create subdirectory");

        let result = find_file_by_stem("example", dir.path());
        assert!(result.is_none());
    }
}
