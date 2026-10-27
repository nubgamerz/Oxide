//! Embeds Oxide's icon and version in the executable, so Explorer, the taskbar and the
//! installer show them, and gives the app its version and commit (`OXIDE_VERSION`,
//! `OXIDE_COMMIT`, read by `version.rs`).
//!
//! The version is the workspace's (`Cargo.toml`), unless `OXIDE_VERSION` is set: the
//! installer check builds the same code as several versions to install one over another. The
//! commit is `git rev-parse --short HEAD`, unless
//! `OXIDE_COMMIT` is set (`scripts/installer.ps1` sets it, marked `-modified` when the working
//! tree has changes).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/oxide.ico");
    println!("cargo:rerun-if-env-changed=OXIDE_VERSION");
    println!("cargo:rerun-if-env-changed=OXIDE_COMMIT");

    let package = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let version = match std::env::var("OXIDE_VERSION") {
        Ok(v) if !v.is_empty() => v,
        _ => package,
    };
    let numbers = parse_version(&version)
        .unwrap_or_else(|| panic!("OXIDE_VERSION {version:?} isn't major.minor.patch"));
    let commit = match std::env::var("OXIDE_COMMIT") {
        Ok(c) if !c.is_empty() => c,
        _ => git_commit().unwrap_or_else(|| "unknown".into()),
    };
    println!("cargo:rustc-env=OXIDE_VERSION={version}");
    println!("cargo:rustc-env=OXIDE_COMMIT={commit}");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let packed = (numbers[0] << 48) | (numbers[1] << 32) | (numbers[2] << 16);
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/oxide.ico");
        resource.set("ProductName", "Oxide");
        resource.set("FileDescription", "Oxide");
        // Installed as Oxide.exe (the `daw` binary renamed).
        resource.set("OriginalFilename", "Oxide.exe");
        resource.set("FileVersion", &version);
        resource.set("ProductVersion", &format!("{version} ({commit})"));
        resource.set_version_info(winresource::VersionInfo::FILEVERSION, packed);
        resource.set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
        resource
            .compile()
            .expect("embedding the icon (rc.exe from the Windows SDK)");
    }
}

/// `major.minor.patch`, each below 65536 (a Windows version resource's 16 bits).
fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.').map(|p| p.parse::<u64>().ok());
    let numbers = [parts.next()??, parts.next()??, parts.next()??];
    (parts.next().is_none() && numbers.iter().all(|n| *n < 65536)).then_some(numbers)
}

/// The short hash of the checked-out commit, and the files that change when it does: `HEAD`,
/// the branches (a folder: a commit rewrites one of its files) and `packed-refs`. Only paths
/// that exist are watched, since Cargo reruns the script every build for a missing one.
fn git_commit() -> Option<String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let dir = PathBuf::from(git(&["rev-parse", "--absolute-git-dir"])?);
    watch(&dir.join("HEAD"));
    watch(&dir.join("refs").join("heads"));
    watch(&dir.join("packed-refs"));
    git(&["rev-parse", "--short", "HEAD"]).filter(|hash| !hash.is_empty())
}

fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
