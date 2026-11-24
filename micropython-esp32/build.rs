fn main() {
    select_script();
    select_cpu_clock();
    linker_be_nice();
    check_xtensa_linker_available();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

#[cfg(unix)]
fn check_xtensa_linker_available() {
    println!("cargo:rerun-if-env-changed=PATH");

    let target = std::env::var("TARGET").unwrap_or_default();
    println!(
        "cargo:rerun-if-env-changed={}",
        cargo_linker_env_var(&target)
    );

    let linker = xtensa_linker(&target);

    let error = match std::process::Command::new(&linker)
        .arg("--version")
        .output()
    {
        Ok(_) => return,
        Err(error) => error,
    };

    let export_file = std::env::var("HOME")
        .map(|home| format!("{home}/export-esp.sh"))
        .unwrap_or_else(|_| "$HOME/export-esp.sh".to_string());

    panic!(
        "Xtensa linker `{linker}` was not found in PATH or could not be executed: {error}.\n\n\
         Xtensa targets on Unix require espup's environment export file to be sourced before building.\n\
         Try: `source {export_file}`\n\n\
         If the export file does not exist, run `espup install` first.\n\
         For more details, see:\n\
         https://github.com/esp-rs/espup?tab=readme-ov-file#environment-variables-setup"
    );
}

#[cfg(not(unix))]
fn check_xtensa_linker_available() {}

#[cfg(unix)]
fn xtensa_linker(target: &str) -> String {
    if let Some(linker) = cargo_configured_linker(target) {
        return linker;
    }

    target
        .strip_prefix("xtensa-")
        .and_then(|target| target.strip_suffix("-none-elf"))
        .map(|chip| format!("xtensa-{chip}-elf-gcc"))
        .unwrap_or_else(|| "xtensa-esp32-elf-gcc".to_string())
}

#[cfg(unix)]
fn cargo_configured_linker(target: &str) -> Option<String> {
    std::env::var(cargo_linker_env_var(target)).ok()
}

#[cfg(unix)]
fn cargo_linker_env_var(target: &str) -> String {
    let target = target.replace('-', "_").to_ascii_uppercase();
    format!("CARGO_TARGET_{target}_LINKER")
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}

/// Chooses the CPU clock: `MPY_CPU_MHZ` (set by `make flash CPU_MHZ=...`) is
/// 80 (the default), 160 or 240, passed to the code as `cfg(cpu_mhz = "...")`.
fn select_cpu_clock() {
    println!("cargo:rerun-if-env-changed=MPY_CPU_MHZ");
    println!("cargo::rustc-check-cfg=cfg(cpu_mhz, values(\"80\", \"160\", \"240\"))");
    let mhz = std::env::var("MPY_CPU_MHZ")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "80".to_string());
    if !["80", "160", "240"].contains(&mhz.as_str()) {
        panic!("CPU_MHZ/MPY_CPU_MHZ must be 80, 160 or 240, not `{}`", mhz);
    }
    println!("cargo:rustc-cfg=cpu_mhz=\"{}\"", mhz);
}

/// Chooses the Python script embedded into the firmware.
///
/// `MPY_SCRIPT` (set by `make flash SCRIPT=...`) is a path relative to this
/// crate's directory. Without it, the board's demo from `scripts/` is used.
/// The path is passed to `main.rs`, which embeds the file with `include_str!`.
fn select_script() {
    println!("cargo:rerun-if-env-changed=MPY_SCRIPT");

    let crate_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let default = if std::env::var_os("CARGO_FEATURE_TFT").is_some() {
        "scripts/ttgo_demo.py"
    } else {
        "scripts/demo.py"
    };
    let chosen = std::env::var("MPY_SCRIPT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string());

    let path = crate_dir.join(&chosen);
    if !path.is_file() {
        panic!(
            "Python script `{}` not found. SCRIPT/MPY_SCRIPT paths are relative to {} \
             and must stay inside the repository (the Docker build only sees the repository).",
            chosen,
            crate_dir.display()
        );
    }
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo:rustc-env=MPY_SCRIPT_PATH={}", path.display());
    println!("cargo:rustc-env=MPY_SCRIPT_NAME={}", chosen);
}
