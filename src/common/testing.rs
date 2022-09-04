use core::sync::atomic::{AtomicUsize, Ordering};
use std::fs;
use std::io::Write;

// Using this in tests allows them to execute concurrently with a clean repo in each case.
static GLOBAL_REPO_ID: AtomicUsize = AtomicUsize::new(0);

// Git utils
pub fn repo_path(n: usize) -> String {
    format!("/tmp/monorail/test-{}.git", n)
}
pub fn get_repo(bare: bool) -> (git2::Repository, String) {
    let id = GLOBAL_REPO_ID.fetch_add(1, Ordering::SeqCst);
    let repo_path = repo_path(id);
    let p = std::path::Path::new(&repo_path);
    if p.exists() {
        purge_repo(&repo_path);
    }
    if bare {
        return (git2::Repository::init_bare(p).unwrap(), repo_path);
    }
    (git2::Repository::init(p).unwrap(), repo_path)
}
pub fn purge_repo(path: &str) {
    let p = std::path::Path::new(path);
    std::fs::remove_dir_all(p).unwrap_or(());
}

pub fn get_signature() -> git2::Signature<'static> {
    git2::Signature::now("test", "test@foo.com").unwrap()
}

pub fn get_tree(repo: &git2::Repository) -> git2::Tree {
    let tree_oid = repo.index().unwrap().write_tree().unwrap();
    repo.find_tree(tree_oid).unwrap()
}

pub fn create_commit(
    repo: &git2::Repository,
    tree: &git2::Tree,
    message: &str,
    update_ref: Option<&str>,
    parents: &[&git2::Commit<'_>],
) -> git2::Oid {
    repo.commit(
        update_ref,
        &get_signature(),
        &get_signature(),
        message,
        tree,
        parents,
    )
    .unwrap()
}

pub fn get_commit(repo: &git2::Repository, oid: git2::Oid) -> git2::Commit<'_> {
    repo.find_commit(oid).unwrap()
}
pub fn commit_file(
    repo: &git2::Repository,
    file_name: &str,
    update_ref: Option<&str>,
    parents: &[&git2::Commit<'_>],
) -> git2::Oid {
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new(file_name)).unwrap();
    index.write_tree().unwrap();
    create_commit(repo, &get_tree(repo), "b", update_ref, parents)
}
pub fn create_file(repo_path: &str, dir: &str, file_name: &str, content: &[u8]) -> std::fs::File {
    fs::create_dir_all(std::path::Path::new(repo_path).join(dir)).unwrap();
    let fpath = std::path::Path::new(repo_path).join(dir).join(file_name);
    let mut file = std::fs::File::create(&fpath).unwrap();
    file.write_all(content).unwrap();
    file
}
