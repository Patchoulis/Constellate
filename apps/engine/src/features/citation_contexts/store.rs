//! The citing sentences of each paper ([`crate::features::citation_contexts`]), read once and kept by paper, and
//! who cites whom among the library's papers, so that "the sentences citing this paper" is one indexed look-up.
//!
//! A paper's contexts are read from its passages (kept by [`crate::library::papers::passages`]) and its reference list, which is
//! read from the PDF's text as `pdf-extract` gives it (the passages leave the list out). They are kept with the
//! passages reader's version, the version of what is read ([`CONTEXTS`]) and the SHA-256 of the PDF they were read
//! from: kept contexts count only while the passages kept for the paper are of that reader and that PDF, so that
//! their addresses are the passages' addresses. Each paper's entries are linked to the library as it is when they
//! are kept; [`Library::citation_contexts`] links kept ones again, so a paper added since is found.
//!
//! Many papers are read on every core (rayon); their passages come from [`Library::paper_texts`], called between
//! rounds and never from inside a rayon task.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};

use rayon::prelude::*;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::analysis::citations::{LibraryPaper, Link, Linker};
use crate::features::citation_contexts::{CONTEXTS, PaperContexts, contexts_of};
use crate::library::{Library, LibraryError, PaperReading, json_column, now, placeholders};
use crate::text::pdf::passages::{Passages, READER};
use crate::text::pdf::pdf_pages;
use crate::text::pdf::references::read_reference_list;

/// Papers read in one round: their passages are held in memory meanwhile.
const AT_ONCE: usize = 200;

/// What reading a paper's citing sentences came to.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextsReading {
    Read(PaperContexts),
    /// The library has no PDF of it.
    NoPdf,
    /// Its PDF cannot be read, for the reason given.
    Unreadable(String),
    NotInLibrary,
}

/// Which entries of kept contexts to load: all of them, or only those that are papers of the library (much less).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entries {
    All,
    InLibrary,
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Is the paper's PDF on disk still the file its kept passages were read from, as far as its size and time of
/// change tell (what [`crate::library::papers::passages`] stamps a reading with)? A PDF replaced since leaves the passages' row as
/// it was until the paper's text is next asked for ([`Library::paper_texts`]); until then the contexts kept with
/// those passages are of the old file, and do not hold. Told without reading the file, so that a community's
/// figures cost one look at each file, not the decoding of every paper's passages.
fn same_file(library: &Library, paper_id: &str, size: i64, modified: i64) -> bool {
    let Ok(found) = fs::metadata(library.pdf_file(paper_id)) else { return false };
    let changed = found
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| i64::try_from(since.as_nanos()).unwrap_or(i64::MAX));
    found.is_file() && i64::try_from(found.len()).unwrap_or(i64::MAX) == size && changed == modified
}

/// Read one paper's contexts: its reference list from its PDF's text, its sentences from `passages`. The PDF's
/// hash comes with them.
fn read_one(
    library: &Library,
    id: &str,
    passages: &Passages,
    linker: &Linker,
) -> Result<(String, PaperContexts), ContextsReading> {
    let bytes = fs::read(library.pdf_file(id)).map_err(|_| ContextsReading::NoPdf)?;
    let pages = pdf_pages(&bytes).map_err(|why| ContextsReading::Unreadable(why.to_string()))?;
    let list = read_reference_list(&pages.join("\n"));
    let contexts = catch_unwind(AssertUnwindSafe(|| contexts_of(passages, &list, linker, id)))
        .map_err(|_| ContextsReading::Unreadable("its citing sentences could not be read".to_owned()))?;
    Ok((sha256(&bytes), contexts))
}

/// `contexts` with only the entries that are papers of the library.
fn in_library(contexts: &PaperContexts) -> PaperContexts {
    PaperContexts {
        entries: contexts
            .entries
            .iter()
            .filter(|entry| matches!(entry.link, Link::InLibrary { .. }))
            .cloned()
            .collect(),
        ..contexts.clone()
    }
}

