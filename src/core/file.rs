use std::collections::{HashMap, VecDeque};
#[cfg(not(target_os = "windows"))]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "windows")]
use std::os::windows::fs::MetadataExt;

use std::path;
use std::result::Result;

use sha2::Digest;
use tokio::io::AsyncReadExt;
use tracing::debug;

use crate::core::error::MonorailError;

pub(crate) fn get_stem(p: &path::Path) -> Result<&str, MonorailError> {
    p.file_stem()
        .ok_or(MonorailError::Generic(format!(
            "Path {} has no stem",
            p.display()
        )))?
        .to_str()
        .ok_or(MonorailError::Generic(format!(
            "Path {} stem is empty",
            p.display()
        )))
}

pub(crate) fn contains_file(p: &path::Path) -> Result<(), MonorailError> {
    if p.is_file() {
        return Ok(());
    }
    // we also require that there be at least one file in it, because
    // many other processes require non-empty directories to be correct
    let mut stack = VecDeque::new();
    stack.push_back(p.to_path_buf());
    while let Some(current_path) = stack.pop_front() {
        for entry in std::fs::read_dir(current_path)?.flatten().by_ref() {
            let ep = entry.path();
            if ep.is_file() {
                return Ok(());
            } else if ep.is_dir() {
                stack.push_back(ep);
            }
        }
    }

    Err(MonorailError::Generic(format!(
        "Directory {} has no files",
        &p.display()
    )))
}

// Open a file from the provided path, and compute its checksum
// TODO: allow hasher to be passed in?
// TODO: configure buffer size based on file size?
// TODO: pass in open file instead of path?
pub(crate) async fn get_file_checksum(p: &path::Path) -> Result<String, MonorailError> {
    let md = match tokio::fs::metadata(p).await {
        Ok(md) => md,
        Err(_) => {
            // non-existent/failed to stat files have an empty checksum
            return Ok(String::new());
        }
    };
    let mut file = match tokio::fs::File::open(p).await {
        Ok(file) => file,
        Err(e) => {
            // TODO: once io::ErrorKind::IsADirectory, use that
            // empty directories have no checksum; this is required here
            // because opening a normal directory would return an error
            if md.is_dir() {
                return Ok(String::new());
            }
            return Err(MonorailError::from(e));
        }
    };

    // check for symlink directory; this is because File::open won't fail to
    // open the symlink, but attempting to read it will fail
    if md.is_dir() {
        return Ok(String::new());
    }

    // hash the file
    let mut hasher = sha2::Sha256::new();

    let mut buffer = [0u8; 64 * 1024];
    loop {
        let num = file.read(&mut buffer).await?;
        if num == 0 {
            break;
        }

        hasher.update(&buffer[..num]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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
        "File stem search"
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
    #[cfg(not(target_os = "windows"))]
    if let Ok(metadata) = std::fs::metadata(p) {
        let permissions = metadata.permissions();
        return permissions.mode() & 0o111 != 0;
    }
    #[cfg(target_os = "windows")]
    if let Some(extension) = p.extension() {
        match extension.to_str().unwrap_or("").to_lowercase().as_str() {
            "exe" | "bat" | "cmd" | "com" | "msi" => return true,
            _ => return false,
        }
    }
    false
}

pub(crate) fn permissions(p: impl AsRef<path::Path>) -> Option<String> {
    match std::fs::metadata(p) {
        Ok(md) => {
            #[cfg(target_os = "windows")]
            return Some(format!("{:o}", md.file_attributes()));
            #[cfg(not(target_os = "windows"))]
            return Some(format!("{:o}", md.permissions().mode() & 0o777));
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;
    use std::fs::{self, set_permissions, File};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_contains_file_with_file() {
        let temp_dir = new_testdir().unwrap();
        let file_path = temp_dir.path().join("test_file.txt");
        fs::write(&file_path, "Hello, world!").unwrap();

        // Should succeed because the path points to a file
        assert!(contains_file(&file_path).is_ok());
    }

    #[test]
    fn test_contains_file_with_directory_containing_file() {
        let temp_dir = new_testdir().unwrap();
        let file_path = temp_dir.path().join("test_file.txt");
        fs::write(file_path, "Hello, world!").unwrap();

        // Should succeed because the directory contains a file
        assert!(contains_file(temp_dir.path()).is_ok());
    }

    #[test]
    fn test_contains_file_with_empty_directory() {
        let temp_dir = new_testdir().unwrap();
        assert!(contains_file(temp_dir.path()).is_err());
    }

    #[test]
    fn test_contains_file_with_nested_directory_with_file() {
        let temp_dir = new_testdir().unwrap();
        let nested_dir = temp_dir.path().join("nested");
        fs::create_dir(&nested_dir).unwrap();
        let nested_file_path = nested_dir.join("nested_file.txt");
        fs::write(nested_file_path, "Nested file content").unwrap();

        // Should succeed because a nested directory contains a file
        assert!(contains_file(temp_dir.path()).is_ok());
    }

    #[test]
    fn test_contains_file_with_nested_empty_directories() {
        let temp_dir = new_testdir().unwrap();
        let nested_dir = temp_dir.path().join("nested");
        fs::create_dir(nested_dir).unwrap();

        // Should fail because the directory and nested directory are empty
        assert!(contains_file(temp_dir.path()).is_err());
    }

    #[test]
    fn test_contains_file_with_multiple_directories_one_with_file() {
        let temp_dir = new_testdir().unwrap();
        let dir1 = temp_dir.path().join("dir1");
        let dir2 = temp_dir.path().join("dir2");
        fs::create_dir(dir1).unwrap();
        fs::create_dir(&dir2).unwrap();
        let file_path = dir2.join("file_in_dir2.txt");
        fs::write(file_path, "Some content").unwrap();

        // Should succeed because dir2 contains a file
        assert!(contains_file(temp_dir.path()).is_ok());
    }

    #[tokio::test]
    async fn test_checksum_is_equal() {
        let td = new_testdir().unwrap();
        let repo_path = &td.path();
        init(repo_path, false).await;
        let fname1 = "test1.txt";

        let root_path = &repo_path;
        let pending = get_pair_map(&[(
            fname1,
            write_with_checksum(&root_path.join(fname1), &[1, 2, 3])
                .await
                .unwrap(),
        )]);

        // checksums must match
        assert!(checksum_is_equal(&pending, repo_path, fname1).await);
        // file error (such as dne) interpreted as checksum mismatch
        assert!(!checksum_is_equal(&pending, repo_path, "dne.txt").await);

        // write a file and use a pending entry with a mismatched checksum
        let fname2 = "test2.txt";
        tokio::fs::write(root_path.join(fname2), &[1])
            .await
            .unwrap();
        let pending2 = get_pair_map(&[(fname2, "foobar".into())]);
        // checksums don't match
        assert!(!checksum_is_equal(&pending2, repo_path, fname2).await);
    }

    #[tokio::test]
    async fn test_get_file_checksum() {
        let td = new_testdir().unwrap();
        let repo_path = &td.path();
        init(repo_path, false).await;

        // files that don't exist have an empty checksum
        let p = &repo_path.join("test.txt");
        assert_eq!(get_file_checksum(p).await.unwrap(), String::new());

        // write file and compare checksums
        let checksum = write_with_checksum(p, &[1, 2, 3]).await.unwrap();
        assert_eq!(get_file_checksum(p).await.unwrap(), checksum);

        // empty directories have an empty checksum
        tokio::fs::create_dir_all(repo_path.join("link"))
            .await
            .unwrap();
        let p = &repo_path.join("link");
        assert_eq!(get_file_checksum(p).await.unwrap(), String::new());
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
