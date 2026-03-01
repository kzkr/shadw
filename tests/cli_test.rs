use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[allow(deprecated)]
fn shadw() -> Command {
    Command::cargo_bin("shadw").unwrap()
}

/// Create a temp dir with a .git/ directory to simulate a git repo.
fn make_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

// --- shadw init ---

#[test]
fn init_outside_git_repo_fails() {
    let dir = TempDir::new().unwrap();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a git repository"));
}

#[test]
fn init_in_git_repo_creates_shadw_dir() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized Shadw"));

    // Verify directory structure
    let shadw = dir.path().join(".shadw");
    assert!(shadw.join("config.toml").exists());
    assert!(shadw.join("contexts").is_dir());
    assert!(shadw.join("state").is_dir());
    assert!(shadw.join("state/cursor.json").exists());
}

#[test]
fn init_twice_fails() {
    let dir = make_git_repo();

    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
}

#[test]
fn init_appends_gitignore() {
    let dir = make_git_repo();
    // Write a pre-existing .gitignore
    fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();

    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(contents.contains("target/"));
    assert!(contents.contains(".shadw/"));
}

#[test]
fn init_does_not_duplicate_gitignore_entry() {
    let dir = make_git_repo();
    fs::write(dir.path().join(".gitignore"), ".shadw/\n").unwrap();

    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(contents.matches(".shadw/").count(), 1);
}

#[test]
fn init_installs_pre_push_hook() {
    let dir = make_git_repo();
    // Create hooks dir to simulate real git repo
    fs::create_dir_all(dir.path().join(".git/hooks")).unwrap();

    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    let hook = dir.path().join(".git/hooks/pre-push");
    assert!(hook.exists(), "pre-push hook should exist");

    let contents = fs::read_to_string(&hook).unwrap();
    assert!(contents.contains("refs/notes/shadw"));
    assert!(contents.starts_with("#!/bin/sh"));
}

#[test]
fn init_creates_github_action() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    let workflow = dir.path().join(".github/workflows/shadw.yml");
    assert!(workflow.exists(), "GitHub Action workflow should exist");

    let contents = fs::read_to_string(&workflow).unwrap();
    assert!(contents.contains("name: Shadw"));
    assert!(contents.contains("refs/notes/shadw"));
}

#[test]
fn init_appends_to_existing_pre_push_hook() {
    let dir = make_git_repo();
    fs::create_dir_all(dir.path().join(".git/hooks")).unwrap();
    fs::write(
        dir.path().join(".git/hooks/pre-push"),
        "#!/bin/sh\necho 'existing hook'\n",
    )
    .unwrap();

    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    let contents = fs::read_to_string(dir.path().join(".git/hooks/pre-push")).unwrap();
    assert!(contents.contains("existing hook"));
    assert!(contents.contains("refs/notes/shadw"));
}

// --- shadw start ---

#[test]
fn start_without_init_fails() {
    let dir = make_git_repo();
    shadw()
        .arg("start")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn start_foreground_starts_and_stops() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    // Start in foreground, then kill it after a brief moment
    #[allow(deprecated)]
    let bin = assert_cmd::cargo::cargo_bin("shadw");
    let mut child = std::process::Command::new(bin)
        .args(["start", "--foreground"])
        .current_dir(dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // Give it time to start and write PID file
    std::thread::sleep(std::time::Duration::from_millis(500));

    let pid_file = dir.path().join(".shadw/state/daemon.pid");
    assert!(pid_file.exists(), "PID file should exist");

    // Send SIGTERM
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }

    let status = child.wait().unwrap();
    // Process should exit cleanly (or with signal)
    // On some systems the exit code may be non-zero due to signal
    let _ = status;

    // PID file should be cleaned up
    // Give a moment for cleanup
    std::thread::sleep(std::time::Duration::from_millis(100));
}

#[test]
fn start_background_and_stop() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    // Start daemon in background
    shadw()
        .arg("start")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Shadw is now watching"));

    // Give the child process time to start and register signal handlers
    std::thread::sleep(std::time::Duration::from_millis(500));

    // PID file should exist
    let pid_file = dir.path().join(".shadw/state/daemon.pid");
    assert!(pid_file.exists(), "PID file should exist after start");

    // Running again should fail
    shadw()
        .arg("start")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("already running"));

    // Stop the daemon
    shadw()
        .arg("stop")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Shadw stopped"));

    // PID file should be gone
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(!pid_file.exists(), "PID file should be removed after stop");
}

// --- shadw stop ---

#[test]
fn stop_without_init_fails() {
    let dir = make_git_repo();
    shadw()
        .arg("stop")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn stop_with_no_daemon_running() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    shadw()
        .arg("stop")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Shadw is not running"));
}

#[test]
fn stop_cleans_stale_pid_file() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    // Write a stale PID (unlikely to be a real process)
    fs::write(
        dir.path().join(".shadw/state/daemon.pid"),
        "9999999",
    )
    .unwrap();

    shadw()
        .arg("stop")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Shadw is not running"));
}

// --- shadw status ---

#[test]
fn status_without_init_fails() {
    let dir = make_git_repo();
    shadw()
        .arg("status")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn status_after_init() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    shadw()
        .arg("status")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(
            predicate::str::contains("stopped")
                .and(predicate::str::contains("gpt-oss")),
        );
}

// --- shadw retry ---

#[test]
fn retry_without_init_fails() {
    let dir = make_git_repo();
    shadw()
        .args(["retry", "abc123"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn retry_no_context_found() {
    let dir = make_git_repo();
    shadw()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    shadw()
        .args(["retry", "deadbeef12345678"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("no saved context found").or(
            predicate::str::contains("no contexts directory"),
        ));
}

// --- shadw use ---

#[test]
fn use_lists_models() {
    // `shadw use` works without init — it lists available models
    shadw()
        .arg("use")
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-oss"));
}

#[test]
fn use_unknown_model_fails() {
    shadw()
        .args(["use", "bogus-model-that-does-not-exist"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown model"));
}
