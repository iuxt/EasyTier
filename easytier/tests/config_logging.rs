#![cfg(feature = "management")]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn binary() -> std::ffi::OsString {
    std::env::var_os("NEXTEST_BIN_EXE_easytier_core")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_easytier-core").into())
}

fn check(path: &Path) -> Command {
    let mut command = Command::new(binary());
    command.env_clear().args(["--check-config", "-c"]).arg(path);
    command
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn full_chinese_template_is_valid() {
    assert_success(
        check(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../config.toml"))
            .output()
            .unwrap(),
    );
}

#[test]
fn startup_ignores_environment_settings_and_expansion() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "instance_name = '$HOME/${HOME}'\n[console_logger]\nlevel = 'warn'\n[file_logger]\nlevel = 'info'").unwrap();
    assert_success(
        check(&path)
            .env("ET_CONFIG_FILE", "/missing.toml")
            .env("ET_CONFIG_DIR", "/missing")
            .env("ET_NETWORK_NAME", "env-network")
            .env("ET_CONSOLE_LOG_LEVEL", "invalid")
            .env("RUST_LOG", "invalid[syntax")
            .env("HOME", "environment-value")
            .output()
            .unwrap(),
    );
}

#[test]
fn malformed_logging_and_removed_arguments_fail() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    for content in [
        "[console_logger]\nlevel = 123",
        "[file_logger]\nlevel = 'invalid'",
        "[core]\nconfig_server = 'udp://127.0.0.1:22020/token'",
    ] {
        fs::write(&path, content).unwrap();
        assert!(!check(&path).output().unwrap().status.success());
    }
    fs::write(&path, "").unwrap();
    for arg in [
        "--network-name",
        "--console-log-level",
        "--config-server",
        "--daemon",
    ] {
        assert!(!check(&path).arg(arg).output().unwrap().status.success());
    }
    assert!(
        !check(&dir.path().join("missing.toml"))
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!check(Path::new("-")).output().unwrap().status.success());
}

#[test]
fn default_config_is_resolved_from_executable_directory() {
    let dir = TempDir::new().unwrap();
    let bin_dir = dir.path().join("bin");
    let work_dir = dir.path().join("work");
    fs::create_dir(&bin_dir).unwrap();
    fs::create_dir(&work_dir).unwrap();
    let executable = bin_dir.join(format!("easytier-core{}", std::env::consts::EXE_SUFFIX));
    fs::copy(binary(), &executable).unwrap();
    fs::write(
        bin_dir.join("config.toml"),
        "instance_name = 'next-to-executable'",
    )
    .unwrap();
    fs::write(work_dir.join("config.toml"), "invalid = [").unwrap();
    assert_success(
        Command::new(&executable)
            .env_clear()
            .current_dir(&work_dir)
            .arg("--check-config")
            .output()
            .unwrap(),
    );
    fs::remove_file(bin_dir.join("config.toml")).unwrap();
    assert!(
        !Command::new(&executable)
            .env_clear()
            .current_dir(&work_dir)
            .arg("--check-config")
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::write(work_dir.join("custom.toml"), "instance_name = 'explicit'").unwrap();
    assert_success(
        Command::new(executable)
            .env_clear()
            .current_dir(work_dir)
            .args(["-c", "custom.toml", "--check-config"])
            .output()
            .unwrap(),
    );
}
