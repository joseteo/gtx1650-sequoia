//! Locate the OpenCore ESP and its `config.plist`.
//!
//! This module answers one question — which `config.plist` actually boots
//! this machine — and refuses to guess: zero candidates is
//! [`Error::NoOpenCore`], more than one (e.g. an installer USB still plugged
//! in) is [`Error::Ambiguous`], exactly one is `Ok`.

use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use thiserror::Error;

/// Errors produced while locating the boot OpenCore config.
#[derive(Debug, Error)]
pub enum Error {
    #[error("no OpenCore config found on any EFI partition — install OpenCore first, or point at it with --efi-path <PATH>")]
    NoOpenCore,

    #[error(
        "found more than one OpenCore config; refusing to guess which one boots this machine:\n{}\nre-run with --efi-path <PATH> to choose one",
        join_paths(.0)
    )]
    Ambiguous(Vec<PathBuf>),

    #[error("--efi-path {}: no OpenCore config there (expected EFI/OC/config.plist, or config.plist itself if the directory is the OC directory)", .path.display())]
    OverrideNoConfig { path: PathBuf },

    #[error("failed to run `{command}`: {source}")]
    Command { command: String, source: io::Error },

    #[error("`{command}` failed: {detail}")]
    CommandFailed { command: String, detail: String },

    #[error("failed to parse `{command}` output: {source}")]
    CommandOutput {
        command: String,
        source: plist::Error,
    },

    #[error("`{command}` returned unexpected data: {detail}")]
    CommandShape { command: String, detail: String },

    #[error("could not mount EFI partition {identifier}: {detail}\nmounting an ESP can require admin rights — try `sudo diskutil mount {identifier}`")]
    MountFailed { identifier: String, detail: String },
}

/// Finds OpenCore configs on this machine's EFI partitions, mounting them
/// if the OS has not already. v1 ships [`MacosEspMounter`]; Windows and
/// Linux implementations land in plan 2. Tests use a fake over a tempdir
/// tree.
pub trait EspMounter {
    /// Returns the path of every `EFI/OC/config.plist` reachable on this
    /// machine's EFI partitions.
    fn list_esp_configs(&self) -> Result<Vec<PathBuf>, Error>;
}

/// Resolves which OpenCore `config.plist` actually boots this machine.
///
/// When `override_dir` is `Some` (the CLI's `--efi-path`), the mounter is
/// **not** consulted: the directory must contain `EFI/OC/config.plist` (an
/// ESP or EFI root), or be the OC directory itself (`config.plist` directly
/// inside it), and exactly that one config is returned. When `override_dir`
/// is `None`, the candidates come from `mounter`.
///
/// The answer is never a guess: zero candidates → [`Error::NoOpenCore`];
/// more than one → [`Error::Ambiguous`] naming every candidate and
/// demanding `--efi-path`; exactly one → `Ok`.
pub fn find_oc_configs(
    override_dir: Option<&Path>,
    mounter: &dyn EspMounter,
) -> Result<Vec<PathBuf>, Error> {
    let configs = match override_dir {
        Some(dir) => vec![override_config(dir).ok_or_else(|| Error::OverrideNoConfig {
            path: dir.to_path_buf(),
        })?],
        None => mounter.list_esp_configs()?,
    };
    match configs.len() {
        0 => Err(Error::NoOpenCore),
        1 => Ok(configs),
        _ => Err(Error::Ambiguous(configs)),
    }
}

