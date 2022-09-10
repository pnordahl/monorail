use monorail::common::testing::*;
use monorail::*;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const CONFIG_BASH: &'static str = r#"
[vcs]
use = "git"

[vcs.git]
trunk = "master"
remote = "origin"

[extension]
use = "bash"

[[group]]
path = "group1"

	[[group.project]]
	path = "project1"

	[[group.project]]
	path = "project1/x"

    [[group.project]]
    path = "project2"
"#;

#[test]
fn test_handle_git_release_no_targets_noop() {
    let (repo, repo_path) = get_repo(false);
    let (_origin, origin_repo_path) = get_repo(true);
    repo.remote("origin", &origin_repo_path).unwrap();
    repo.remote_add_push("origin", "refs/tags:refs/tags")
        .unwrap();

    // generate initial changeset
    let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
    let oid2 = create_commit(
        &repo,
        &get_tree(&repo),
        "b",
        Some("HEAD"),
        &vec![&get_commit(&repo, oid1)],
    );

    let c: Config = toml::from_str(CONFIG_BASH).unwrap();
    let result = handle_release(
        &c,
        HandleReleaseInput {
            git_path: "git",
            use_libgit2_status: false,
            release_type: "patch",
            dry_run: false,
        },
        &repo_path,
    );
    assert!(result.is_ok());
    assert_eq!(result.unwrap().id, "");

    // ensure no tag was created
    assert!(libgit2_latest_tag(&repo, oid2).unwrap().is_none());
}

#[test]
fn test_handle_git_release_err_not_trunk() {
    let (repo, repo_path) = get_repo(false);
    let (_origin, origin_repo_path) = get_repo(true);
    repo.remote("origin", &origin_repo_path).unwrap();
    repo.remote_add_push("origin", "refs/tags:refs/tags")
        .unwrap();

    // generate initial changeset
    let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
    let oid2 = create_commit(
        &repo,
        &get_tree(&repo),
        "a",
        Some("HEAD"),
        &vec![&get_commit(&repo, oid1)],
    );

    let _branch = repo
        .branch("baz", &repo.find_commit(oid2).unwrap(), false)
        .unwrap();
    repo.checkout_tree(
        &repo.find_object(oid2, Some(git2::ObjectType::Any)).unwrap(),
        Some(&mut git2::build::CheckoutBuilder::new()),
    )
    .unwrap();
    repo.set_head("refs/heads/baz").unwrap();

    let c: Config = toml::from_str(CONFIG_BASH).unwrap();
    let result = handle_release(
        &c,
        HandleReleaseInput {
            git_path: "git",
            use_libgit2_status: false,
            release_type: "patch",
            dry_run: true,
        },
        &repo_path,
    );
    assert!(result.is_err());
}

