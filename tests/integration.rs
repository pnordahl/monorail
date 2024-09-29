// use monorail::common::testing::*;
// // use monorail::*;
// use monorail::libgit2_latest_monorail_tag;
// use std::fs::File;
// use std::io::Read;
// use std::path::Path;
// use std::process;

// const CONFIG_BASH: &str = r#"
// [[targets]]
// path = "group1"

// [[targets]]
// path = "group1/project1"

// [[targets]]
// path = "group1/project1/x"

// [[targets]]
// path = "group1/project2"

// "#;

// fn prep_integration_repo() -> (git2::Repository, String) {
//     let (repo, repo_path) = get_repo(false);
//     let _f2 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
//     create_file(
//         &repo_path,
//         "group1",
//         "monorail.sh",
//         b"function whoami { echo 'group1' }\n function err { exit 1 }",
//     );
//     create_file(
//         &repo_path,
//         "group1/project1",
//         "monorail.sh",
//         b"function whoami { echo 'group1/project1' }\n function err { exit 1 }",
//     );
//     create_file(
//         &repo_path,
//         "group1/project1/x",
//         "monorail.sh",
//         b"function whoami { echo 'group1/project1/x' }\n function err { exit 1 }",
//     );
//     create_file(
//         &repo_path,
//         "group1/project2",
//         "monorail.sh",
//         b"function whoami { echo 'group1/project2' }\n function err { exit 1 }",
//     );
//     (repo, repo_path)
// }

// #[test]
// fn test_handle_git_checkpoint_no_changes_noop() {
//     let (repo, repo_path) = prep_integration_repo();
//     let (_origin, origin_repo_path) = get_repo(true);
//     repo.remote("origin", &origin_repo_path).unwrap();
//     repo.remote_add_push("origin", "refs/tags:refs/tags")
//         .unwrap();

//     // generate initial changeset
//     let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let oid2 = create_commit(
//         &repo,
//         &get_tree(&repo),
//         "b",
//         Some("HEAD"),
//         &[&get_commit(&repo, oid1)],
//     );

//     let monorail_toml = "/tmp/monorail/Monorail.toml";
//     let _f2 = create_file(&repo_path, "", monorail_toml, CONFIG_BASH.as_bytes());
//     let output = process::Command::new("target/debug/monorail")
//         .args([
//             "-w",
//             &repo_path,
//             "-c",
//             monorail_toml,
//             "checkpoint",
//             "create",
//         ])
//         .output()
//         .expect("command failed");

//     dbg!(std::str::from_utf8(output.stdout.as_slice()).unwrap());
//     assert_eq!(std::str::from_utf8(output.stderr.as_slice()).unwrap(), "{\"class\":\"generic\",\"message\":\"No changes detected, aborting checkpoint creation\"}\n");

//     // ensure no tag was created
//     assert!(libgit2_latest_monorail_tag(&repo, oid2).unwrap().is_none());
// }

// #[test]
// fn test_handle_git_checkpoint_no_targets() {
//     let (repo, repo_path) = get_repo(false);
//     let (_origin, origin_repo_path) = get_repo(true);
//     repo.remote("origin", &origin_repo_path).unwrap();
//     repo.remote_add_push("origin", "refs/tags:refs/tags")
//         .unwrap();

//     // generate initial changeset
//     let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let oid2 = create_commit(
//         &repo,
//         &get_tree(&repo),
//         "b",
//         Some("HEAD"),
//         &[&get_commit(&repo, oid1)],
//     );

//     let _f2 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
//     let output = process::Command::new("target/debug/monorail")
//         .args(["-w", &repo_path, "checkpoint", "create"])
//         .output()
//         .expect("command failed");

//     assert_eq!(
//         std::str::from_utf8(output.stdout.as_slice()).unwrap(),
//         "{\"id\":\"monorail-1\",\"targets\":[],\"dry_run\":false}\n"
//     );

