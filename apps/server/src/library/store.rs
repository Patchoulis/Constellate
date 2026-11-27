//! Reading and writing the small files several threads and processes share (caches, settings, a run's results).
//!
//! Three rules, each learned the hard way:
//!
//! * A file is replaced, never rewritten in place: it is written to a temporary file beside it and moved over
//!   the old one, so a reader sees the old content or the new, never half of either.
//! * On Windows that move fails while anybody has the old file open (a reader of ours, a sync client, a virus
//!   scanner), so it is tried again for a few seconds before giving up, and the temporary file never stays
//!   behind.
//! * Read-change-write happens under a lock ([`update_json`]): one inside this process, and a lock file beside
//!   the data so that a second process waits its turn too.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// How long to keep trying to replace a file that is open elsewhere.
const PATIENCE: Duration = Duration::from_secs(5);
/// A lock file older than this was left by a process that died.
const STALE: Duration = Duration::from_secs(60);

/// One read-change-write at a time inside this process.
static UPDATES: Mutex<()> = Mutex::new(());
/// Tells this process's temporary files apart.
static TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// What can go wrong with a file, always with the file's name.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The file is there but is not the JSON it should be (a sync program, an edit by hand).
    #[error("{} is not valid JSON: {source}", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> StoreError + '_ {
    move |source| StoreError::Io { path: path.to_path_buf(), source }
}

/// Put `content` at `path`: written beside it, then moved into place.
pub fn replace(path: &Path, content: &[u8]) -> Result<(), StoreError> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(io_error(folder))?;
    }
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let unique = TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!("{name}.{}.{unique}.tmp", std::process::id()));
    let moved = fs::write(&temporary, content).and_then(|()| move_into_place(&temporary, path));
    if moved.is_err() {
        let _ = fs::remove_file(&temporary); // never left behind; gone already if the move succeeded
    }
    moved.map_err(io_error(path))
}

fn move_into_place(temporary: &Path, path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        match fs::rename(temporary, path) {
            // Windows: the destination is open somewhere
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// The file's content, or `None` when there is no such file.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(path)(error)),
    };
    serde_json::from_str(&text).map(Some).map_err(|source| StoreError::Json { path: path.to_path_buf(), source })
}

pub fn write_json<T: Serialize + ?Sized>(path: &Path, content: &T) -> Result<(), StoreError> {
    let text = serde_json::to_vec(content).map_err(|source| StoreError::Json { path: path.to_path_buf(), source })?;
    replace(path, &text)
}

/// `<path>.lock`, held from creation until dropped. Created exclusively, so only one process can have it.
struct BetweenProcesses(PathBuf);

impl BetweenProcesses {
    fn hold(path: &Path, stale: Duration) -> Result<Self, StoreError> {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let lock = path.with_file_name(format!("{name}.lock"));
        if let Some(folder) = lock.parent() {
            fs::create_dir_all(folder).map_err(io_error(folder))?;
        }
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(_) => return Ok(Self(lock)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let age = fs::metadata(&lock)
                        .and_then(|found| found.modified())
                        .map(|written| SystemTime::now().duration_since(written).unwrap_or_default());
                    match age {
                        Ok(age) if age >= stale => {
                            let _ = fs::remove_file(&lock); // its owner is gone
                        }
                        Ok(_) => thread::sleep(Duration::from_millis(20)),
                        Err(_) => {} // released just now: try again at once
                    }
                }
                // Windows answers "access denied" while the previous holder's delete is still pending
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(io_error(&lock)(error)),
            }
        }
    }
}

impl Drop for BetweenProcesses {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Read a JSON object, let `change` alter it, write it back: as one step that neither another thread nor
/// another process can interleave. A file that does not exist yet starts as an empty object.
pub fn update_json(
    path: &Path,
    change: impl FnOnce(&mut Map<String, Value>),
) -> Result<Map<String, Value>, StoreError> {
    update_json_with(path, STALE, change)
}

fn update_json_with(
    path: &Path,
    stale: Duration,
    change: impl FnOnce(&mut Map<String, Value>),
) -> Result<Map<String, Value>, StoreError> {
    // a thread that panicked while holding the lock has not left the file half-written: it is replaced whole
    let _within = UPDATES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _between = BetweenProcesses::hold(path, stale)?;
    let mut content: Map<String, Value> = read_json(path)?.unwrap_or_default();
    change(&mut content);
    write_json(path, &content)?;
    Ok(content)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn files_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_is_replaced_whole_and_nothing_temporary_stays() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("deep").join("settings.json");
        replace(&path, b"first").unwrap();
        replace(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(files_in(path.parent().unwrap()), ["settings.json"]);
    }

    #[test]
    fn a_missing_file_reads_as_nothing_and_a_damaged_one_says_which_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("ledger.json");
        assert_eq!(read_json::<Value>(&path).unwrap(), None);
        fs::write(&path, "{ not json").unwrap();
        let error = read_json::<Value>(&path).unwrap_err();
        assert!(matches!(error, StoreError::Json { .. }));
        assert!(error.to_string().contains("ledger.json"));
    }

    #[test]
    fn what_is_written_is_what_is_read() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("run.json");
        write_json(&path, &json!({"papers": 12487, "title": "NeurIPS 2025 + ICML 2026"})).unwrap();
        let read: Value = read_json(&path).unwrap().unwrap();
        assert_eq!(read["papers"], 12487);
    }

    #[test]
    fn updates_from_many_threads_are_all_kept() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("counts.json");
        thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..25 {
                        update_json(&path, |content| {
                            let seen = content.get("calls").and_then(Value::as_u64).unwrap_or(0);
                            content.insert("calls".into(), json!(seen + 1));
                        })
                        .unwrap();
                    }
                });
            }
        });
        let read: Value = read_json(&path).unwrap().unwrap();
        assert_eq!(read["calls"], 200);
        assert_eq!(files_in(folder.path()), ["counts.json"]); // no lock or temporary file left
    }

    #[test]
    fn a_lock_left_by_a_process_that_died_is_taken_over() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("rates.json");
        fs::write(folder.path().join("rates.json.lock"), "").unwrap();
        let written = update_json_with(&path, Duration::ZERO, |content| {
            content.insert("CAD".into(), json!(1.42));
        })
        .unwrap();
        assert_eq!(written["CAD"], 1.42);
        assert_eq!(files_in(folder.path()), ["rates.json"]);
    }

    #[test]
    fn a_damaged_file_is_not_overwritten_by_an_update() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("usage.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(update_json(&path, |content| content.clear()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json"); // left for a person to look at
        assert_eq!(files_in(folder.path()), ["usage.json"]);
    }
}