#[test]
fn test_handle_git_release() {
    let (repo, repo_path) = get_repo(false);
    let (_origin, origin_repo_path) = get_repo(true);
    repo.remote("origin", &origin_repo_path).unwrap();
    repo.remote_add_push("origin", "refs/tags:refs/tags")
        .unwrap();

    // generate initial changeset for use in first release
    let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
    let _f1 = create_file(&repo_path, "group1/project1/x", "foo.txt", b"x");
    let oid2 = commit_file(
        &repo,
        "group1/project1/x/foo.txt",
        Some("HEAD"),
        &[&get_commit(&repo, oid1)],
    );

    let c: Config = toml::from_str(CONFIG_BASH).unwrap();

    // dry run release
    let o = handle_release(
        &c,
        HandleReleaseInput {
            git_path: "git",
            use_libgit2_status: false,
            release_type: "patch",
            dry_run: true,
        },
        &repo_path,
    )
    .unwrap();
    assert_eq!(
        o.targets,
        vec!["group1/project1".to_string(), "group1/project1/x".to_string()]
    );
    assert_eq!(o.id, "v0.0.1".to_string());
    assert_eq!(o.dry_run, true);

    // actual release
    let o = handle_release(
        &c,
        HandleReleaseInput {
            git_path: "git",
            use_libgit2_status: false,
            release_type: "patch",
            dry_run: false,
        },
        &repo_path,
    )
    .unwrap();
    assert_eq!(
        o.targets,
        vec!["group1/project1".to_string(), "group1/project1/x".to_string()]
    );
    assert_eq!(o.id, "v0.0.1".to_string());
    assert_eq!(o.dry_run, false);

    // first tag in the repo
    let lt = libgit2_latest_tag(&repo, oid2).unwrap().unwrap();
    assert_eq!(lt.name().unwrap(), "v0.0.1");
    assert_eq!(lt.message().unwrap(), "group1/project1\ngroup1/project1/x");

    // generate another changeset to test a second release
    let _f2 = create_file(&repo_path, "group1/project1/x", "bar.txt", b"x");
    let oid3 = commit_file(
        &repo,
        "group1/project1/x/bar.txt",
        Some("HEAD"),
        &[&get_commit(&repo, oid2)],
    );
    let o2 = handle_release(
        &c,
        HandleReleaseInput {
            git_path: "git",
            use_libgit2_status: false,
            release_type: "minor",
            dry_run: false,
        },
        &repo_path,
    )
    .unwrap();

    assert_eq!(
        o2.targets,
        vec!["group1/project1".to_string(), "group1/project1/x".to_string()]
    );
    assert_eq!(o2.id, "v0.1.0".to_string());
    assert_eq!(o2.dry_run, false);

    // check subsequent tag exists
    let lt2 = libgit2_latest_tag(&repo, oid3).unwrap().unwrap();
    assert_eq!(lt2.name().unwrap(), "v0.1.0");
    assert_eq!(lt2.message().unwrap(), "group1/project1\ngroup1/project1/x");
}

// TODO: test_handle_git_release_err_latest_bad_semver
// TODO: test_handle_git_release_head_from_noop

#[test]
fn test_extension_bash_exec_implicit_target() {
    let (repo, repo_path) = get_repo(false);
    const SCRIPT: &'static str = r#"
#!/bin/bash
function call_me {
    echo 'called' > output.txt
}
"#;
    // TODO: shouldn't have to make initial repo commit to make HEAD resolve
    let _oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
    let _f1 = create_file(
        &repo_path,
        "group1/project1/support/script",
        "monorail-exec.sh",
        SCRIPT.as_bytes(),
    );
    let _f2 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
    let _output = std::process::Command::new("bash")
        .args(&[
            "ext/bash/monorail-bash.sh",
            "-v",
            "-d",
            &repo_path,
            "exec",
            "-m",
            Path::new(&std::env::current_dir().unwrap())
                .join("target/debug/monorail")
                .to_str()
                .unwrap(),
            "-c",
            "call_me",
        ])
        .output()
        .expect("failed to run monorail-bash");

    let mut file = File::open(Path::new(&repo_path).join("group1/project1/output.txt")).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "called\n");

    purge_repo(&repo_path);
}

#[test]
fn test_extension_bash_exec_explicit_target() {
    let (repo, repo_path) = get_repo(false);
    const SCRIPT: &'static str = r#"
#!/bin/bash
function call_me {
    echo 'called' > output.txt
}
"#;
    const SCRIPT2: &'static str = r#"
#!/bin/bash
function call_me {
    touch should_not_exist.txt
}
"#;
    // TODO: shouldn't have to make initial repo commit to make HEAD resolve
    let _oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
    let _f1 = create_file(
        &repo_path,
        "group1/project1/support/script",
        "monorail-exec.sh",
        SCRIPT.as_bytes(),
    );
    let _f2 = create_file(
        &repo_path,
        "group1/project2/support/script",
        "monorail-exec.sh",
        SCRIPT2.as_bytes(),
    );
    let _f3 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
    let _output = std::process::Command::new("bash")
        .args(&[
            "ext/bash/monorail-bash.sh",
            "-v",
            "-d",
            &repo_path,
            "exec",
            "-m",
            Path::new(&std::env::current_dir().unwrap())
                .join("target/debug/monorail")
                .to_str()
                .unwrap(),
            "-t",
            "group1/project1",
            "-c",
            "call_me",
        ])
        .output()
        .expect("failed to run monorail-bash");
    let mut file = File::open(Path::new(&repo_path).join("group1/project1/output.txt")).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "called\n");

    assert!(!Path::new(&repo_path)
        .join("group1/project2/should_not_exist.txt")
        .exists());

    purge_repo(&repo_path);
}

