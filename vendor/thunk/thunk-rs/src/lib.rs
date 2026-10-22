#![doc = include_str!("../README.md")]

use std::{env, path::PathBuf};

const VC_LTL_VERSION: &'static str = "5.2.2";
const YY_THUNKS_VERSION: &'static str = "1.1.7";

/// This function should be call in build.rs.
pub fn thunk() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap();

    if target_os != "windows" || target_env != "msvc" {
        println!("cargo::warning=Skipped! Only Windows(MSVC) is supported!");
        return;
    }

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    // Enable VC-LTL5
    let vc_ltl_arch = if target_arch == "x86" { "Win32" } else { "x64" };
    let vc_ltl_platform = if cfg!(feature = "xp") {
        if vc_ltl_arch == "Win32" {
            "5.1.2600.0"
        } else {
            "5.2.3790.0"
        }
    } else if cfg!(feature = "vista") || cfg!(feature = "win7") {
        "6.0.6000.0"
    } else if cfg!(feature = "win8") {
        "6.2.9200.0"
    } else if cfg!(feature = "win10_10240") {
        "10.0.10240.0"
    } else if cfg!(feature = "win10_19041") {
        "10.0.19041.0"
    } else if cfg!(feature = "vc_ltl_only") {
        "6.0.6000.0"
    } else {
        println!("cargo::warning=VC-LTL5 Skipped: Nothing to do!");
        return;
    };

    let vc_ltl = local_library("VC_LTL", &format!("VC-LTL-{}", VC_LTL_VERSION));

    let vc_ltl_path = vc_ltl.join(&format!(
        "TargetPlatform/{}/lib/{}",
        vc_ltl_platform, vc_ltl_arch
    ));
    assert!(
        vc_ltl_path.is_dir(),
        "VC-LTL libraries are missing: {}. Set VC_LTL to a local installation for other target platforms.",
        vc_ltl_path.display()
    );

    println!("cargo::rustc-link-search={}", vc_ltl_path.to_string_lossy());
    println!(
        "cargo::warning=VC-LTL5 Enabled: {}({})",
        vc_ltl_platform, vc_ltl_arch
    );

    // Enable YY-Thunks
    let yy_thunks_arch = if target_arch == "x86" { "x86" } else { "x64" };
    let yy_thunks_platform = if cfg!(feature = "xp") {
        "WinXP"
    } else if cfg!(feature = "vista") {
        "Vista"
    } else if cfg!(feature = "win7") {
        "Win7"
    } else if cfg!(feature = "win8") {
        "Win8"
    } else if cfg!(feature = "win10_10240") {
        "Win10.0.10240"
    } else if cfg!(feature = "win10_19041") {
        "Win10.0.19041"
    } else {
        println!("cargo::warning=YY-Thunks Skipped: Nothing to do!!");
        return;
    };

    let yy_thunks = local_library("YY_THUNKS", &format!("YY-Thunks-{}", YY_THUNKS_VERSION));

    let yy_thunks = yy_thunks.join(format!(
        "objs/{}/YY_Thunks_for_{}.obj",
        yy_thunks_arch, yy_thunks_platform
    ));
    assert!(
        yy_thunks.is_file(),
        "YY-Thunks object is missing: {}. Set YY_THUNKS to a local installation for other target platforms.",
        yy_thunks.display()
    );

    println!("cargo::rustc-link-arg={}", yy_thunks.to_string_lossy());
    println!(
        "cargo::warning=YY-Thunks Enabled: {}({})",
        yy_thunks_platform, yy_thunks_arch
    );

    // Return if is lib mode
    if cfg!(feature = "lib") {
        println!("cargo::warning=Lib Mode Enabled!");
        return;
    }

    // Set subsystem to windows
    let os_version = if cfg!(feature = "xp") {
        if target_arch == "x86" {
            ",5.01"
        } else {
            ",5.02"
        }
    } else {
        ""
    };

    if cfg!(feature = "subsystem_windows") && env::var("PROFILE").unwrap() != "debug" {
        println!("cargo::rustc-link-arg=/SUBSYSTEM:WINDOWS{}", os_version);
        println!("cargo::rustc-link-arg=/ENTRY:mainCRTStartup");
        println!("cargo::warning=Subsystem is set to WINDOWS");
    } else if cfg!(feature = "subsystem_console") {
        println!("cargo::rustc-link-arg=/SUBSYSTEM:CONSOLE{}", os_version);
        println!("cargo::warning=Subsystem is set to CONSOLE");
    }
}

fn local_library(env_path: &str, bundled_name: &str) -> PathBuf {
    println!("cargo::rerun-if-env-changed={}", env_path);
    let path = env::var_os(env_path).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../windows-compat")
            .join(bundled_name)
    });
    assert!(
        path.is_dir(),
        "Local {} directory is missing: {}",
        env_path,
        path.display()
    );
    println!("cargo::rerun-if-changed={}", path.display());
    path
}