/// Resolves an `--efi-path` directory to its config: `<dir>/EFI/OC/
/// config.plist` for an ESP/EFI root, else `<dir>/config.plist` when the
/// directory is the OC directory itself.
fn override_config(dir: &Path) -> Option<PathBuf> {
    [
        dir.join("EFI").join("OC").join("config.plist"),
        dir.join("config.plist"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// The macOS [`EspMounter`]: enumerates partitions with `diskutil list
/// -plist`, mounts each EFI partition that is not already mounted (tried
/// unprivileged — on failure the error message carries the `sudo diskutil
/// mount` hint), and collects every `EFI/OC/config.plist` found. A
/// partition that will not mount is an error, not a skip: silently
/// ignoring it could hide the config that really boots the machine.
///
/// Partitions mounted here stay mounted — the fix phase writes to the ESP.
pub struct MacosEspMounter;

impl EspMounter for MacosEspMounter {
    fn list_esp_configs(&self) -> Result<Vec<PathBuf>, Error> {
        let mut configs = Vec::new();
        for partition in list_efi_partitions()? {
            let mount_point = match &partition.mount_point {
                Some(mp) => PathBuf::from(mp),
                None => mount(&partition.identifier)?,
            };
            let candidate = mount_point.join("EFI").join("OC").join("config.plist");
            if candidate.is_file() {
                configs.push(candidate);
            }
        }
        Ok(configs)
    }
}

/// One EFI partition from `diskutil list -plist`.
struct EfiPartition {
    identifier: String,
    mount_point: Option<String>,
}

const DISKUTIL_LIST: &str = "diskutil list -plist";

/// Lists every partition whose `Content` is `EFI` across all disks.
fn list_efi_partitions() -> Result<Vec<EfiPartition>, Error> {
    let output = Command::new("diskutil")
        .args(["list", "-plist"])
        .output()
        .map_err(|source| Error::Command {
            command: DISKUTIL_LIST.to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            command: DISKUTIL_LIST.to_string(),
            detail: stderr_of(&output),
        });
    }
    let value = plist::Value::from_reader(Cursor::new(&output.stdout)).map_err(|source| {
        Error::CommandOutput {
            command: DISKUTIL_LIST.to_string(),
            source,
        }
    })?;
    let root = value.as_dictionary().ok_or_else(|| Error::CommandShape {
        command: DISKUTIL_LIST.to_string(),
        detail: "root is not a dictionary".to_string(),
    })?;
    let disks = root
        .get("AllDisksAndPartitions")
        .and_then(plist::Value::as_array)
        .ok_or_else(|| Error::CommandShape {
            command: DISKUTIL_LIST.to_string(),
            detail: "no AllDisksAndPartitions array".to_string(),
        })?;

    let mut partitions = Vec::new();
    for disk in disks {
        let Some(parts) = disk
            .as_dictionary()
            .and_then(|d| d.get("Partitions"))
            .and_then(plist::Value::as_array)
        else {
            continue;
        };
        for part in parts {
            let Some(dict) = part.as_dictionary() else {
                continue;
            };
            if dict.get("Content").and_then(plist::Value::as_string) != Some("EFI") {
                continue;
            }
            let Some(identifier) = dict
                .get("DeviceIdentifier")
                .and_then(plist::Value::as_string)
            else {
                continue;
            };
            let mount_point = dict
                .get("MountPoint")
                .and_then(plist::Value::as_string)
                .filter(|mp| !mp.is_empty())
                .map(str::to_string);
            partitions.push(EfiPartition {
                identifier: identifier.to_string(),
                mount_point,
            });
        }
    }
    Ok(partitions)
}

/// Mounts the EFI partition `identifier` and returns its mount point.
/// Tried unprivileged; on failure the error carries the `sudo diskutil
/// mount` hint.
fn mount(identifier: &str) -> Result<PathBuf, Error> {
    let mount_cmd = format!("diskutil mount {identifier}");
    let output = Command::new("diskutil")
        .args(["mount", identifier])
        .output()
        .map_err(|source| Error::Command {
            command: mount_cmd.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::MountFailed {
            identifier: identifier.to_string(),
            detail: stderr_of(&output),
        });
    }

    // `diskutil mount` prints prose; the authoritative mount point comes
    // from `diskutil info -plist`.
    let info_cmd = format!("diskutil info -plist {identifier}");
    let output = Command::new("diskutil")
        .args(["info", "-plist", identifier])
        .output()
        .map_err(|source| Error::Command {
            command: info_cmd.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::MountFailed {
            identifier: identifier.to_string(),
            detail: stderr_of(&output),
        });
    }
    let value = plist::Value::from_reader(Cursor::new(&output.stdout)).map_err(|source| {
        Error::CommandOutput {
            command: info_cmd.clone(),
            source,
        }
    })?;
    let mount_point = value
        .as_dictionary()
        .and_then(|d| d.get("MountPoint"))
        .and_then(plist::Value::as_string)
        .filter(|mp| !mp.is_empty())
        .ok_or_else(|| Error::MountFailed {
            identifier: identifier.to_string(),
            detail: "mounted, but no mount point reported".to_string(),
        })?;
    Ok(PathBuf::from(mount_point))
}

/// Formats one candidate path per line, indented, for the ambiguity error.
fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| format!("  {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Returns trimmed stderr for diagnostics, falling back to stdout when the
/// failure was reported there.
fn stderr_of(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    }
}
