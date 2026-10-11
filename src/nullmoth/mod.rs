//! NullMoth driver payload: install and post-boot verification.
//!
//! The install is never reimplemented in Rust (spec §11.6): the
//! hash-verified release tarball is extracted to a staging dir and its own
//! `install.sh` runs via sudo — the script already ships the rollback
//! snapshot machinery. When the process is not root, the install cannot
//! run: the exact sudo re-run command is printed and the driver phase stays
//! pending (Review Focus 5 — the binary never sudo-reexecs itself).

use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::detect::Hardware;
use crate::fetch::{self, Http};

pub mod check;

/// Directory the verified tarball is extracted into for the install run —
/// the staging location INSTALL-GUIDE §11 documents.
const STAGING_DIR: &str = "/Users/Shared/nullmoth-stage";

/// `csr-active-config` value permissive enough for the NullMoth driver.
const SIP_PERMISSIVE: &str = "0x0a43";

/// Errors produced while installing the NullMoth payload.
#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create staging dir {}: {source}", .path.display())]
    Staging { path: PathBuf, source: io::Error },

    #[error("failed to run `{command}`: {source}")]
    Command { command: String, source: io::Error },

    #[error("`{command}` failed: {detail}")]
    CommandFailed { command: String, detail: String },

    #[error("failed to parse `hdiutil attach -plist` output: {0}")]
    HdiutilOutput(plist::Error),

    #[error("hdiutil attached {} but reported no mount point", .0.display())]
    NoMountPoint(PathBuf),

    #[error("NullMothSafe.efi not found under {} (expected 1401.app/Contents/Resources/NullMothSafe.efi)", .0.display())]
    NoNullMothSafe(PathBuf),

    #[error("failed to copy {} to {}: {source}", .from.display(), .to.display())]
    Copy {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },

    #[error("{}: no pkgroot/install.sh in the extracted tarball", .0.display())]
    NoInstallScript(PathBuf),

    #[error("`sudo ./install.sh` in {} failed: {detail}", .dir.display())]
    Install { dir: PathBuf, detail: String },
}

/// Installs the hash-verified NullMoth tarball.
///
/// - `dry_run`: prints the exact commands, touches nothing.
/// - non-root process: prints the exact sudo re-run command plus a "driver
///   phase pending" note and returns `Ok(())` — the caller exits 0 and the
///   user finishes the driver phase with sudo (Review Focus 5).
/// - root: extracts the tarball into [`STAGING_DIR`] and runs
///   `sudo ./install.sh` from the extracted `pkgroot` (spec §11.6).
pub fn install_nullmoth(payload: &Path, dry_run: bool) -> Result<(), Error> {
    let staging = Path::new(STAGING_DIR);
    let pkgroot = staging.join("pkgroot");
    if dry_run {
        println!("driver phase (dry-run) — the commands that would run:");
        println!("  mkdir -p {}", staging.display());
        println!("  tar -xzf {} -C {}", payload.display(), staging.display());
        println!("  cd {} && sudo ./install.sh", pkgroot.display());
        return Ok(());
    }
    if euid() != Some(0) {
        print_driver_pending();
        return Ok(());
    }

    fs::create_dir_all(staging).map_err(|source| Error::Staging {
        path: staging.to_path_buf(),
        source,
    })?;
    let tar = format!("tar -xzf {} -C {}", payload.display(), staging.display());
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(payload)
        .arg("-C")
        .arg(staging)
        .status()
        .map_err(|source| Error::Command {
            command: tar.clone(),
            source,
        })?;
    if !status.success() {
        return Err(Error::CommandFailed {
            command: tar,
            detail: format!("exit status {status}"),
        });
    }
    if !pkgroot.join("install.sh").is_file() {
        return Err(Error::NoInstallScript(pkgroot));
    }
    let install = format!("cd {} && sudo ./install.sh", pkgroot.display());
    let status = Command::new("sudo")
        .arg("./install.sh")
        .current_dir(&pkgroot)
        .status()
        .map_err(|source| Error::Command {
            command: install.clone(),
            source,
        })?;
    if !status.success() {
        return Err(Error::Install {
            dir: pkgroot,
            detail: format!("exit status {status}"),
        });
    }
    println!("NullMoth driver installed — reboot, then verify with `greenlight fix --check`.");
    Ok(())
}

/// True when this process runs as root (euid 0) — the driver install needs
/// it. Callers check this *before* fetching anything: deferring needs no
/// network.
pub fn running_as_root() -> bool {
    euid() == Some(0)
}

/// The non-root deferral: a "driver phase pending" note plus the exact sudo
/// re-run command. Printed instead of installing whenever the process is
/// not root (Review Focus 5 — the binary never sudo-reexecs itself).
pub fn print_driver_pending() {
    println!("driver phase pending: installing the NullMoth driver requires root.");
    println!("re-run this same command with sudo to finish:");
    println!("  sudo {}", full_command_string());
}

