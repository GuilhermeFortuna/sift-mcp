//! CONTRIBUTING.md must point at `./ci.sh` rather than redefining the suite.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

#[test]
fn contributing_does_not_redefine_the_validation_command() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("CONTRIBUTING.md");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("CONTRIBUTING.md must exist at {}: {e}", path.display()));

    assert!(
        text.contains("./ci.sh"),
        "CONTRIBUTING.md must name ./ci.sh as the validation command"
    );
    assert!(
        !text.contains("cargo fmt --all -- --check"),
        "CONTRIBUTING.md must not redefine the validation suite; that sequence lives in ci.sh"
    );
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn executable(path: &std::path::Path, body: &str) {
    fs::write(path, body).expect("write fake executable");
    let mut permissions = fs::metadata(path)
        .expect("read fake executable metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fake executable executable");
}

fn fake_environment() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("create hook test directory");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).expect("create fake executable directory");

    executable(
        &bin.join("git"),
        "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> \"$HOOK_LOG\"\nif [ \"$1\" = rev-parse ]; then printf '%s\\n' \"$HOOK_ROOT\"; fi\n",
    );
    executable(
        &bin.join("cargo"),
        "#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" >> \"$HOOK_LOG\"\nexit \"${FAKE_CARGO_STATUS:-0}\"\n",
    );

    (temp, bin)
}

fn run_hook(
    hook: &str,
    temp: &TempDir,
    bin: &std::path::Path,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let log = temp.path().join("calls.log");
    let system_path = std::env::var_os("PATH").expect("test environment has PATH");
    let path = std::env::join_paths(
        std::iter::once(bin.as_os_str().to_owned())
            .chain(std::env::split_paths(&system_path).map(|path| path.into_os_string())),
    )
    .expect("construct test PATH");
    let mut command = Command::new(workspace_root().join(".githooks").join(hook));
    command
        .env("PATH", path)
        .env("HOOK_ROOT", temp.path())
        .env("HOOK_LOG", &log);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().expect("run hook")
}

#[test]
fn pre_commit_formats_and_auto_fixes_before_strict_clippy() {
    let (temp, bin) = fake_environment();

    let output = run_hook("pre-commit", &temp, &bin, &[]);

    assert!(output.status.success(), "pre-commit failed: {output:?}");
    let calls = fs::read_to_string(temp.path().join("calls.log")).expect("read hook calls");
    assert_eq!(
        calls,
        "git rev-parse --show-toplevel\n\
cargo fmt --all\n\
cargo clippy --fix --workspace --all-targets --allow-dirty --allow-staged -- -D warnings\n\
cargo fmt --all\n\
git diff --check\n\
cargo clippy --workspace --all-targets -- -D warnings\n"
    );
}

#[test]
fn pre_commit_stops_when_formatting_fails() {
    let (temp, bin) = fake_environment();

    let output = run_hook("pre-commit", &temp, &bin, &[("FAKE_CARGO_STATUS", "17")]);

    assert_eq!(output.status.code(), Some(17));
    let calls = fs::read_to_string(temp.path().join("calls.log")).expect("read hook calls");
    assert_eq!(
        calls,
        "git rev-parse --show-toplevel\n\
cargo fmt --all\n"
    );
}

#[test]
fn pre_push_runs_ci_from_the_repository_root() {
    let (temp, bin) = fake_environment();
    executable(
        &temp.path().join("ci.sh"),
        "#!/bin/sh\nprintf 'ci %s\\n' \"$PWD\" >> \"$HOOK_LOG\"\nexit \"${FAKE_CI_STATUS:-0}\"\n",
    );

    let output = run_hook("pre-push", &temp, &bin, &[]);

    assert!(output.status.success(), "pre-push failed: {output:?}");
    let calls = fs::read_to_string(temp.path().join("calls.log")).expect("read hook calls");
    assert_eq!(
        calls,
        format!(
            "git rev-parse --show-toplevel\nci {}\n",
            temp.path().display()
        )
    );
}

#[test]
fn pre_push_propagates_ci_failure() {
    let (temp, bin) = fake_environment();
    executable(
        &temp.path().join("ci.sh"),
        "#!/bin/sh\nexit \"${FAKE_CI_STATUS:-0}\"\n",
    );

    let output = run_hook("pre-push", &temp, &bin, &[("FAKE_CI_STATUS", "23")]);

    assert_eq!(output.status.code(), Some(23));
}