//     // ensure tag was created
//     assert!(libgit2_latest_monorail_tag(&repo, oid2).unwrap().is_some());
// }

// #[test]
// fn test_handle_git_checkpoint_err_not_trunk() {
//     let (repo, repo_path) = prep_integration_repo();
//     let (_origin, origin_repo_path) = get_repo(true);
//     repo.remote("origin", &origin_repo_path).unwrap();
//     repo.remote_add_push("origin", "refs/tags:refs/tags")
//         .unwrap();

//     // generate initial changeset
//     let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let oid2 = create_commit(
//         &repo,
//         &get_tree(&repo),
//         "a",
//         Some("HEAD"),
//         &[&get_commit(&repo, oid1)],
//     );

//     let _branch = repo
//         .branch("baz", &repo.find_commit(oid2).unwrap(), false)
//         .unwrap();
//     repo.checkout_tree(
//         &repo.find_object(oid2, Some(git2::ObjectType::Any)).unwrap(),
//         Some(&mut git2::build::CheckoutBuilder::new()),
//     )
//     .unwrap();
//     repo.set_head("refs/heads/baz").unwrap();

//     let _f2 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
//     let output = process::Command::new("target/debug/monorail")
//         .args(["-w", &repo_path, "checkpoint", "create"])
//         .output()
//         .expect("command failed");

//     assert_eq!(std::str::from_utf8(output.stderr.as_slice()).unwrap(), "{\"class\":\"generic\",\"message\":\"HEAD points to refs/heads/baz expected vcs.git.trunk branch master\"}\n");
// }

// #[test]
// fn test_handle_git_checkpoint_err_tag_push() {
//     let bad_remote_config = r#"
// [vcs.git]
// remote = "wrong"

// [[targets]]
// path = "group1"

// [[targets]]
// path = "group1/project1"

// [[targets]]
// path = "group1/project1/x"

// [[targets]]
// path = "group1/project2"

// "#;

//     let (repo, repo_path) = prep_integration_repo();
//     let (_origin, origin_repo_path) = get_repo(true);
//     repo.remote("origin", &origin_repo_path).unwrap();
//     repo.remote_add_push("origin", "refs/tags:refs/tags")
//         .unwrap();

//     // generate initial changeset
//     let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let oid2 = create_commit(
//         &repo,
//         &get_tree(&repo),
//         "b",
//         Some("HEAD"),
//         &[&get_commit(&repo, oid1)],
//     );

//     let output = process::Command::new("target/debug/monorail")
//         .args(["-w", &repo_path, "checkpoint", "create"])
//         .output()
//         .expect("command failed");

//     assert_eq!(std::str::from_utf8(output.stderr.as_slice()).unwrap(), "{\"class\":\"generic\",\"message\":\"failed to push tags: fatal: 'wrong' does not appear to be a git repository\\nfatal: Could not read from remote repository.\\n\\nPlease make sure you have the correct access rights\\nand the repository exists.\\n\"}\n");

//     // ensure tag was not created
//     assert!(libgit2_latest_monorail_tag(&repo, oid2).unwrap().is_none());
// }

// #[test]
// fn test_git_checkpoint() {
//     let (repo, repo_path) = prep_integration_repo();
//     let (_origin, origin_repo_path) = get_repo(true);
//     repo.remote("origin", &origin_repo_path).unwrap();
//     repo.remote_add_push("origin", "refs/tags:refs/tags")
//         .unwrap();

//     // generate initial changeset for use in first checkpoint
//     let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let _f1 = create_file(&repo_path, "group1/project1/x", "foo.txt", b"x");
//     let oid2 = commit_file(
//         &repo,
//         "group1/project1/x/foo.txt",
//         Some("HEAD"),
//         &[&get_commit(&repo, oid1)],
//     );

//     // dry run checkpoint
//     let output = process::Command::new("target/debug/monorail")
//         .args(["-w", &repo_path, "checkpoint", "create", "--dry-run"])
//         .output()
//         .expect("command failed");

