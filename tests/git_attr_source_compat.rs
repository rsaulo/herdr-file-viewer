//! #160: a git that rejects `--attr-source` (Apple Git 2.39.x) must still activate
//! git awareness without letting repository attributes/configuration execute filters.
//! Own integration binary so the process-wide probe cache starts unset and sees the
//! wrapper before any other test in this crate can pin it.
//!
//! Unix-only: the wrapper and hostile filter fixtures are shell scripts. Windows CI uses a
//! current Git for Windows (2.40+), so this Apple-git path is not the failure mode there.

#![cfg(unix)]

mod common;

use common::{TempDir, canon, git, init_repo_with_commit};
use herdr_file_viewer::context::LaunchContext;
use herdr_file_viewer::git::{Baseline, Status, current_branch, diff, status};
use herdr_file_viewer::root::resolve;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Locate the real `git` *before* we prepend a wrapper to `PATH`.
fn real_git_path() -> PathBuf {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .expect("resolve git on PATH");
    assert!(
        out.status.success(),
        "git must be on PATH: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
}

fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

/// A `git` that fails if `--attr-source` is present (Apple Git 2.39 behaviour) and
/// otherwise execs the real binary. The probe (`git --attr-source=… --version`) hits
/// this first, so the viewer takes its older-git compatibility path.
fn install_attr_source_rejecting_wrapper(dir: &Path, real_git: &Path) {
    let wrapper = dir.join("git");
    let script = format!(
        "#!/bin/sh\n\
         for arg in \"$@\"; do\n\
         case \"$arg\" in\n\
         --attr-source=*) exit 129 ;;\n\
         esac\n\
         done\n\
         exec {} \"$@\"\n",
        real_git.display()
    );
    fs::write(&wrapper, script).unwrap();
    make_executable(&wrapper);
}

fn install_hostile_filters(repo: &Path, payloads: &Path) -> (PathBuf, PathBuf) {
    let clean_marker = payloads.join("CLEAN_EXECUTED");
    let process_marker = payloads.join("PROCESS_EXECUTED");
    let clean_script = payloads.join("clean-filter.sh");
    let process_script = payloads.join("process-filter.sh");

    // The clean driver is a byte-for-byte pass-through; the process driver only marks and exits.
    // Both are configured as required below, so correct status/diff also proves the hardening
    // neutralized `required`, not merely the executable command.
    fs::write(
        &clean_script,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", clean_marker.display()),
    )
    .unwrap();
    fs::write(
        &process_script,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", process_marker.display()),
    )
    .unwrap();
    make_executable(&clean_script);
    make_executable(&process_script);

    fs::write(
        repo.join(".gitattributes"),
        "clean.txt filter=hostile-clean\nprocess.txt filter=hostile-process\n",
    )
    .unwrap();
    git(
        repo,
        &[
            "config",
            "filter.hostile-clean.clean",
            clean_script.to_str().unwrap(),
        ],
    );
    git(
        repo,
        &[
            "config",
            "filter.hostile-process.process",
            process_script.to_str().unwrap(),
        ],
    );
    git(repo, &["config", "filter.hostile-clean.required", "true"]);
    git(repo, &["config", "filter.hostile-process.required", "true"]);
    (clean_marker, process_marker)
}

/// Force Git past its stat-cache shortcut after a same-byte-length edit. This makes status
/// content-check both tracked paths and deterministically reaches clean/process conversion.
fn rewrite_with_distinct_mtime(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let touched = Command::new("touch")
        .args(["-t", "200001010000.00"])
        .arg(path)
        .status()
        .expect("run touch");
    assert!(touched.success(), "force fixture mtime");
}

/// Restores `PATH` on drop so a panic mid-test cannot leak the wrapper into later
/// commands in this process (this binary has only one test, but Drop is the honest cleanup).
struct RestorePath(OsString);

impl Drop for RestorePath {
    fn drop(&mut self) {
        // SAFETY: same process-wide PATH mutation as the install below; Drop runs
        // once, on this thread, after the viewer calls have finished.
        unsafe { std::env::set_var("PATH", &self.0) };
    }
}

#[test]
fn old_git_compat_keeps_awareness_and_diffs_without_executing_filters() {
    let repo = TempDir::new();
    init_repo_with_commit(repo.path());
    fs::write(repo.path().join("clean.txt"), "clean-a\n").unwrap();
    fs::write(repo.path().join("process.txt"), "process-a\n").unwrap();
    git(repo.path(), &["add", "clean.txt", "process.txt"]);
    git(repo.path(), &["commit", "-q", "-m", "tracked fixtures"]);

    let payloads = TempDir::new();
    let (clean_marker, process_marker) = install_hostile_filters(repo.path(), payloads.path());
    rewrite_with_distinct_mtime(&repo.path().join("clean.txt"), "clean-b\n");
    rewrite_with_distinct_mtime(&repo.path().join("process.txt"), "process-b\n");

    let real_git = real_git_path();
    let bin = TempDir::new();
    install_attr_source_rejecting_wrapper(bin.path(), &real_git);

    // Sanity: the wrapper itself rejects the flag the way Apple Git 2.39 does.
    let probe = Command::new(bin.path().join("git"))
        .args([format!("--attr-source={EMPTY_TREE}"), "--version".into()])
        .output()
        .expect("run wrapper probe");
    assert!(!probe.status.success(), "wrapper must reject --attr-source");

    let orig_path = std::env::var_os("PATH").expect("PATH is set");
    let mut prefixed = std::env::split_paths(&orig_path).collect::<Vec<_>>();
    prefixed.insert(0, bin.path().to_path_buf());
    let new_path = std::env::join_paths(prefixed).expect("join PATH");
    // SAFETY: this integration binary has one test that mutates PATH. `RestorePath`
    // puts the original back on drop. `Command::new("git")` in the viewer reads PATH
    // at spawn time, so the probe and every later query see the wrapper.
    unsafe { std::env::set_var("PATH", &new_path) };
    let _restore = RestorePath(orig_path);

    let resolved = resolve(&LaunchContext {
        cwd: repo.path().to_path_buf(),
        ..Default::default()
    });
    let map = status(repo.path());
    let branch = current_branch(repo.path());
    let clean_diff = diff(
        repo.path(),
        Path::new("clean.txt"),
        Baseline::Head,
        None,
        false,
    );
    let process_diff = diff(
        repo.path(),
        Path::new("process.txt"),
        Baseline::Head,
        None,
        false,
    );

    assert!(
        !clean_marker.exists(),
        "older-git compatibility must not execute a configured clean filter"
    );
    assert!(
        !process_marker.exists(),
        "older-git compatibility must not execute a configured process filter"
    );
    assert!(
        resolved.is_git_repo,
        "#160: a git that rejects --attr-source must still be detected as a repo"
    );
    assert_eq!(
        resolved.repo_root.as_ref().map(|p| canon(p)),
        Some(canon(repo.path()))
    );
    assert_eq!(map.get(Path::new("clean.txt")), Some(&Status::Modified));
    assert_eq!(map.get(Path::new("process.txt")), Some(&Status::Modified));
    assert!(
        branch.is_some(),
        "#160: current branch must resolve for the tree border"
    );
    assert!(
        clean_diff.contains("-clean-a") && clean_diff.contains("+clean-b"),
        "clean-filter path must return a real correct diff: {clean_diff:?}"
    );
    assert!(
        process_diff.contains("-process-a") && process_diff.contains("+process-b"),
        "process-filter path must return a real correct diff: {process_diff:?}"
    );

    // Git accepts an empty filter subsection and selects it with `filter=`.
    fs::write(
        repo.path().join(".gitattributes"),
        "clean.txt filter=\nprocess.txt filter=\n",
    )
    .unwrap();
    git(repo.path(), &["config", "filter..required", "true"]);
    for field in ["clean", "process"] {
        git(
            repo.path(),
            &[
                "config",
                &format!("filter..{field}"),
                payloads
                    .path()
                    .join(format!("{field}-filter.sh"))
                    .to_str()
                    .unwrap(),
            ],
        );
        let empty_filter_status = status(repo.path());
        let empty_filter_diff = diff(
            repo.path(),
            Path::new("clean.txt"),
            Baseline::Head,
            None,
            false,
        );
        assert!(
            !clean_marker.exists(),
            "empty clean driver must not execute"
        );
        assert!(
            !process_marker.exists(),
            "empty process driver must not execute"
        );
        assert_eq!(
            empty_filter_status.get(Path::new("clean.txt")),
            Some(&Status::Modified)
        );
        assert_eq!(
            empty_filter_status.get(Path::new("process.txt")),
            Some(&Status::Modified)
        );
        assert!(
            empty_filter_diff.contains("-clean-a") && empty_filter_diff.contains("+clean-b"),
            "empty {field} driver must preserve diffs: {empty_filter_diff:?}"
        );
    }

    // A name that cannot be represented by `-c key=value` must refuse the query.
    git(
        repo.path(),
        &[
            "config",
            "filter.hostile=clean.clean",
            payloads.path().join("clean-filter.sh").to_str().unwrap(),
        ],
    );
    fs::write(
        repo.path().join(".gitattributes"),
        "clean.txt filter=hostile=clean\n",
    )
    .unwrap();
    assert!(status(repo.path()).is_empty());
    assert!(
        diff(
            repo.path(),
            Path::new("clean.txt"),
            Baseline::Head,
            None,
            false
        )
        .is_empty()
    );
    assert!(
        !clean_marker.exists(),
        "unrepresentable driver must not execute"
    );
}
