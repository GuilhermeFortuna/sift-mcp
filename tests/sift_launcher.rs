use std::process::{Command, Stdio};

fn launcher() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/sift")
}

#[test]
fn launcher_help_lists_supported_commands() {
    let output = Command::new(launcher()).arg("help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "run", "dev", "daemon", "console", "build", "stop", "status", "logs",
    ] {
        assert!(stdout.contains(command), "missing {command} in {stdout}");
    }
}

#[test]
fn launcher_rejects_unknown_commands() {
    let output = Command::new(launcher()).arg("unknown").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown command"));
}

#[test]
fn launcher_stop_terminates_the_owned_process_group() {
    let state = tempfile::tempdir().unwrap();
    let mut process = Command::new("setsid")
        .args(["--wait", "sleep", "30"])
        .spawn()
        .unwrap();
    std::fs::write(state.path().join("console.pid"), process.id().to_string()).unwrap();

    let output = Command::new(launcher())
        .env("SIFT_STATE_DIR", state.path())
        .arg("stop")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let _ = process.wait();
    assert!(!process_group_exists(process.id()));
}

fn process_group_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", "--", &format!("-{pid}")])
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