//     assert_eq!(std::str::from_utf8(output.stdout.as_slice()).unwrap(), "{\"id\":\"monorail-1\",\"targets\":[\"group1\",\"group1/project1\",\"group1/project1/x\",\"group1/project2\"],\"dry_run\":true}\n");

//     // actual checkpoint
//     let output = process::Command::new("target/debug/monorail")
//         .args(["-w", &repo_path, "checkpoint", "create"])
//         .output()
//         .expect("command failed");

//     assert_eq!(std::str::from_utf8(output.stdout.as_slice()).unwrap(), "{\"id\":\"monorail-1\",\"targets\":[\"group1\",\"group1/project1\",\"group1/project1/x\",\"group1/project2\"],\"dry_run\":false}\n");

//     // first tag in the repo
//     let lt = libgit2_latest_monorail_tag(&repo, oid2).unwrap().unwrap();
//     assert_eq!(lt.name().unwrap(), "monorail-1");
//     assert_eq!(
//         lt.message().unwrap(),
//         "{\n  \"num_changes\": 6,\n  \"targets\": [\n    \"group1\",\n    \"group1/project1\",\n    \"group1/project1/x\",\n    \"group1/project2\"\n  ]\n}\n"
//     );

//     // generate another changeset to test a second checkpoint
//     let _f2 = create_file(&repo_path, "group1/project1/x", "bar.txt", b"x");
//     let oid3 = commit_file(
//         &repo,
//         "group1/project1/x/bar.txt",
//         Some("HEAD"),
//         &[&get_commit(&repo, oid2)],
//     );
//     let output = process::Command::new("target/debug/monorail")
//         .args(["-w", &repo_path, "checkpoint", "create"])
//         .output()
//         .expect("command failed");

//     assert_eq!(std::str::from_utf8(output.stdout.as_slice()).unwrap(), "{\"id\":\"monorail-2\",\"targets\":[\"group1\",\"group1/project1\",\"group1/project1/x\",\"group1/project2\"],\"dry_run\":false}\n");

//     // check subsequent tag exists
//     let lt2 = libgit2_latest_monorail_tag(&repo, oid3).unwrap().unwrap();
//     assert_eq!(lt2.name().unwrap(), "monorail-2");
//     assert_eq!(
//         lt2.message().unwrap(),
//         "{\n  \"num_changes\": 7,\n  \"targets\": [\n    \"group1\",\n    \"group1/project1\",\n    \"group1/project1/x\",\n    \"group1/project2\"\n  ]\n}\n"
//     );
// }

// // TODO: test_handle_git_checkpoint_err_latest_bad_semver
// // TODO: test_handle_git_checkpoint_head_from_noop

// #[test]
// fn test_extension_bash_exec_implicit_target() {
//     let (repo, repo_path) = get_repo(false);
//     const SCRIPT: &str = r#"
// #!/bin/bash
// function call_me {
//     echo 'called' > output.txt
// }
// "#;
//     // TODO: shouldn't have to make initial repo commit to make HEAD resolve
//     let _oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let _f1 = create_file(
//         &repo_path,
//         "group1/project1/support/script",
//         "monorail-exec.sh",
//         SCRIPT.as_bytes(),
//     );
//     let _f2 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
//     let output = process::Command::new("bash")
//         .args([
//             "ext/bash/monorail-bash.sh",
//             "-v",
//             "-w",
//             &repo_path,
//             "exec",
//             "-m",
//             Path::new(&std::env::current_dir().unwrap())
//                 .join("target/debug/monorail")
//                 .to_str()
//                 .unwrap(),
//             "-c",
//             "call_me",
//         ])
//         .output()
//         .expect("failed to run monorail-bash");

