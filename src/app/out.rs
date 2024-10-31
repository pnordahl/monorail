use crate::core::error::MonorailError;
use serde::Serialize;
use std::path;

#[derive(Debug)]
pub(crate) struct OutDeleteInput {
    pub(crate) all: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct OutDeleteOutput {
    recovered_mb: f64,
}
pub(crate) fn out_delete(
    output_dir: &str,
    input: &OutDeleteInput,
) -> Result<OutDeleteOutput, MonorailError> {
    let recovered_mb = calculate_dir_size_in_mb(output_dir)?;
    if input.all {
        let tlds = get_tlds(output_dir)?;
        for d in tlds.iter() {
            std::fs::remove_dir_all(d)?;
        }
    }

    Ok(OutDeleteOutput { recovered_mb })
}

// Calculate the dir size of all files nested at any depth of path
fn calculate_dir_size_in_mb<P: AsRef<path::Path>>(p: P) -> Result<f64, MonorailError> {
    let mut total_size: u64 = 0;
    let mut stack = vec![p.as_ref().to_path_buf()];

    while let Some(current_path) = stack.pop() {
        for entry in std::fs::read_dir(current_path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;

            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                total_size += metadata.len();
            }
        }
    }

    Ok(total_size as f64 / (1048576.0)) // mb
}

// Get the names of all top-level directories in path
fn get_tlds<P: AsRef<path::Path>>(p: P) -> Result<Vec<path::PathBuf>, MonorailError> {
    let mut directories = Vec::new();

    for entry in std::fs::read_dir(p)? {
        let entry = entry?;
        let metadata = entry.metadata()?;

        if metadata.is_dir() {
            directories.push(entry.path());
        }
    }

    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_get_tlds_empty_directory() -> Result<(), MonorailError> {
        let dir = tempdir()?; // Create a temporary directory

        // Run the function and verify it returns an empty vector for an empty directory
        let top_level_dirs = get_tlds(dir.path())?;
        assert!(top_level_dirs.is_empty());

        Ok(())
    }

    #[test]
    fn test_get_tlds_with_only_files() -> Result<(), MonorailError> {
        let dir = tempdir()?;

        // Create some files in the top-level directory
        File::create(dir.path().join("file1.txt"))?;
        File::create(dir.path().join("file2.txt"))?;

        // Run the function and verify it returns an empty vector when only files are present
        let top_level_dirs = get_tlds(dir.path())?;
        assert!(top_level_dirs.is_empty());

        Ok(())
    }

    #[test]
    fn test_get_tlds_with_subdirectories() -> Result<(), MonorailError> {
        let dir = tempdir()?;

        // Create some subdirectories in the top-level directory
        let subdir1 = dir.path().join("subdir1");
        let subdir2 = dir.path().join("subdir2");
        fs::create_dir(&subdir1)?;
        fs::create_dir(&subdir2)?;

        // Run the function and verify the directories are found
        let mut top_level_dirs = get_tlds(dir.path())?;
        top_level_dirs.sort();

        assert_eq!(top_level_dirs, vec![subdir1, subdir2]);

        Ok(())
    }

    #[test]
    fn test_get_tlds_with_mixed_files_and_directories() -> Result<(), MonorailError> {
        let dir = tempdir()?;

        // Create a mix of files and directories
        let subdir1 = dir.path().join("subdir1");
        let subdir2 = dir.path().join("subdir2");
        let file1 = dir.path().join("file1.txt");
        let file2 = dir.path().join("file2.txt");

        fs::create_dir(&subdir1)?;
        fs::create_dir(&subdir2)?;
        File::create(file1)?;
        File::create(file2)?;

        // Run the function and verify it returns only the directories
        let mut top_level_dirs = get_tlds(dir.path())?;
        top_level_dirs.sort();

        assert_eq!(top_level_dirs, vec![subdir1, subdir2]);

        Ok(())
    }

    #[test]
    fn test_size_empty_directory() -> Result<(), MonorailError> {
        let dir = tempdir()?;
        let size_in_mb = calculate_dir_size_in_mb(dir.path())?;
        assert_eq!(size_in_mb, 0.0);
        Ok(())
    }

    #[test]
    fn test_size_single_file() -> Result<(), MonorailError> {
        let dir = tempdir()?;
        let file_path = dir.path().join("file.txt");

        // Create a 1 MB file
        let mut file = File::create(file_path)?;
        file.write_all(&vec![0u8; 1_048_576])?;

        let size_in_mb = calculate_dir_size_in_mb(dir.path())?;
        assert!((size_in_mb - 1.0).abs() < f64::EPSILON); // Ensure it's approximately 1 MB
        Ok(())
    }

    #[test]
    fn test_size_multiple_files() -> Result<(), MonorailError> {
        let dir = tempdir()?;

        // Create multiple files with varying sizes
        let file1 = dir.path().join("file1.txt");
        let file2 = dir.path().join("file2.txt");
        let file3 = dir.path().join("file3.txt");

        File::create(file1)?.write_all(&vec![0u8; 524_288])?; // 512 KB
        File::create(file2)?.write_all(&vec![0u8; 262_144])?; // 256 KB
        File::create(file3)?.write_all(&vec![0u8; 262_144])?; // 256 KB

        let size_in_mb = calculate_dir_size_in_mb(dir.path())?;
        assert!((size_in_mb - 1.0).abs() < f64::EPSILON); // Should be approximately 1 MB
        Ok(())
    }

    #[test]
    fn test_size_nested_directories() -> Result<(), MonorailError> {
        let dir = tempdir()?;

        // Create nested directories and files
        let sub_dir = dir.path().join("subdir");
        fs::create_dir(&sub_dir)?;

        let file1 = dir.path().join("file1.txt");
        let file2 = sub_dir.join("file2.txt");

        File::create(file1)?.write_all(&vec![0u8; 524_288])?; // 512 KB
        File::create(file2)?.write_all(&vec![0u8; 524_288])?; // 512 KB

        let size_in_mb = calculate_dir_size_in_mb(dir.path())?;
        assert!((size_in_mb - 1.0).abs() < f64::EPSILON); // Should be approximately 1 MB
        Ok(())
    }

    #[test]
    fn test_size_large_directory() -> Result<(), MonorailError> {
        let dir = tempdir()?;

        // Create a file with a larger size (e.g., 5 MB)
        let file_path = dir.path().join("large_file.txt");
        File::create(file_path)?.write_all(&vec![0u8; 5 * 1_048_576])?;

        let size_in_mb = calculate_dir_size_in_mb(dir.path())?;
        assert!((size_in_mb - 5.0).abs() < f64::EPSILON); // Should be approximately 5 MB
        Ok(())
    }
}
