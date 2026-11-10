//! Run on a Mac with Metal and VideoToolbox. Cocoa requires the process main thread.
#![cfg(all(target_os = "macos", feature = "demo-library"))]

#[test]
#[ignore = "requires a Mac display and hardware decoder"]
fn metal_shaders_draw_the_reference_fixtures() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_nightjar"))
        .arg("--check-metal")
        .status()
        .expect("start Nightjar");
    assert!(status.success(), "Metal check exited with {status}");
}

#[test]
#[ignore = "requires a Mac display"]
fn launcher_can_select_a_host_without_an_audio_driver() {
    let config =
        std::env::temp_dir().join(format!("nightjar-launcher-no-audio-{}", std::process::id()));
    std::fs::create_dir(&config).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_nightjar"))
        .env("NIGHTJAR_CONFIG_DIR", &config)
        .env_remove("NIGHTJAR_AUTOSTREAM")
        .env("SDL_AUDIO_DRIVER", "nightjar-missing-driver")
        .args(["--host", "missing-test-pc"])
        .output();
    std::fs::remove_dir_all(config).unwrap();
    let output = result.expect("start Nightjar");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("no saved PC is named missing-test-pc"),
        "{stderr}"
    );
}
