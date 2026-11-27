//! A run's files, in the runs folder of the data folder, laid out as the Python laid them out so that runs made by
//! either open in the other:
//!
//! ```text
//! runs/<name>/run.json          what was selected, the settings, the papers' ids in the order of the graph's nodes
//!             communities.json  the tree of communities (RunResult in the contracts)
//!             graph.json        the papers and their links, for the graph view (GraphData)
//!             centroids.npy     the mean embedding of every community (float32)
//!             report.md         the readable report
//!             title.txt         the title the person gave it, when they gave one
//! ```
//!
//! The Python also wrote `adjacency.npy`, the whole similarity matrix (600 MB for 12,000 papers), which nothing
//! read again; it is not written any more, and one left by the Python is left alone.
//!
//! A run is a folder of the runs folder with `run.json` and `communities.json`. Writing a run removes its
//! `communities.json` first and writes it last, every file replaced whole ([`store::replace`]): a run stopped
//! while its files were written is not shown half old and half new, it is not shown until it is written again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ndarray::ArrayView2;
use serde::Serialize;
use serde_json::Value;

use crate::library::store::{self, StoreError};
use crate::selection::Selection;

pub const RUNS: &str = "runs";
pub const RUN: &str = "run.json";
pub const COMMUNITIES: &str = "communities.json";
pub const GRAPH: &str = "graph.json";
pub const CENTROIDS: &str = "centroids.npy";
pub const REPORT: &str = "report.md";
pub const TITLE: &str = "title.txt";
/// The longest title kept, in characters.
pub const LONGEST_TITLE: usize = 120;

pub fn runs_folder(data: &Path) -> PathBuf {
    data.join(RUNS)
}

/// Why a run's folder is not there to be read.
#[derive(Debug, thiserror::Error)]
pub enum RunFileError {
    #[error("no such run")]
    NotFound,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{} is not what a run's file should be: {reason}", path.display())]
    Unreadable { path: PathBuf, reason: String },
}

/// The folder of the run `name`: exactly a folder inside the runs folder (not a link to elsewhere), with its
/// `run.json`. `name` must already be one plain name (`api::routes::one_name`); this checks again on disk.
pub fn folder_of(data: &Path, name: &str) -> Result<PathBuf, RunFileError> {
    let runs = runs_folder(data);
    let folder = runs.join(name);
    let plain = Path::new(name).file_name().is_some_and(|file| file == name);
    let is_folder = fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir());
    if !plain || !is_folder || !folder.join(RUN).is_file() {
        return Err(RunFileError::NotFound);
    }
    let (Ok(inside), Ok(runs)) = (folder.canonicalize(), runs.canonicalize()) else {
        return Err(RunFileError::NotFound);
    };
    if inside.parent() != Some(runs.as_path()) {
        return Err(RunFileError::NotFound);
    }
    Ok(folder)
}

/// The title a run is shown under: the one it was given, or its name.
pub fn title(folder: &Path, name: &str) -> String {
    fs::read_to_string(folder.join(TITLE))
        .map(|text| text.trim().to_owned())
        .ok()
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| name.to_owned())
}

/// A title as it is kept: its words with single spaces, at most [`LONGEST_TITLE`] characters (as the Python).
pub fn clean_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(LONGEST_TITLE).collect()
}

/// Give a run another title, or its name back when the title is empty or the name. Answers the title shown.
pub fn retitle(folder: &Path, name: &str, title: &str) -> Result<String, StoreError> {
    let title = clean_title(title);
    let file = folder.join(TITLE);
    if !title.is_empty() && title != name {
        store::replace(&file, title.as_bytes())?;
    } else {
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(StoreError::Io { path: file, source }),
        }
    }
    Ok(self::title(folder, name))
}

