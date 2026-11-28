/// The git commit the firmware is built from, as `MPY_GIT_COMMIT` for the
/// code (`#@version`, the banner): set by the Makefile, else from git here,
/// else `unknown`. As in micropython-esp32.
fn main() {
    // memory/mg24.x, the linker script (.cargo/config.toml: -Tmg24.x)
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={}/memory", dir);
    println!("cargo:rerun-if-changed=memory/mg24.x");
    println!("cargo:rerun-if-env-changed=MPY_GIT_COMMIT");
    let commit = std::env::var("MPY_GIT_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
            let out = std::process::Command::new("git")
                .args([
                    "-C",
                    &dir,
                    "describe",
                    "--always",
                    "--dirty",
                    "--abbrev=7",
                    "--exclude=*",
                ])
                .output()
                .ok()?;
            let text = String::from_utf8(out.stdout).ok()?;
            Some(text.trim().to_string()).filter(|s| out.status.success() && !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=MPY_GIT_COMMIT={}", commit);
}