/// True when SIP looks permissive enough for the NullMoth driver:
/// `csr-active-config` pinned at `0x0a43`, or SIP fully disabled. Anything
/// else — including an unknown status — is false. Callers warn; SIP never
/// blocks the fix flow.
pub fn sip_ok(hw: &Hardware) -> bool {
    match hw.sip.as_deref() {
        Some(status) => {
            let status = status.trim().to_ascii_lowercase();
            status == SIP_PERMISSIVE || status == "disabled"
        }
        None => false,
    }
}

/// Best-effort fetch of `NullMothSafe.efi`, the boot-picker recovery tool
/// (spec §11.3): it ships inside the 1401.app DMG asset of the latest
/// NullMoth release, not in the tarball. The DMG is hash-verified against
/// its own SHA256SUMS.txt line, mounted read-only, the .efi copied out, and
/// the DMG detached. **Any** failure — no DMG asset, no sums line, a
/// checksum mismatch, an `hdiutil` error — degrades to `Ok(None)`; the
/// caller then prints the manual OpenShell recovery steps instead.
///
/// Never exercised against a real DMG in automated tests (no `hdiutil`);
/// the `None` paths are covered with a fixture `Http`.
pub fn try_fetch_nullmoth_safe(http: &dyn Http, dest: &Path) -> Result<Option<PathBuf>, Error> {
    let Ok(dmg) = fetch::fetch_nullmoth_safe_dmg(http, dest) else {
        return Ok(None);
    };
    match extract_nullmoth_safe(&dmg.path, dest) {
        Ok(path) => Ok(Some(path)),
        Err(_) => Ok(None),
    }
}

/// Mounts `dmg` read-only, copies `1401.app/Contents/Resources/
/// NullMothSafe.efi` to `dest`, and detaches — the detach runs even when
/// the copy fails.
fn extract_nullmoth_safe(dmg: &Path, dest: &Path) -> Result<PathBuf, Error> {
    let attach = format!("hdiutil attach -nobrowse -readonly {}", dmg.display());
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-plist"])
        .arg(dmg)
        .output()
        .map_err(|source| Error::Command {
            command: attach.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            command: attach,
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let mount_point = mount_point_of(&output.stdout, dmg)?;

    let result = (|| {
        let efi = mount_point
            .join("1401.app")
            .join("Contents")
            .join("Resources")
            .join("NullMothSafe.efi");
        if !efi.is_file() {
            return Err(Error::NoNullMothSafe(mount_point.clone()));
        }
        let out = dest.join("NullMothSafe.efi");
        fs::copy(&efi, &out).map_err(|source| Error::Copy {
            from: efi,
            to: out.clone(),
            source,
        })?;
        Ok(out)
    })();

    let detach = format!("hdiutil detach {}", mount_point.display());
    let detach_status = Command::new("hdiutil")
        .arg("detach")
        .arg(&mount_point)
        .status();
    match detach_status {
        Ok(status) if status.success() => {}
        Ok(status) if result.is_ok() => {
            return Err(Error::CommandFailed {
                command: detach,
                detail: format!("exit status {status}"),
            })
        }
        Err(source) if result.is_ok() => {
            return Err(Error::Command {
                command: detach,
                source,
            })
        }
        _ => {}
    }
    result
}

/// Parses the mount point out of `hdiutil attach -plist` output: the last
/// `system-entities` entry carrying a `mount-point` key.
fn mount_point_of(plist_bytes: &[u8], dmg: &Path) -> Result<PathBuf, Error> {
    let value =
        plist::Value::from_reader(Cursor::new(plist_bytes)).map_err(Error::HdiutilOutput)?;
    value
        .as_dictionary()
        .and_then(|root| root.get("system-entities"))
        .and_then(plist::Value::as_array)
        .and_then(|entities| {
            entities.iter().rev().find_map(|entity| {
                entity
                    .as_dictionary()
                    .and_then(|d| d.get("mount-point"))
                    .and_then(plist::Value::as_string)
            })
        })
        .map(PathBuf::from)
        .ok_or_else(|| Error::NoMountPoint(dmg.to_path_buf()))
}

/// The effective uid via `id -u`; `None` (→ treated as non-root) when the
/// probe itself fails.
fn euid() -> Option<u32> {
    let output = Command::new("id").arg("-u").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// This invocation's full command line, for the sudo re-run hint: argv[0]
/// canonicalized to an absolute path (users run the built binary at
/// `./target/release/greenlight` — it is not on PATH, theirs or sudo's) and
/// every argument POSIX single-quoted, so the printed command pastes
/// verbatim from any directory. Falls back to the bare `greenlight` name
/// when argv[0] cannot be canonicalized.
fn full_command_string() -> String {
    let mut args = std::env::args_os();
    let argv0 = args
        .next()
        .and_then(|a| fs::canonicalize(a).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "greenlight".to_string());
    let mut parts = vec![shell_quote(&argv0)];
    parts.extend(args.map(|a| shell_quote(&a.to_string_lossy())));
    parts.join(" ")
}

/// POSIX single-quote escaping: wrap in single quotes, writing each
/// embedded single quote as `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes_for_pasting() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("with space"), "'with space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
