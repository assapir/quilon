// Shared by `build.rs` and `quilon-rt/build.rs` via `include!`: cargo build scripts are
// separate crates, so this is the plain way to share a helper between two of them.
//
// The macOS version to stamp compiled objects with. An explicit `MACOSX_DEPLOYMENT_TARGET`
// wins; otherwise this matches rustc's own default for the target.
fn macos_deployment_target() -> String {
    if let Ok(v) = std::env::var("MACOSX_DEPLOYMENT_TARGET") {
        return v;
    }
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let target = std::env::var("TARGET").unwrap_or_default();
    let mut command = std::process::Command::new(rustc);
    command.arg("--print").arg("deployment-target");
    if !target.is_empty() {
        command.arg("--target").arg(&target);
    }
    let output = command
        .output()
        .expect("failed to run `rustc --print deployment-target`");
    assert!(
        output.status.success(),
        "`rustc --print deployment-target` failed"
    );
    // Prints `MACOSX_DEPLOYMENT_TARGET=11.0` (the env var name varies by platform,
    // e.g. `IPHONEOS_DEPLOYMENT_TARGET` for iOS) — the version is everything after the `=`.
    String::from_utf8(output.stdout)
        .expect("rustc output is not UTF-8")
        .trim()
        .rsplit('=')
        .next()
        .expect("unexpected `rustc --print deployment-target` output")
        .to_string()
}