impl Library {
    /// Every paper of the library as the citation network's [`Linker`] matches references to it.
    pub fn link_papers(&self) -> Result<Vec<LibraryPaper>, LibraryError> {
        self.read(|connection| {
            let mut statement =
                connection.prepare("SELECT id, title, authors, published, url, origin FROM papers ORDER BY id")?;
            let rows = statement.query_map([], |row| {
                let id: String = row.get(0)?;
                let published: String = row.get(3)?;
                let origin: String = row.get(5)?;
                Ok(LibraryPaper {
                    arxiv: (origin == "arxiv").then(|| id.clone()),
                    title: row.get(1)?,
                    authors: json_column(row, 2)?,
                    year: published.get(..4).and_then(|year| year.parse().ok()),
                    venue: None,
                    doi: None,
                    url: row.get(4)?,
                    id,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The contexts kept for `paper_ids` that still hold (read by this reader, of the PDF the paper's passages
    /// were read from), with that PDF's hash; papers with none are left out.
    fn kept_contexts_with_hash(
        &self,
        paper_ids: &[&str],
        entries: Entries,
    ) -> Result<HashMap<String, (String, PaperContexts)>, LibraryError> {
        let column = match entries {
            Entries::All => "c.content",
            Entries::InLibrary => "c.linked",
        };
        self.read(|connection| {
            let mut kept = HashMap::new();
            for ids in paper_ids.chunks(500) {
                let sql = format!(
                    "SELECT c.paper_id, c.pdf_hash, {column}, p.pdf_size, p.pdf_modified FROM citation_contexts c
                     JOIN paper_passages p ON p.paper_id = c.paper_id
                     WHERE c.paper_id IN ({}) AND c.reader = ?{} AND c.version = ?{} AND p.reader = c.reader
                         AND p.pdf_hash = c.pdf_hash",
                    placeholders(ids.len()),
                    ids.len() + 1,
                    ids.len() + 2
                );
                let mut statement = connection.prepare(&sql)?;
                let mut values: Vec<rusqlite::types::Value> =
                    ids.iter().map(|id| rusqlite::types::Value::Text((*id).to_owned())).collect();
                values.push(rusqlite::types::Value::Integer(i64::from(READER)));
                values.push(rusqlite::types::Value::Integer(i64::from(CONTEXTS)));
                let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?;
                for row in rows {
                    let (id, hash, content, size, modified) = row?;
                    if !same_file(self, &id, size, modified) {
                        continue;
                    }
                    // contents that no longer read as contexts are read again
                    if let Ok(contexts) = serde_json::from_str(&content) {
                        kept.insert(id, (hash, contexts));
                    }
                }
            }
            Ok(kept)
        })
    }

    /// The contexts kept for `paper_ids` that still hold, by paper; papers with none are left out.
    pub fn kept_contexts(
        &self,
        paper_ids: &[&str],
        entries: Entries,
    ) -> Result<HashMap<String, PaperContexts>, LibraryError> {
        Ok(self
            .kept_contexts_with_hash(paper_ids, entries)?
            .into_iter()
            .map(|(id, (_, contexts))| (id, contexts))
            .collect())
    }

    /// The papers whose kept contexts (that still hold) cite `cited`, with in how many sentences: the most first,
    /// then by id.
    pub fn citing_papers(&self, cited: &str) -> Result<Vec<(String, usize)>, LibraryError> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT l.citing, l.sentences, p.pdf_size, p.pdf_modified FROM citation_links l
                 JOIN citation_contexts c ON c.paper_id = l.citing
                 JOIN paper_passages p ON p.paper_id = c.paper_id
                 WHERE l.cited = ?1 AND c.reader = ?2 AND c.version = ?3 AND p.reader = c.reader
                     AND p.pdf_hash = c.pdf_hash
                 ORDER BY l.sentences DESC, l.citing",
            )?;
            let rows = statement.query_map(params![cited, READER, CONTEXTS], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            let mut citing = Vec::new();
            for row in rows {
                let (id, sentences, size, modified) = row?;
                if same_file(self, &id, size, modified) {
                    citing.push((id, sentences));
                }
            }
            Ok(citing)
        })
    }

    /// Keep each paper's contexts, read from the PDF of that hash, and who it cites, in one transaction.
    fn keep_contexts(&self, kept: &[(&str, &str, &PaperContexts)]) -> Result<(), LibraryError> {
        if kept.is_empty() {
            return Ok(());
        }
        self.write(|transaction| {
            let mut contexts = transaction.prepare_cached(
                "INSERT INTO citation_contexts (paper_id, reader, version, pdf_hash, content, linked, read_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (paper_id) DO UPDATE SET
                     reader = excluded.reader, version = excluded.version, pdf_hash = excluded.pdf_hash,
                     content = excluded.content, linked = excluded.linked, read_at = excluded.read_at",
            )?;
            let mut forget = transaction.prepare_cached("DELETE FROM citation_links WHERE citing = ?1")?;
            let mut link = transaction
                .prepare_cached("INSERT INTO citation_links (citing, cited, sentences) VALUES (?1, ?2, ?3)")?;
            let mut passages_of = transaction
                .prepare_cached("SELECT pdf_hash FROM paper_passages WHERE paper_id = ?1 AND reader = ?2")?;
            for (paper_id, hash, read) in kept {
                // read from another PDF than the passages were (the file replaced in between): not kept, read again
                let of_passages: Option<String> =
                    passages_of.query_row(params![paper_id, READER], |row| row.get(0)).optional()?;
                if of_passages.as_deref() != Some(*hash) {
                    continue;
                }
                let content = serde_json::to_string(read)?;
                let linked = serde_json::to_string(&in_library(read))?;
                contexts.execute(params![paper_id, READER, CONTEXTS, hash, content, linked, now()])?;
                forget.execute([paper_id])?;
                for (cited, sentences) in read.in_library() {
                    link.execute(params![paper_id, cited, i64::try_from(sentences.len()).unwrap_or(i64::MAX)])?;
                }
            }
            Ok(())
        })
    }

    /// The citing sentences of each of `paper_ids` (each once), in the order asked: kept ones linked again against `linker`
    /// (kept again when that changed them), the others read now on every core and kept. `stop` is asked between
    /// rounds of [`AT_ONCE`] papers; `done` is told how many have been read so far. When stopped, the papers not
    /// reached are left out of the answer.
    pub fn citation_contexts(
        &self,
        paper_ids: &[&str],
        linker: &Linker,
        stop: &dyn Fn() -> bool,
        done: &dyn Fn(usize),
    ) -> Result<Vec<(String, ContextsReading)>, LibraryError> {
        let mut answer = Vec::with_capacity(paper_ids.len());
        for round in paper_ids.chunks(AT_ONCE) {
            if stop() {
                break;
            }
            // the passages first: a PDF replaced since is read again, so that contexts of the old one no longer hold
            let mut readings: HashMap<&str, PaperReading> =
                round.iter().copied().zip(self.paper_texts(round)?).collect();
            let mut kept = self.kept_contexts_with_hash(round, Entries::All)?;
            let mut relinked: Vec<String> = Vec::new();
            for (id, (_, contexts)) in &mut kept {
                if contexts.relink(linker, id) {
                    relinked.push(id.clone());
                }
            }
            relinked.sort_unstable();
            // each paper once, however many times it is asked for: every place it is asked at gets its reading
            let mut to_read: Vec<&str> = round.iter().copied().filter(|id| !kept.contains_key(*id)).collect();
            let mut seen = std::collections::HashSet::new();
            to_read.retain(|id| seen.insert(*id));
            let readings: Vec<PaperReading> =
                to_read.iter().map(|id| readings.remove(id).unwrap_or(PaperReading::NotInLibrary)).collect();
            let read = self.read_round(&to_read, &readings, linker);
            let mut to_keep: Vec<(&str, &str, &PaperContexts)> = to_read
                .iter()
                .zip(&read)
                .filter_map(|(id, (hash, reading))| match (hash, reading) {
                    (Some(hash), ContextsReading::Read(contexts)) => Some((*id, hash.as_str(), contexts)),
                    _ => None,
                })
                .collect();
            to_keep.extend(
                relinked
                    .iter()
                    .filter_map(|id| kept.get(id).map(|(hash, contexts)| (id.as_str(), hash.as_str(), contexts))),
            );
            self.keep_contexts(&to_keep)?;
            let read: HashMap<&str, ContextsReading> =
                to_read.into_iter().zip(read.into_iter().map(|(_, reading)| reading)).collect();
            for id in round {
                // looked up, not taken out: a paper asked for twice is answered at both places
                let reading = match kept.get(*id) {
                    Some((_, contexts)) => ContextsReading::Read(contexts.clone()),
                    None => read.get(id).cloned().unwrap_or(ContextsReading::NotInLibrary),
                };
                answer.push(((*id).to_owned(), reading));
            }
            done(answer.len());
        }
        Ok(answer)
    }

    /// Read the contexts of `to_read`, whose passages are `readings`, on every core, in the order asked whatever
    /// the number of threads; each read one with the hash of the PDF it was read from. It touches no database:
    /// the passages were had before.
    fn read_round(
        &self,
        to_read: &[&str],
        readings: &[PaperReading],
        linker: &Linker,
    ) -> Vec<(Option<String>, ContextsReading)> {
        to_read
            .par_iter()
            .zip(readings.par_iter())
            .map(|(id, reading)| match reading {
                PaperReading::Read(passages) => match read_one(self, id, passages, linker) {
                    Ok((hash, contexts)) => (Some(hash), ContextsReading::Read(contexts)),
                    Err(failed) => (None, failed),
                },
                PaperReading::NoPdf { .. } => (None, ContextsReading::NoPdf),
                PaperReading::Unreadable { why, .. } => (None, ContextsReading::Unreadable(why.clone())),
                PaperReading::NotInLibrary => (None, ContextsReading::NotInLibrary),
            })
            .collect()
    }

    /// Of `paper_ids`, those whose citing sentences are kept and still hold.
    pub fn contexts_kept_among(&self, paper_ids: &[&str]) -> Result<HashSet<String>, LibraryError> {
        Ok(self.kept_contexts_with_hash(paper_ids, Entries::InLibrary)?.into_keys().collect())
    }

    /// Which of `paper_ids` have a PDF this reader tried and could not read (and that file is still the one kept):
    /// their citing sentences cannot be read, so they are neither read nor waiting to be.
    pub fn unreadable_among(&self, paper_ids: &[&str]) -> Result<HashSet<String>, LibraryError> {
        self.read(|connection| {
            let mut unreadable = HashSet::new();
            for ids in paper_ids.chunks(500) {
                let sql = format!(
                    "SELECT paper_id, pdf_size, pdf_modified FROM paper_passages
                     WHERE paper_id IN ({}) AND reader = ?{} AND outcome = 'unreadable'",
                    placeholders(ids.len()),
                    ids.len() + 1
                );
                let mut statement = connection.prepare(&sql)?;
                let mut values: Vec<rusqlite::types::Value> =
                    ids.iter().map(|id| rusqlite::types::Value::Text((*id).to_owned())).collect();
                values.push(rusqlite::types::Value::Integer(i64::from(READER)));
                let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
                })?;
                for row in rows {
                    let (id, size, modified) = row?;
                    if same_file(self, &id, size, modified) {
                        unreadable.insert(id);
                    }
                }
            }
            Ok(unreadable)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::library::Known;
    use crate::library::papers::passages::fixtures::pdf_with;
    use crate::library::papers::tests::paper;

    /// A library of three papers: "seed" (with a PDF that cites the two others), "arith" and "ties" (without).
    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        let mut papers = vec![paper("seed"), paper("arith"), paper("ties")];
        papers[1].title = "Editing models with task arithmetic".to_owned();
        papers[1].published = "2023-05-01T00:00:00Z".to_owned();
        papers[2].title = "Resolving interference when merging models".to_owned();
        papers[2].published = "2023-06-01T00:00:00Z".to_owned();
        library.add_papers(&papers, None, Known::Keep).unwrap();
        library.keep_pdf("seed", &seed_pdf()).unwrap();
        (folder, library)
    }

    fn seed_pdf() -> Vec<u8> {
        pdf_with(&[
            "1 Introduction",
            "",
            "Merging is cheap. Task arithmetic adds the updates of each task [1]. We study when it fails.",
            "",
            "2 Experiments",
            "",
            "We compare against the merging of [2] as a baseline. It is weaker than [3].",
            "",
            "References",
            "",
            "[1] A. Lovelace. Editing models with task arithmetic. In ICLR, 2023.",
            "",
            "[2] A. Turing. Resolving interference when merging models. In NeurIPS, 2023.",
            "",
            "[3] G. Hopper. A work that is not in the library. In ICML, 2022.",
        ])
    }

    fn read(library: &Library, ids: &[&str]) -> Vec<(String, ContextsReading)> {
        let papers = library.link_papers().unwrap();
        let linker = Linker::new(&papers);
        library.citation_contexts(ids, &linker, &|| false, &|_| {}).unwrap()
    }

    #[test]
    fn a_papers_citing_sentences_are_read_from_its_pdf_kept_and_indexed_by_the_paper_they_cite() {
        let (_folder, library) = library();
        let read = read(&library, &["seed", "arith", "nowhere"]);
        let ContextsReading::Read(contexts) = &read[0].1 else { panic!("{:?}", read[0]) };
        let linked: Vec<String> = contexts.in_library().into_iter().map(|(paper, _)| paper).collect();
        assert_eq!(linked, ["arith", "ties"], "{contexts:?}");
        assert_eq!(read[1].1, ContextsReading::NoPdf, "a paper with no PDF is its own case");
        assert_eq!(read[2].1, ContextsReading::NotInLibrary);
        assert_eq!(library.citing_papers("arith").unwrap(), [("seed".to_owned(), 1)]);
        assert_eq!(library.citing_papers("ties").unwrap(), [("seed".to_owned(), 1)]);
        assert!(library.citing_papers("seed").unwrap().is_empty());
        let kept = library.kept_contexts(&["seed"], Entries::InLibrary).unwrap();
        assert_eq!(kept["seed"].entries.len(), 2, "only the entries in the library");
        assert_eq!(library.kept_contexts(&["seed"], Entries::All).unwrap()["seed"].entries.len(), 3);
        assert_eq!(library.contexts_kept_among(&["seed", "arith"]).unwrap(), HashSet::from(["seed".to_owned()]));
    }

    #[test]
    fn a_paper_asked_for_twice_is_answered_at_both_places_whether_kept_or_read_now() {
        let (_folder, library) = library();
        // read now, both times in one round
        let first = read(&library, &["seed", "arith", "seed"]);
        assert_eq!(first.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["seed", "arith", "seed"]);
        assert!(matches!(first[0].1, ContextsReading::Read(_)) && first[2].1 == first[0].1, "{first:?}");
        // kept by now
        let again = read(&library, &["seed", "seed", "arith", "seed"]);
        assert!(matches!(again[0].1, ContextsReading::Read(_)), "{again:?}");
        assert_eq!((&again[1].1, &again[3].1), (&again[0].1, &again[0].1));
        assert_eq!(again[2].1, ContextsReading::NoPdf);
    }

    #[test]
    fn kept_contexts_are_not_read_again_until_the_pdf_changes() {
        let (_folder, library) = library();
        read(&library, &["seed"]);
        let stamp = |library: &Library| -> String {
            library
                .connection()
                .query_row("SELECT read_at || pdf_hash FROM citation_contexts WHERE paper_id = 'seed'", [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        let first = stamp(&library);
        library.connection().execute("UPDATE citation_contexts SET read_at = 'kept'", []).unwrap();
        read(&library, &["seed"]);
        assert!(stamp(&library).starts_with("kept"), "kept: not read again");
        library.keep_pdf("seed", &pdf_with(&["1 Introduction", "", "Nothing is cited here at all."])).unwrap();
        let again = read(&library, &["seed"]);
        let ContextsReading::Read(contexts) = &again[0].1 else { panic!() };
        assert!(contexts.entries.is_empty());
        assert_ne!(stamp(&library), first, "another PDF is read again");
        assert!(library.citing_papers("arith").unwrap().is_empty(), "who it cites is replaced with it");
    }

    #[test]
    fn contexts_of_a_pdf_replaced_since_do_not_hold_even_before_its_text_is_read_again() {
        let (_folder, library) = library();
        read(&library, &["seed"]);
        assert_eq!(library.citing_papers("arith").unwrap().len(), 1);
        // replaced, and nothing has read the new file yet: the passages' row is still the old file's
        library.keep_pdf("seed", &pdf_with(&["1 Introduction", "", "Nothing is cited here at all, now."])).unwrap();
        assert!(library.kept_contexts(&["seed"], Entries::All).unwrap().is_empty());
        assert!(library.contexts_kept_among(&["seed"]).unwrap().is_empty());
        assert!(library.citing_papers("arith").unwrap().is_empty(), "who it cited is of the old file");
        // the PDF gone altogether: nothing holds either
        std::fs::remove_file(library.pdf_file("seed")).unwrap();
        assert!(library.kept_contexts(&["seed"], Entries::InLibrary).unwrap().is_empty());
    }

    #[test]
    fn a_pdf_that_could_not_be_read_is_counted_unreadable_until_it_is_replaced() {
        let (_folder, library) = library();
        library.keep_pdf("arith", b"not a pdf at all").unwrap();
        assert!(library.unreadable_among(&["seed", "arith", "ties"]).unwrap().is_empty(), "not tried yet");
        library.paper_texts(&["seed", "arith", "ties"]).unwrap();
        assert_eq!(library.unreadable_among(&["seed", "arith", "ties"]).unwrap(), HashSet::from(["arith".to_owned()]));
        library.keep_pdf("arith", &seed_pdf()).unwrap();
        assert!(library.unreadable_among(&["arith"]).unwrap().is_empty(), "another file: not known unreadable");
    }

    #[test]
    fn a_paper_added_to_the_library_later_is_linked_when_the_contexts_are_asked_for_again() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(&[paper("seed")], None, Known::Keep).unwrap();
        library.keep_pdf("seed", &seed_pdf()).unwrap();
        read(&library, &["seed"]);
        assert!(library.citing_papers("arith").unwrap().is_empty());
        let mut arith = paper("arith");
        arith.title = "Editing models with task arithmetic".to_owned();
        library.add_papers(&[arith], None, Known::Keep).unwrap();
        read(&library, &["seed"]);
        assert_eq!(library.citing_papers("arith").unwrap(), [("seed".to_owned(), 1)]);
    }

    #[test]
    fn many_papers_read_on_any_number_of_threads_give_the_same_contexts_in_the_order_asked() {
        let (_folder, library) = library();
        let ids: Vec<String> = (0..12).map(|n| format!("p{n}")).collect();
        library.add_papers(&ids.iter().map(|id| paper(id)).collect::<Vec<_>>(), None, Known::Keep).unwrap();
        for (n, id) in ids.iter().enumerate() {
            let cites = if n % 2 == 0 { "[1]" } else { "[2]" };
            let sentence = format!("Paper {n} builds on earlier work {cites}. It adds little.");
            library
                .keep_pdf(
                    id,
                    &pdf_with(&[
                        "1 Introduction",
                        "",
                        &sentence,
                        "",
                        "References",
                        "",
                        "[1] A. Lovelace. Editing models with task arithmetic. In ICLR, 2023.",
                        "",
                        "[2] A. Turing. Resolving interference when merging models. In NeurIPS, 2023.",
                        "",
                        "[3] G. Hopper. A work that is not in the library. In ICML, 2022.",
                    ]),
                )
                .unwrap();
        }
        let mut wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
        wanted.push("arith");
        wanted.reverse();
        let papers = library.link_papers().unwrap();
        let linker = Linker::new(&papers);
        let readings = library.paper_texts(&wanted).unwrap();
        // only the parallel part runs in each pool: the passages were had outside it
        let on = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            pool.install(|| library.read_round(&wanted, &readings, &linker))
        };
        let one = on(1);
        assert_eq!(one.len(), wanted.len());
        assert_eq!(one, on(4));
        assert_eq!(one, on(7));
        let read = library.citation_contexts(&wanted, &linker, &|| false, &|_| {}).unwrap();
        assert_eq!(read.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), wanted, "in the order asked");
        assert_eq!(library.citing_papers("arith").unwrap().len(), 6);
    }

    #[test]
    fn reading_stops_between_rounds_and_says_how_far_it_got() {
        let (_folder, library) = library();
        let told = Mutex::new(Vec::new());
        let papers = library.link_papers().unwrap();
        let linker = Linker::new(&papers);
        let none =
            library.citation_contexts(&["seed"], &linker, &|| true, &|done| told.lock().unwrap().push(done)).unwrap();
        assert!(none.is_empty() && told.lock().unwrap().is_empty());
        library
            .citation_contexts(&["seed", "arith"], &linker, &|| false, &|done| told.lock().unwrap().push(done))
            .unwrap();
        assert_eq!(*told.lock().unwrap(), [2]);
    }

    #[test]
    fn the_library_papers_are_linked_by_their_arxiv_id_and_year() {
        let (_folder, library) = library();
        let mut arxiv = paper("2601.00007");
        arxiv.published = "2026-01-03T00:00:00Z".to_owned();
        library.add_papers(&[arxiv], None, Known::Keep).unwrap();
        let papers = library.link_papers().unwrap();
        let found = papers.iter().find(|paper| paper.id == "2601.00007").unwrap();
        assert_eq!((found.arxiv.as_deref(), found.year), (Some("2601.00007"), Some(2026)));
        assert_eq!(papers.iter().find(|paper| paper.id == "arith").unwrap().year, Some(2023));
    }
}
