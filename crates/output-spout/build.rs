use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SPOUT_TAG: &str = "2.007.017";
const SPOUT_COMMIT: &str = "f49e2f469f8cb25f559a6eaa61a3f5b8173fc100";
const SPOUT_ARCHIVE_SHA256: &str =
    "cb60c83d4df3c2927cd3c5a505910bb720a8011d505217a71d293968405e4bf4";
const SPOUT_ARCHIVE_BYTES: &str = "5099633";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/CMakeLists.txt");
    println!("cargo:rerun-if-changed=native/spout_bridge.cpp");
    println!("cargo:rerun-if-changed=native/spout_bridge.h");
    println!("cargo:rerun-if-env-changed=LATENTDECK_SPOUT2_SOURCE_ROOT");
    println!("cargo:rerun-if-env-changed=CMAKE");

    if env::var_os("CARGO_FEATURE_SPOUT_SDK").is_none() {
        return;
    }

    require_target("CARGO_CFG_TARGET_OS", "windows");
    require_target("CARGO_CFG_TARGET_ENV", "msvc");
    require_target("CARGO_CFG_TARGET_ARCH", "x86_64");

    let manifest_dir = PathBuf::from(required_env("CARGO_MANIFEST_DIR"));
    let source_root = env::var_os("LATENTDECK_SPOUT2_SOURCE_ROOT")
        .map_or_else(|| default_source_root(&manifest_dir), PathBuf::from);
    let source_root = source_root.canonicalize().unwrap_or_else(|error| {
        panic!(
            "prepared Spout2 source root is missing (run tools/Prepare-Spout2.ps1): {}: {error}",
            source_root.display()
        )
    });
    let source_root = without_windows_verbatim_prefix(&source_root);

    validate_prepared_source(&source_root);
    let source_dir = source_root.join("source");
    println!(
        "cargo:rerun-if-changed={}",
        source_root.join("LATENTDECK_SPOUT2_SOURCE.txt").display()
    );
    for relative in [
        "CMakeLists.txt",
        "LICENSE",
        "SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.cpp",
        "SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.h",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            source_dir.join(relative).display()
        );
    }

    let out_dir = PathBuf::from(required_env("OUT_DIR"));
    let build_dir = out_dir.join("spout2-cmake");
    let cmake = env::var_os("CMAKE").unwrap_or_else(|| "cmake".into());
    let native_dir = manifest_dir.join("native");

    run(
        Command::new(&cmake)
            .arg("-S")
            .arg(&native_dir)
            .arg("-B")
            .arg(&build_dir)
            .args(["-G", "Visual Studio 17 2022", "-A", "x64"])
            .arg(format!(
                "-DLATENTDECK_SPOUT2_SOURCE_DIR:PATH={}",
                cmake_path(&source_dir)
            )),
        "configure the pinned Spout2 bridge",
    );
    run(
        Command::new(&cmake).arg("--build").arg(&build_dir).args([
            "--config",
            "Release",
            "--target",
            "latentdeck_spout_bridge",
            "--parallel",
        ]),
        "build the pinned Spout2 bridge",
    );

    println!(
        "cargo:rustc-link-search=native={}",
        build_dir.join("lib").display()
    );
    for library in [
        "latentdeck_spout_bridge",
        "SpoutDX12_static",
        "SpoutDX_static",
        "Spout_static",
    ] {
        println!("cargo:rustc-link-lib=static={library}");
    }
    for library in [
        "d3d12", "d3d11", "d3d9", "dxgi", "opengl32", "kernel32", "user32", "gdi32", "winspool",
        "comdlg32", "comctl32", "advapi32", "shell32", "ole32", "oleaut32", "uuid", "odbc32",
        "odbccp32", "version", "winmm", "psapi",
    ] {
        println!("cargo:rustc-link-lib={library}");
    }
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("Cargo did not provide required environment {name}"))
}

fn require_target(name: &str, expected: &str) {
    let actual = required_env(name);
    assert_eq!(
        actual, expected,
        "the real Spout2 bridge supports only Windows x64 MSVC (expected {name}={expected}, got {actual})"
    );
}

fn default_source_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .join("../..")
        .join("vendor-local/spout2")
        .join(format!("{SPOUT_TAG}-{SPOUT_COMMIT}"))
}

fn validate_prepared_source(source_root: &Path) {
    let stamp_path = source_root.join("LATENTDECK_SPOUT2_SOURCE.txt");
    let stamp = fs::read_to_string(&stamp_path).unwrap_or_else(|error| {
        panic!(
            "prepared Spout2 stamp is missing or unreadable: {}: {error}",
            stamp_path.display()
        )
    });
    let expected = format!(
        "schema=1\ntag={SPOUT_TAG}\ncommit={SPOUT_COMMIT}\narchive_sha256={SPOUT_ARCHIVE_SHA256}\narchive_bytes={SPOUT_ARCHIVE_BYTES}\nsource_directory=source\n"
    );
    assert_eq!(
        stamp.replace("\r\n", "\n"),
        expected,
        "prepared Spout2 stamp does not match the exact approved pin; rerun tools/Prepare-Spout2.ps1 into a clean destination"
    );

    let source_dir = source_root.join("source");
    for relative in [
        "CMakeLists.txt",
        "LICENSE",
        "SPOUTSDK/SpoutGL/CMakeLists.txt",
        "SPOUTSDK/SpoutDirectX/SpoutDX/CMakeLists.txt",
        "SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/CMakeLists.txt",
        "SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.cpp",
        "SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.h",
    ] {
        let path = source_dir.join(relative);
        assert!(
            path.is_file(),
            "prepared Spout2 source is incomplete: missing {relative}"
        );
    }
}

fn cmake_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn without_windows_verbatim_prefix(path: &Path) -> PathBuf {
    let rendered = path.to_string_lossy();
    if let Some(rest) = rendered.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    rendered
        .strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn run(command: &mut Command, action: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to {action}: {error}"));
    assert!(status.success(), "failed to {action}: {status}");
}
