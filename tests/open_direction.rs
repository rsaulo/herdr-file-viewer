//! Executable coverage for the config-to-launcher `open_direction` handoff.
//!
//! The pure config resolver and argv parser have unit coverage, but the feature only works when
//! the built binary reads `HERDR_PLUGIN_CONFIG_DIR`, prints a Herdr direction token, and each split
//! launcher carries that token into the final Herdr argv. These tests exercise that production
//! boundary with real launcher scripts and a recording Herdr stub.

mod common;

use common::TempDir;
use std::path::Path;
use std::process::{Command, Output};

fn query_direction(config_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_herdr-file-viewer"))
        .arg("--open-direction")
        .env("HERDR_PLUGIN_CONFIG_DIR", config_dir)
        .output()
        .expect("run the built viewer's --open-direction query")
}

fn assert_direction(config_dir: &Path, expected: &str) {
    let output = query_direction(config_dir);
    assert!(
        output.status.success(),
        "direction query failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{expected}\n")
    );
    assert!(
        output.stderr.is_empty(),
        "direction query must be a quiet launcher contract: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn binary_open_direction_query_reads_the_injected_plugin_config_dir() {
    let temp = TempDir::new();
    let config_dir = temp.path().join("plugin-config");
    std::fs::create_dir_all(&config_dir).expect("create plugin config dir");

    for (configured, expected) in [("down", "down"), (" Bottom ", "down"), ("right", "right")] {
        std::fs::write(
            config_dir.join("config.toml"),
            format!("open_direction = {configured:?}\n"),
        )
        .expect("write config");
        assert_direction(&config_dir, expected);
    }
}

#[test]
fn binary_open_direction_query_falls_back_to_right_without_a_valid_config() {
    let temp = TempDir::new();
    let config_dir = temp.path().join("plugin-config");
    std::fs::create_dir_all(&config_dir).expect("create plugin config dir");

    // Missing config.
    assert_direction(&config_dir, "right");

    // Malformed config.
    std::fs::write(config_dir.join("config.toml"), "open_direction = [\n")
        .expect("write malformed config");
    assert_direction(&config_dir, "right");

    // A well-formed but unrecognized value.
    std::fs::write(
        config_dir.join("config.toml"),
        "open_direction = \"sideways\"\n",
    )
    .expect("write unrecognized config");
    assert_direction(&config_dir, "right");
}

#[cfg(unix)]
mod unix_launcher {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable(path: &Path) {
        let mut permissions = std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)
            .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
    }

    fn run_launcher(
        config: Option<&str>,
        config_dir_fails: bool,
        panes_json: Option<&str>,
    ) -> (Output, String) {
        let temp = TempDir::new();
        let plugin_root = temp.path().join("plugin");
        let scripts_dir = plugin_root.join("scripts");
        let release_dir = plugin_root.join("target/release");
        let config_dir = temp.path().join("plugin-config");
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        std::fs::create_dir_all(&release_dir).expect("create release dir");
        std::fs::create_dir_all(&config_dir).expect("create plugin config dir");

        let launcher = scripts_dir.join("open-file-viewer.sh");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/open-file-viewer.sh"),
            &launcher,
        )
        .expect("copy Unix launcher");
        executable(&launcher);

        let viewer = release_dir.join("herdr-file-viewer");
        std::fs::copy(env!("CARGO_BIN_EXE_herdr-file-viewer"), &viewer)
            .expect("copy built viewer into the launcher's expected layout");
        executable(&viewer);

        if let Some(contents) = config {
            std::fs::write(config_dir.join("config.toml"), contents).expect("write config");
        }

        let capture = temp.path().join("herdr-calls");
        let fake_herdr = temp.path().join("herdr-stub");
        std::fs::write(
            &fake_herdr,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$HERDR_CAPTURE"
case "$1 $2" in
  "pane list")
    if [ -n "${HERDR_PANES_JSON:-}" ]; then printf '%s\n' "$HERDR_PANES_JSON"; fi
    ;;
  "plugin config-dir")
    if [ "${HERDR_CONFIG_DIR_FAIL:-0}" = 1 ]; then exit 1; fi
    printf '%s\n' "$HERDR_TEST_CONFIG_DIR"
    ;;