/// What `run.json` says was selected: the selection made again from it, as the Python's `rerun` did.
pub fn stored_selection(folder: &Path) -> Result<Selection, RunFileError> {
    let path = folder.join(RUN);
    let stored: Value = store::read_json(&path)?.ok_or(RunFileError::NotFound)?;
    let selection = &stored["selection"];
    let strings = |key: &str| -> Vec<String> {
        selection[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
    };
    Selection::parse(
        &strings("venues"),
        selection["start"].as_str(),
        selection["end"].as_str(),
        &strings("categories"),
        selection["include_workshops"].as_bool().unwrap_or(false),
    )
    .map_err(|error| RunFileError::Unreadable { path, reason: error.to_string() })
}

/// The ids of a run's papers, in the order of its graph's nodes.
pub fn paper_ids(folder: &Path) -> Result<Vec<String>, RunFileError> {
    let path = folder.join(RUN);
    let stored: Value = store::read_json(&path)?.ok_or(RunFileError::NotFound)?;
    let ids = stored["paper_ids"]
        .as_array()
        .ok_or_else(|| RunFileError::Unreadable { path: path.clone(), reason: "it has no paper_ids".to_owned() })?;
    Ok(ids.iter().filter_map(Value::as_str).map(str::to_owned).collect())
}

/// The settings that decide a run's communities: which papers go in and how they are cut. The others (how many
/// links are drawn, which model names the communities) leave the communities as they are.
pub const SHAPING: [&str; 11] = [
    "embedding_model",
    "center_embeddings",
    "clustering",
    "max_communities",
    "min_community_size",
    "min_split_modularity",
    "sub_levels",
    "detect_ai",
    "include_ai_papers",
    "acceptance_filter",
    "exclude_flags",
];

/// What making a run again in its folder would do to its communities, said before it is made again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Remade {
    /// They are cut again otherwise: a setting that shapes them differs from the one the run was made with, or the
    /// run was made by another clustering (the Python's k-means, or an earlier version of the engine's).
    Change,
    /// The same settings and the same clustering: they come out the same, unless the papers have changed (more of
    /// them listed since, or downloaded or embedded again).
    MayChange,
}

/// A setting that shapes the communities, as the run was made with it and as it is now. `was` is null when the
/// run did not record it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Differs {
    pub setting: &'static str,
    pub was: Value,
    pub now: Value,
}

/// The run a request would make again in place (`RunReplaced` in the contracts): what the person loses.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Replaced {
    pub run: String,
    pub title: String,
    /// Its communities and sub-communities now, and how many of them have a name.
    pub communities: usize,
    pub named: usize,
    pub communities_will: Remade,
    pub changed: Vec<Differs>,
}

/// What making the run in `folder` again with `settings` would replace. The run's `run.json` says what it was
/// made with; `clustering_version` (absent in runs the Python made) says by which clustering.
pub fn replacing(folder: &Path, name: &str, settings: &Value, clustering: u32) -> Result<Replaced, RunFileError> {
    let path = folder.join(RUN);
    let stored: Value = store::read_json(&path)?.ok_or(RunFileError::NotFound)?;
    let changed: Vec<Differs> = SHAPING
        .iter()
        .filter_map(|setting| {
            let was = stored["settings"].get(*setting).cloned().unwrap_or(Value::Null);
            let now = settings.get(*setting).cloned().unwrap_or(Value::Null);
            (was != now).then_some(Differs { setting, was, now })
        })
        .collect();
    // the Python's bisection has no random start, and the engine's cuts the same papers the same way (measured on
    // a run of 12,487 papers: the same 24 communities); its k-means did not
    let same_clustering = match stored.get("clustering_version").and_then(Value::as_u64) {
        Some(version) => version == u64::from(clustering),
        None => stored["settings"]["clustering"] == "fiedler",
    };
    let (communities, named) = match crate::writing::read_tree(folder)? {
        Some(tree) => {
            fn named(stored: &[crate::writing::Stored]) -> usize {
                stored
                    .iter()
                    .map(|community| {
                        usize::from(community.name.as_deref().is_some_and(|name| !name.trim().is_empty()))
                            + named(&community.children)
                    })
                    .sum()
            }
            (tree.paths().len(), named(&tree.communities))
        }
        None => (0, 0),
    };
    Ok(Replaced {
        run: name.to_owned(),
        title: title(folder, name),
        communities,
        named,
        communities_will: if changed.is_empty() && same_clustering { Remade::MayChange } else { Remade::Change },
        changed,
    })
}

/// A float32 matrix as NumPy's `.npy` (version 1, little-endian, C order), which the Python wrote and read.
pub fn npy(matrix: ArrayView2<f32>) -> Vec<u8> {
    let (rows, columns) = matrix.dim();
    let mut header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({rows}, {columns}), }}");
    // the header (with the 10 bytes before it and its newline) is padded to a multiple of 64 bytes
    let unpadded = 10 + header.len() + 1;
    header.push_str(&" ".repeat((64 - unpadded % 64) % 64));
    header.push('\n');
    let mut bytes = Vec::with_capacity(10 + header.len() + rows * columns * 4);
    bytes.extend_from_slice(b"\x93NUMPY\x01\x00");
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    for value in matrix.iter() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Everything a run writes.
pub struct Written<'a, R: Serialize, G: Serialize, S: Serialize> {
    pub run: &'a S,
    pub communities: &'a R,
    pub graph: &'a G,
    pub centroids: ArrayView2<'a, f32>,
    pub report: &'a str,
}

