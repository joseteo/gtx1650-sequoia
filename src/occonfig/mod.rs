//! Read, scan, plan, and write OpenCore `config.plist` files.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use plist::{Dictionary, Value};
use thiserror::Error;

pub mod fix;
pub mod scan;
pub use fix::{
    amfipass_registered, apply_fix, plan_fix, plan_is_noop, register_amfipass,
    register_nullmoth_safe, FixPlan, QuirkEdit,
};
pub use scan::{scan, Finding, FindingKind};

/// NVRAM GUID that owns OpenCore's `boot-args` and `csr-active-config`.
const NVRAM_GUID: &str = "7C436110-AB2A-4BBB-A880-FE41995C9F82";

/// Errors produced while loading, saving, or backing up an OpenCore config.
#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to read {}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },

    #[error("failed to parse {}: {source}", .path.display())]
    Parse { path: PathBuf, source: plist::Error },

    #[error("{}: plist root is not a dictionary", .path.display())]
    RootNotDictionary { path: PathBuf },

    #[error("failed to write {}: {source}", .path.display())]
    Write { path: PathBuf, source: io::Error },

    #[error("failed to serialize {}: {source}", .path.display())]
    Serialize { path: PathBuf, source: plist::Error },

    #[error("failed to back up {} to {}: {source}", .from.display(), .to.display())]
    Backup {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },

    #[error("{}: cannot derive a backup name (no file name)", .path.display())]
    NoFileName { path: PathBuf },

    #[error(
        "SecureBootModel must be downgraded to Disabled for the NVIDIA driver, but the change was not confirmed"
    )]
    SecureBootModelNotConfirmed,
}

/// An OpenCore `config.plist` loaded from disk.
#[derive(Debug)]
pub struct OcConfig {
    doc: Dictionary,
    path: PathBuf,
}

impl OcConfig {
    /// Loads the plist at `path`. Both XML and binary plists are accepted.
    pub fn load(path: &Path) -> Result<OcConfig, Error> {
        let file = fs::File::open(path).map_err(|source| Error::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let value = Value::from_reader(BufReader::new(file)).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let doc = value
            .into_dictionary()
            .ok_or_else(|| Error::RootNotDictionary {
                path: path.to_path_buf(),
            })?;
        Ok(OcConfig {
            doc,
            path: path.to_path_buf(),
        })
    }

    /// Writes the plist as XML back to the path it was loaded from. The XML
    /// goes to a temp sibling whose buffer is flushed explicitly — a late
    /// I/O error surfaces here instead of vanishing into a `BufWriter`
    /// drop-flush — and the temp file is then renamed over the target, so a
    /// failed write never leaves the boot config truncated.
    pub fn save(&self) -> Result<(), Error> {
        let file_name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::NoFileName {
                path: self.path.clone(),
            })?;
        let tmp = self
            .path
            .with_file_name(format!("{file_name}.tmp-greenlight-{}", std::process::id()));

        let write_result = (|| {
            let file = fs::File::create(&tmp).map_err(|source| Error::Write {
                path: tmp.clone(),
                source,
            })?;
            let mut w = BufWriter::new(file);
            Value::Dictionary(self.doc.clone())
                .to_writer_xml(&mut w)
                .map_err(|source| Error::Serialize {
                    path: self.path.clone(),
                    source,
                })?;
            w.flush().map_err(|source| Error::Write {
                path: tmp.clone(),
                source,
            })
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        write_result?;

        fs::rename(&tmp, &self.path).map_err(|source| Error::Write {
            path: self.path.clone(),
            source,
        })
    }

    /// Copies `path` to a sibling named
    /// `<name>.bak-greenlight-YYYYMMDD-HHMMSS` (UTC) and returns the new path.
    pub fn backup(path: &Path) -> Result<PathBuf, Error> {
        let file_name =
            path.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Error::NoFileName {
                    path: path.to_path_buf(),
                })?;
        let stamp = utc_timestamp(SystemTime::now());
        let backup_path = path.with_file_name(format!("{file_name}.bak-greenlight-{stamp}"));
        fs::copy(path, &backup_path).map_err(|source| Error::Backup {
            from: path.to_path_buf(),
            to: backup_path.clone(),
            source,
        })?;
        Ok(backup_path)
    }

    /// Returns the boot-args under `NVRAM → Add → 7C436110-…` as a set.
    /// Missing or malformed nodes yield an empty set.
    pub fn boot_args(&self) -> BTreeSet<String> {
        self.doc
            .get("NVRAM")
            .and_then(Value::as_dictionary)
            .and_then(|nvram| nvram.get("Add"))
            .and_then(Value::as_dictionary)
            .and_then(|add| add.get(NVRAM_GUID))
            .and_then(Value::as_dictionary)
            .and_then(|guid| guid.get("boot-args"))
            .and_then(Value::as_string)
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Stores `args` as a single space-joined `boot-args` string under
    /// `NVRAM → Add → 7C436110-…`, creating intermediate nodes if absent
    /// and replacing them if they hold the wrong type.
    pub fn set_boot_args(&mut self, args: &BTreeSet<String>) {
        let joined = args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        let nvram = dict_entry(&mut self.doc, "NVRAM");
        let add = dict_entry(nvram, "Add");
        let guid = dict_entry(add, NVRAM_GUID);
        guid.insert("boot-args".to_string(), Value::String(joined));
    }

    /// Returns the `DeviceProperties → Add` dictionary, or `None` if the
    /// config has no such node.
    pub fn device_properties(&self) -> Option<&Dictionary> {
        self.doc
            .get("DeviceProperties")
            .and_then(Value::as_dictionary)
            .and_then(|dp| dp.get("Add"))
            .and_then(Value::as_dictionary)
    }

    /// Returns the `DeviceProperties → Add` dictionary, or `None` if the
    /// config has no such node.
    pub fn device_properties_mut(&mut self) -> Option<&mut Dictionary> {
        self.doc
            .get_mut("DeviceProperties")
            .and_then(Value::as_dictionary_mut)
            .and_then(|dp| dp.get_mut("Add"))
            .and_then(Value::as_dictionary_mut)
    }
}

/// Returns `parent[key]` as a dictionary, inserting a fresh one when the key
/// is absent and replacing the value when it is not a dictionary.
fn dict_entry<'a>(parent: &'a mut Dictionary, key: &str) -> &'a mut Dictionary {
    let replace = match parent.get(key) {
        Some(value) => value.as_dictionary().is_none(),
        None => true,
    };
    if replace {
        parent.insert(key.to_string(), Value::Dictionary(Dictionary::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_dictionary_mut)
        .expect("just inserted a dictionary")
}

/// Formats `now` as `YYYYMMDD-HHMMSS` in UTC, using the civil-from-days
/// algorithm (Howard Hinnant) so no date crate is needed.
fn utc_timestamp(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (hour, minute, second) = (
        secs_of_day / 3600,
        secs_of_day % 3600 / 60,
        secs_of_day % 60,
    );

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}{m:02}{d:02}-{hour:02}{minute:02}{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn stamp(secs: u64) -> String {
        utc_timestamp(UNIX_EPOCH + Duration::from_secs(secs))
    }

    #[test]
    fn epoch_formats_as_19700101() {
        assert_eq!(stamp(0), "19700101-000000");
    }

    #[test]
    fn known_date_formats_correctly() {
        // 2026-01-01 00:00:00 UTC.
        assert_eq!(stamp(1_767_225_600), "20260101-000000");
    }

    #[test]
    fn leap_day_formats_correctly() {
        // 2024-02-29 12:34:56 UTC.
        assert_eq!(stamp(1_709_210_096), "20240229-123456");
    }
}