esac
exit 0
"#,
        )
        .expect("write Herdr stub");
        executable(&fake_herdr);

        let mut command = Command::new(&launcher);
        command
            .env("HERDR_BIN_PATH", &fake_herdr)
            .env("HERDR_CAPTURE", &capture)
            .env("HERDR_TEST_CONFIG_DIR", &config_dir)
            .env(
                "HERDR_CONFIG_DIR_FAIL",
                if config_dir_fails { "1" } else { "0" },
            )
            // Prove the action discovers the plugin config directory rather than inheriting it.
            .env_remove("HERDR_PLUGIN_CONFIG_DIR")
            // Keep a failed config-dir lookup away from the developer's real fallback config.
            .env("XDG_CONFIG_HOME", temp.path().join("empty-xdg"))
            .env("HOME", temp.path().join("empty-home"));
        if let Some(json) = panes_json {
            command.env("HERDR_PANES_JSON", json);
        } else {
            command.env_remove("HERDR_PANES_JSON");
        }

        let output = command.output().expect("run Unix split launcher");
        let calls = std::fs::read_to_string(&capture).unwrap_or_default();
        (output, calls)
    }

    fn assert_launcher_succeeded(output: &Output) {
        assert!(
            output.status.success(),
            "launcher failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn unix_open_path_discovers_config_and_passes_down_to_herdr() {
        let (output, calls) = run_launcher(Some("open_direction = \"down\"\n"), false, None);
        assert_launcher_succeeded(&output);
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            [
                "pane list",
                "plugin config-dir herdr-file-viewer",
                "plugin pane open --plugin herdr-file-viewer --entrypoint file-viewer --placement split --direction down --focus",
            ]
        );
    }

    #[test]
    fn unix_open_path_falls_back_to_right_when_config_dir_discovery_fails() {
        let (output, calls) = run_launcher(None, true, None);
        assert_launcher_succeeded(&output);
        let open = calls
            .lines()
            .find(|line| line.starts_with("plugin pane open "))
            .unwrap_or_else(|| panic!("launcher never opened a pane:\n{calls}"));
        assert!(
            open.contains("--direction right"),
            "failed config discovery must preserve the right split default: {open}"
        );
    }

    #[test]
    fn unix_focus_path_does_not_probe_open_direction() {
        let panes = r#"{"result":{"panes":[{"pane_id":"w1:p1","label":"Terminal","focused":true,"tab_id":"w1:t1"},{"pane_id":"w1:p2","label":"Files","focused":false,"tab_id":"w1:t1"}]}}"#;
        let (output, calls) = run_launcher(Some("open_direction = \"down\"\n"), false, Some(panes));
        assert_launcher_succeeded(&output);
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            ["pane list", "pane zoom w1:p2 --on", "pane zoom w1:p2 --off"]
        );
        assert!(
            !calls.contains("plugin config-dir") && !calls.contains("plugin pane open"),
            "FOCUS must not perform OPEN-only config or pane work:\n{calls}"
        );
    }
}

#[cfg(windows)]
mod windows_launcher {
    use super::*;

    #[test]
    fn powershell_open_path_discovers_config_and_passes_down_to_herdr() {
        let temp = TempDir::new();
        let plugin_root = temp.path().join("plugin");
        let scripts_dir = plugin_root.join("scripts");
        let release_dir = plugin_root.join("target/release");
        let config_dir = temp.path().join("plugin-config");
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        std::fs::create_dir_all(&release_dir).expect("create release dir");
        std::fs::create_dir_all(&config_dir).expect("create plugin config dir");

        let launcher = scripts_dir.join("open-file-viewer.ps1");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/open-file-viewer.ps1"),
            &launcher,
        )
        .expect("copy PowerShell launcher");
        std::fs::copy(
            env!("CARGO_BIN_EXE_herdr-file-viewer"),
            release_dir.join("herdr-file-viewer.exe"),
        )
        .expect("copy built viewer into the launcher's expected layout");
        std::fs::write(
            config_dir.join("config.toml"),
            "open_direction = \"down\"\n",
        )
        .expect("write config");

        let capture = temp.path().join("herdr-calls.txt");
        let fake_herdr = temp.path().join("herdr-stub.cmd");
        std::fs::write(
            &fake_herdr,
            r#"@echo off
echo %*>>"%HERDR_CAPTURE%"
if "%1 %2"=="pane list" exit /b 0
if "%1 %2"=="plugin config-dir" (
  echo %HERDR_TEST_CONFIG_DIR%
  exit /b 0
)
if "%1 %2"=="pane split" echo {"pane_id":"w1:p2"}
exit /b 0
"#,
        )
        .expect("write Herdr cmd stub");

        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&launcher)
            .env("HERDR_BIN_PATH", &fake_herdr)
            .env("HERDR_CAPTURE", &capture)
            .env("HERDR_TEST_CONFIG_DIR", &config_dir)
            .env_remove("HERDR_PLUGIN_CONFIG_DIR")
            .env("USERPROFILE", temp.path().join("empty-profile"))
            .output()
            .expect("run PowerShell split launcher");
        assert!(
            output.status.success(),
            "PowerShell launcher failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let calls = std::fs::read_to_string(&capture).expect("read recorded Herdr calls");
        assert!(
            calls
                .lines()
                .any(|line| line == "plugin config-dir herdr-file-viewer"),
            "launcher must discover the plugin config directory:\n{calls}"
        );
        let split = calls
            .lines()
            .find(|line| line.starts_with("pane split "))
            .unwrap_or_else(|| panic!("launcher never split a pane:\n{calls}"));
        assert!(
            split.contains("--direction down"),
            "launcher must pass the configured direction to Herdr: {split}"
        );
        assert!(
            split.contains("--env") && split.contains("HERDR_PLUGIN_CONFIG_DIR="),
            "launcher must pass the discovered config directory to the viewer pane: {split}"
        );
    }
}