//     match File::open(Path::new(&repo_path).join("group1/project1/output.txt")) {
//         Ok(mut file) => {
//             let mut contents = String::new();
//             file.read_to_string(&mut contents).unwrap();
//             assert_eq!(contents, "called\n");
//         }
//         Err(e) => {
//             panic!("{:?} {:?}", output, e);
//         }
//     }

//     purge_repo(&repo_path);
// }

// #[test]
// fn test_extension_bash_exec_explicit_target() {
//     let (repo, repo_path) = get_repo(false);
//     const SCRIPT: &str = r#"
// #!/bin/bash
// function call_me {
//     echo 'called' > output.txt
// }
// "#;
//     const SCRIPT2: &str = r#"
// #!/bin/bash
// function call_me {
//     touch should_not_exist.txt
// }
// "#;
//     // TODO: shouldn't have to make initial repo commit to make HEAD resolve
//     let _oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let _f1 = create_file(
//         &repo_path,
//         "group1/project1/support/script",
//         "monorail-exec.sh",
//         SCRIPT.as_bytes(),
//     );
//     let _f2 = create_file(
//         &repo_path,
//         "group1/project2/support/script",
//         "monorail-exec.sh",
//         SCRIPT2.as_bytes(),
//     );
//     let _f3 = create_file(&repo_path, "", "Monorail.toml", CONFIG_BASH.as_bytes());
//     let _output = process::Command::new("bash")
//         .args([
//             "ext/bash/monorail-bash.sh",
//             "-v",
//             "-w",
//             &repo_path,
//             "exec",
//             "-m",
//             Path::new(&std::env::current_dir().unwrap())
//                 .join("target/debug/monorail")
//                 .to_str()
//                 .unwrap(),
//             "-t",
//             "group1/project1",
//             "-c",
//             "call_me",
//         ])
//         .output()
//         .expect("failed to run monorail-bash");
//     let mut file = File::open(Path::new(&repo_path).join("group1/project1/output.txt")).unwrap();
//     let mut contents = String::new();
//     file.read_to_string(&mut contents).unwrap();
//     assert_eq!(contents, "called\n");

//     assert!(!Path::new(&repo_path)
//         .join("group1/project2/should_not_exist.txt")
//         .exists());

//     purge_repo(&repo_path);
// }

// #[test]
// fn test_run_err() {
//     let (repo, repo_path) = prep_integration_repo();

//     // TODO: shouldn't have to make initial repo commit to make HEAD resolve
//     let _oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);

//     let output = process::Command::new("monorail")
//         .args(["-w", &repo_path, "run", "-f", "err"])
//         .output()
//         .expect("failed to run function");
//     assert!(!output.status.success());

//     purge_repo(&repo_path);
// }

// #[test]
// fn test_handle_analyze() {
//     let (repo, repo_path) = prep_integration_repo();

//     // prep repo with some changes
//     let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
//     let _f1 = create_file(&repo_path, "group1/project1/x", "foo.txt", b"x");
//     let _oid2 = commit_file(
//         &repo,
//         "group1/project1/x/foo.txt",
//         Some("HEAD"),
//         &[&get_commit(&repo, oid1)],
//     );

//     let output = process::Command::new("target/debug/monorail")
//         .args(["-w", &repo_path, "analyze", "--show-changes"])
//         .output()
//         .expect("command failed");
//     assert_eq!(
//         std::str::from_utf8(output.stdout.as_slice()).unwrap(),
//         "{\"changes\":[{\"path\":\"Monorail.toml\"},{\"path\":\"group1/monorail.sh\"},{\"path\":\"group1/project1/monorail.sh\"},{\"path\":\"group1/project1/x/foo.txt\"},{\"path\":\"group1/project1/x/monorail.sh\"},{\"path\":\"group1/project2/monorail.sh\"}],\"targets\":[\"group1\",\"group1/project1\",\"group1/project1/x\",\"group1/project2\"]}\n"
//     );

//     purge_repo(&repo_path);
// }