#[test]
fn test_extension_bash_exec_command_err() {
    let (repo, repo_path) = get_repo(false);
    const SCRIPT: &'static str = r#"
#!/bin/bash
function call_me {
    exit 1
}
"#;
    // TODO: shouldn't have to make initial repo commit to make HEAD resolve
    let _oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
    let _f1 = create_file(
        &repo_path,
        "group1/project1/support/script",
        "monorail-exec.sh",
        SCRIPT.as_bytes(),
    );
    let _f2 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
    let output = std::process::Command::new("bash")
        .args(&[
            "ext/bash/monorail-bash.sh",
            "-v",
            "-d",
            &repo_path,
            "exec",
            "-m",
            Path::new(&std::env::current_dir().unwrap())
                .join("target/debug/monorail")
                .to_str()
                .unwrap(),
            "-c",
            "call_me",
        ])
        .output()
        .expect("failed to run monorail-bash");
    assert!(!output.status.success());

    let output = std::process::Command::new("bash")
        .args(&[
            "ext/bash/monorail-bash.sh",
            "-v",
            "-d",
            &repo_path,
            "exec",
            "-m",
            Path::new(&std::env::current_dir().unwrap())
                .join("target/debug/monorail")
                .to_str()
                .unwrap(),
            "-c",
            "call_me",
        ])
        .output()
        .expect("failed to run monorail-bash");
    assert!(!output.status.success());

    purge_repo(&repo_path);
}

#[test]
fn test_handle_inspect_change() {
    let (repo, repo_path) = get_repo(false);

    // prep repo with some changes
    let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
    let _f1 = create_file(&repo_path, "group1/project1/x", "foo.txt", b"x");
    let _oid2 = commit_file(
        &repo,
        "group1/project1/x/foo.txt",
        Some("HEAD"),
        &[&get_commit(&repo, oid1)],
    );

    let c: Config = toml::from_str(CONFIG_BASH).unwrap();
    let o = handle_inspect_change(
        &c,
        HandleInspectChangeInput {
            start: None,
            end: Some("HEAD"),
            git_path: "git",
            use_libgit2_status: false,
        },
        &repo_path,
    )
    .unwrap();

    let gc = &o.group.get("group1").unwrap().change;

    let gcf = gc.file.get(0).unwrap();
    assert_eq!(gcf.name, "group1/project1/x/foo.txt".to_string());
    assert_eq!(gcf.target, Some("group1/project1".into()));
    assert_eq!(gcf.action, FileActionKind::Use);
    assert_eq!(gcf.reason, FileReasonKind::TargetMatch);

    let gcf = gc.file.get(1).unwrap();
    assert_eq!(gcf.name, "group1/project1/x/foo.txt".to_string());
    assert_eq!(gcf.target, Some("group1/project1/x".into()));
    assert_eq!(gcf.action, FileActionKind::Use);
    assert_eq!(gcf.reason, FileReasonKind::TargetMatch);

    assert!(gc.project.contains("group1/project1"));
    assert!(gc.project.contains("group1/project1/x"));
    assert!(gc.link.is_empty());
    assert!(gc.depend.is_empty());

    purge_repo(&repo_path);
}