/// Write a run's files into `folder` (made if need be), `communities.json` last. Its title is left as it is, so a
/// run made again keeps the title it was given.
pub fn write<R: Serialize, G: Serialize, S: Serialize>(
    folder: &Path,
    files: &Written<'_, R, G, S>,
) -> Result<(), StoreError> {
    fs::create_dir_all(folder).map_err(|source| StoreError::Io { path: folder.to_path_buf(), source })?;
    let communities = folder.join(COMMUNITIES);
    match fs::remove_file(&communities) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(StoreError::Io { path: communities, source }),
    }
    store::write_json(&folder.join(RUN), files.run)?;
    store::write_json(&folder.join(GRAPH), files.graph)?;
    store::replace(&folder.join(CENTROIDS), &npy(files.centroids))?;
    store::replace(&folder.join(REPORT), files.report.as_bytes())?;
    store::write_json(&communities, files.communities)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn only_a_folder_inside_the_runs_folder_with_its_run_file_is_a_run() {
        let data = tempfile::tempdir().unwrap();
        let runs = runs_folder(data.path());
        fs::create_dir_all(runs.join("demo")).unwrap();
        fs::write(runs.join("demo").join(RUN), "{}").unwrap();
        fs::create_dir_all(runs.join("empty")).unwrap();
        fs::write(data.path().join(RUN), "{}").unwrap(); // a decoy one level up
        assert!(folder_of(data.path(), "demo").is_ok());
        for name in ["..", ".", "nope", "empty", "demo/..", "../demo", "demo/../demo", ""] {
            assert!(matches!(folder_of(data.path(), name), Err(RunFileError::NotFound)), "{name:?}");
        }
    }

    #[test]
    fn a_title_is_tidied_kept_beside_the_run_and_dropped_when_empty_or_the_name() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(retitle(folder.path(), "demo", "  My   run ").unwrap(), "My run");
        assert_eq!(fs::read_to_string(folder.path().join(TITLE)).unwrap(), "My run");
        assert_eq!(retitle(folder.path(), "demo", "").unwrap(), "demo");
        assert!(!folder.path().join(TITLE).exists());
        assert_eq!(retitle(folder.path(), "demo", "demo").unwrap(), "demo");
        assert!(!folder.path().join(TITLE).exists());
        assert_eq!(retitle(folder.path(), "demo", &"é".repeat(300)).unwrap().chars().count(), LONGEST_TITLE);
    }

    #[test]
    fn a_matrix_is_written_as_numpy_writes_it_and_read_back_by_the_import() {
        let matrix = ndarray::array![[1.0_f32, 2.0, 3.0], [4.0, 5.0, 6.5]];
        let bytes = npy(matrix.view());
        assert_eq!((10 + u16::from_le_bytes([bytes[8], bytes[9]]) as usize) % 64, 0);
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("m.npy");
        fs::write(&path, &bytes).unwrap();
        let (rows, columns, data) = crate::library::import::read_npy(&path).unwrap();
        assert_eq!((rows, columns), (2, 3));
        assert_eq!(f32::from_le_bytes([data[20], data[21], data[22], data[23]]), 6.5);
    }

    #[test]
    fn the_selection_of_a_run_made_by_the_python_is_read_back() {
        let folder = tempfile::tempdir().unwrap();
        let python = r#"{"selection": {"venues": ["NeurIPS 2025", "ICML 2026"], "start": null, "end": null,
            "categories": [], "include_workshops": false}, "settings": {"clustering": "spectral"}, "paper_ids": ["a", "b"]}"#;
        fs::write(folder.path().join(RUN), python).unwrap();
        assert_eq!(stored_selection(folder.path()).unwrap().name(), "neurips2025_icml2026");
        assert_eq!(paper_ids(folder.path()).unwrap(), ["a", "b"]);
        let dates = r#"{"selection": {"venues": [], "start": "2026-11-01", "end": "2026-11-07", "categories": ["cs.LG"],
            "include_workshops": false}, "paper_ids": []}"#;
        fs::write(folder.path().join(RUN), dates).unwrap();
        assert_eq!(stored_selection(folder.path()).unwrap().name(), "arxiv_2026-11-01_2026-11-07_cs.LG");
    }
}
