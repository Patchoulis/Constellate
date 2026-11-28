//! The papers of a community closest in meaning to each statement of its description: found with the run's own
//! embedding model on this computer, for every description, plain or in sections. No language model is asked,
//! nothing is sent anywhere and nothing is paid for.
//!
//! ```text
//! description ──statements──> a section's text, or each sentence of a plain description
//!             ──embedded as queries (the model's query form)──> cosine with the members' kept embeddings
//!             ──the top TOP above FLOOR, ties by paper id──> for each, its sentence closest to the statement
//!             ──located in the paper's title or abstract (evidence::Evidence::locate)──> shown
//! ```
//!
//! # Why the cosines are comparable
//!
//! The members' vectors are the library's: `Qwen/Qwen3-Embedding-0.6B` of each paper's title and abstract, as the
//! run embedded them, unit length and *not* centred (a run's "centre the embeddings" is applied when its graph is
//! built, between papers; the library keeps the model's vectors). A statement is embedded by the same model in the
//! form the model was trained to compare with documents: `Instruct: <task>\nQuery:<statement>` ([`query`]), also
//! unit length and not centred. Centring a query by the mean of the papers would move it by a vector it never had.
//! A paper's candidate sentences are embedded as documents (no instruction), as the papers were. The figure shown
//! is that cosine, which says how close in meaning two texts are, not that the paper says the statement.
//!
//! # What is kept
//!
//! Each statement's papers and sentences, under [`Feature::Closest`] with the key of the embedding model, the
//! statement's words and the community's members ([`key_of`]): opening the community again shows them at once; a
//! community that gains or loses a paper, another model, or another wording is found again. The sentence is kept
//! as words and located in the paper again whenever it is shown, as every feature's quotes are.
//!
//! # Deterministic
//!
//! Cosines are rounded to [`DIGITS`] decimals before they are ranked, and ties go to the lower paper id, so the
//! same vectors give the same papers in the same order; the rounding keeps the few last bits a graphics card and
//! a processor disagree on from reordering papers that are equally close.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Stored, one_block};
use crate::evidence::{Evidence, PaperText, SHORTEST_QUOTE};
use crate::jobs::{Progress, Tracker};
use crate::library::{AnswerKey, Feature, Library, LibraryError, Paper, Slot, Subject};
use crate::pipeline::steps::embedding::Embedding;
use crate::text::pdf::passages::{letters_in, sentence_spans};
use crate::usage::Usd;

/// The version of how statements are found papers for: raised when it changes, so kept results are found again.
pub const VERSION: u32 = 1;
/// What the job finding them is called in `/api/status`.
pub const JOB: &str = "finding the closest papers";
/// Papers kept per statement, closest first.
pub const TOP: usize = 5;
/// The least cosine similarity a paper is shown at. Measured on a run of 12,487 papers (Qwen3-Embedding-0.6B; the
/// 27 statements of the first 6 communities' plain descriptions, as queries, against each community's own members,
/// 600 papers and more): a statement's closest member scored 0.58 to 0.81, its fifth closest 0.53 to 0.75, its median
/// member 0.35 to 0.55 and its least close 0.09 to 0.34. 0.50 is above the median member of nearly every
/// statement (a paper shown is closer than at least half the community) and below every statement's closest
/// (each measured statement kept at least one paper).
pub const FLOOR: f32 = 0.50;
/// Decimals a cosine is rounded to before it is ranked (and as it is shown).
pub const DIGITS: i32 = 3;
/// Sentences of one paper compared with a statement to find its supporting one: the first of its abstract.
pub const SENTENCES: usize = 12;
/// What the model is told the queries are for (Qwen3-Embedding's instruction form).
pub const TASK: &str = "Given a statement about the work of a research community, retrieve the abstracts of research papers that support it";

/// A statement as the model compares it with the papers: in the query form Qwen3-Embedding was trained with.
pub fn query(statement: &str) -> String {
    format!("Instruct: {TASK}\nQuery:{statement}")
}

/// One statement of a description: its words, and the section it is from (none for a plain description).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub text: String,
    pub section: Option<String>,
}

/// The statements of a community's description: each section's text, or each sentence of a plain description.
pub fn statements_of(community: &Stored) -> Vec<Statement> {
    if let Some(sections) = &community.sections {
        return sections
            .iter()
            .filter_map(|section| {
                let text = one_block(section.get("text").and_then(Value::as_str)?);
                let heading = section.get("heading").and_then(Value::as_str).map(str::to_owned);
                (!text.is_empty()).then_some(Statement { text, section: heading })
            })
            .collect();
    }
    let description = one_block(community.description.as_deref().unwrap_or(""));
    sentences(&description).into_iter().map(|text| Statement { text, section: None }).collect()
}

/// The sentences of `text` ([`sentence_spans`]: not cut after "e.g.", "et al.", an initial or a decimal), trimmed,
/// empty ones left out.
pub fn sentences(text: &str) -> Vec<String> {
    sentence_spans(text)
        .into_iter()
        .map(|(from, to)| text.chars().skip(from).take(to - from).collect::<String>().trim().to_owned())
        .filter(|sentence| !sentence.is_empty())
        .collect()
}

/// The cosine similarity of two vectors (0 when either is all zeros).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut aa, mut bb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        aa += f64::from(*x) * f64::from(*x);
        bb += f64::from(*y) * f64::from(*y);
    }
    if aa == 0.0 || bb == 0.0 {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let similarity = (dot / (aa.sqrt() * bb.sqrt())) as f32;
    similarity
}

/// `value` rounded to [`DIGITS`] decimals.
pub fn rounded(value: f32) -> f32 {
    let scale = 10_f32.powi(DIGITS);
    (value * scale).round() / scale
}

/// The places among `papers` (id and vector) of the `most` closest to `statement`, with their cosine (rounded),
/// closest first, ties by paper id; none below `floor`.
pub fn ranked(statement: &[f32], papers: &[(&str, &[f32])], most: usize, floor: f32) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = papers
        .iter()
        .enumerate()
        .map(|(place, (_, vector))| (place, rounded(cosine(statement, vector))))
        .filter(|(_, similarity)| *similarity >= floor)
        .collect();
    scored.sort_by(|(a, first), (b, second)| second.total_cmp(first).then_with(|| papers[*a].0.cmp(papers[*b].0)));
    scored.truncate(most);
    scored
}

/// The place of the vector among `candidates` closest to `statement`; the first of equals. `None` for none.
pub fn closest_of(statement: &[f32], candidates: &[Vec<f32>]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (place, vector) in candidates.iter().enumerate() {
        let similarity = rounded(cosine(statement, vector));
        if best.is_none_or(|(_, most)| similarity > most) {
            best = Some((place, similarity));
        }
    }
    best.map(|(place, _)| place)
}

/// The sentences of a paper that can support a statement, in its own words: the first [`SENTENCES`] of its
/// abstract long enough to be a quote ([`SHORTEST_QUOTE`]), then its title when it is.
pub fn candidates(paper: &Paper) -> Vec<String> {
    let mut found: Vec<String> = sentences(&paper.r#abstract)
        .into_iter()
        .filter(|sentence| letters_in(sentence) >= SHORTEST_QUOTE)
        .take(SENTENCES)
        .collect();
    let title = paper.title.trim();
    if letters_in(title) >= SHORTEST_QUOTE {
        found.push(title.to_owned());
    }
    found
}

/// A paper found for a statement, as kept: the paper, its cosine, and its sentence closest to the statement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptPaper {
    pub paper: String,
    pub cosine: f32,
    pub sentence: String,
}

/// What is kept for one statement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub statement: String,
    pub papers: Vec<KeptPaper>,
}

/// The key a statement's papers are kept under: the embedding model, the statement's words, the members.
pub fn key_of(model: &str, statement: &str, members: &[&str]) -> AnswerKey {
    AnswerKey::of_feature(Feature::Closest, VERSION, members, &format!("{model}\n{statement}"))
}

/// A paper shown under a statement: `ClosestPaper` in the contracts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShownPaper {
    pub id: String,
    pub title: String,
    /// Its cosine similarity with the statement, rounded to [`DIGITS`] decimals.
    pub cosine: f32,
    /// Its sentence closest to the statement, located in its title or abstract.
    pub evidence: Evidence,
}

/// A statement with its closest papers: `ClosestStatement` in the contracts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Shown {
    pub statement: String,
    pub section: Option<String>,
    /// Closest first; empty when no member reaches [`FLOOR`].
    pub papers: Vec<ShownPaper>,
}

/// What is kept for `statement`, shown: each sentence located again in its paper (a paper whose words changed so
/// that its sentence is no longer there is left out).
pub fn shown(statement: &Statement, kept: &Kept, records: &HashMap<String, Paper>) -> Shown {
    let papers = kept
        .papers
        .iter()
        .filter_map(|found| {
            let paper = records.get(&found.paper)?;
            let text = PaperText::Unread { title: paper.title.clone(), abstract_text: paper.r#abstract.clone() };
            let evidence = Evidence::locate(&paper.id, &text, &found.sentence).ok()?;
            Some(ShownPaper { id: paper.id.clone(), title: paper.title.clone(), cosine: found.cosine, evidence })
        })
        .collect();
    Shown { statement: statement.text.clone(), section: statement.section.clone(), papers }
}

/// What finding a community's closest papers needs: its statements, its members (records and kept vectors, in
/// the same order) and the model.
pub struct Asked<'a> {
    pub model: &'a str,
    pub statements: &'a [Statement],
    pub members: &'a [Paper],
    /// One row per member: the library's vector of it by `model`.
    pub vectors: &'a [Vec<f32>],
}

/// Texts embedded at once (as the run embeds papers).
const BATCH: usize = 16;

fn encode(embedding: &dyn Embedding, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let mut vectors = Vec::with_capacity(texts.len());
    for batch in texts.chunks(BATCH) {
        let batch: Vec<&str> = batch.iter().map(String::as_str).collect();
        vectors.extend(embedding.encode(model, &batch).map_err(|error| error.to_string())?);
    }
    Ok(vectors)
}

/// Find each statement's closest members and their supporting sentences with `embedding`, told as each statement
/// is done; `kept` is handed each one's result as it is found. Stops (with what is done kept) when asked to.
pub fn find(
    embedding: &dyn Embedding,
    asked: &Asked<'_>,
    tracker: &Tracker<'_>,
    kept: &mut dyn FnMut(&Statement, Kept) -> Result<(), LibraryError>,
) -> Result<(), String> {
    let queries: Vec<String> = asked.statements.iter().map(|statement| query(&statement.text)).collect();
    let statement_vectors = encode(embedding, asked.model, &queries)?;
    let members: Vec<(&str, &[f32])> =
        asked.members.iter().zip(asked.vectors).map(|(paper, vector)| (paper.id.as_str(), vector.as_slice())).collect();
    let of = u64::try_from(asked.statements.len()).unwrap_or(u64::MAX);
    for (done, (statement, vector)) in asked.statements.iter().zip(&statement_vectors).enumerate() {
        if tracker.cancelled() {
            return Err("stopped".to_owned());
        }
        let mut papers = Vec::new();
        for (place, similarity) in ranked(vector, &members, TOP, FLOOR) {
            let paper = &asked.members[place];
            let candidates = candidates(paper);
            let sentence_vectors = encode(embedding, asked.model, &candidates)?;
            let Some(best) = closest_of(vector, &sentence_vectors) else { continue };
            papers.push(KeptPaper { paper: paper.id.clone(), cosine: similarity, sentence: candidates[best].clone() });
        }
        kept(statement, Kept { statement: statement.text.clone(), papers }).map_err(|error| error.to_string())?;
        let done = u64::try_from(done + 1).unwrap_or(u64::MAX);
        tracker.progress(Progress { done, of, usd: Usd::ZERO });
    }
    Ok(())
}

/// Keep one statement's result for `subject`.
pub fn keep(
    library: &Library,
    subject: &Subject,
    model: &str,
    members: &[&str],
    kept: &Kept,
) -> Result<(), LibraryError> {
    let slot =
        Slot { feature: Feature::Closest, key: key_of(model, &kept.statement, members), subject: subject.clone() };
    library.keep(&slot, "", VERSION, kept)
}

/// What is kept for each statement, `None` where nothing is.
pub fn kept_of(
    library: &Library,
    model: &str,
    members: &[&str],
    statements: &[Statement],
) -> Result<Vec<Option<Kept>>, LibraryError> {
    statements
        .iter()
        .map(|statement| {
            Ok(library
                .kept::<Kept>(Feature::Closest, &key_of(model, &statement.text, members))?
                .map(|kept| kept.answer))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::jobs::Cancel;
    use crate::library::Known;
    use crate::library::papers::tests::paper;
    use crate::pipeline::RunError;
    use crate::pipeline::steps::{Board, StepOutcome};

    #[test]
    fn a_plain_description_is_split_into_sentences_but_not_after_abbreviations_or_decimals() {
        let text = "Papers study graph neural networks (GNNs). Methods include e.g. message passing, as in Kipf et \
                    al. (2017), with gains of 3.5 points. They differ from vision (cf. CNNs). Is it new? Yes.";
        assert_eq!(
            sentences(text),
            [
                "Papers study graph neural networks (GNNs).",
                "Methods include e.g. message passing, as in Kipf et al. (2017), with gains of 3.5 points.",
                "They differ from vision (cf. CNNs).",
                "Is it new?",
                "Yes."
            ]
        );
        assert!(sentences("  ").is_empty());
        assert_eq!(sentences("No full stop at the end"), ["No full stop at the end"]);
    }

    #[test]
    fn the_statements_are_a_sectioned_descriptions_sections_or_a_plain_ones_sentences() {
        let plain: Stored = serde_json::from_value(json!({"tree_path": "c0", "name": "A", "description":
            "First claim here. Second claim\nthere."}))
        .unwrap();
        assert_eq!(
            statements_of(&plain),
            [
                Statement { text: "First claim here.".into(), section: None },
                Statement { text: "Second claim there.".into(), section: None }
            ]
        );
        let sectioned: Stored = serde_json::from_value(json!({"tree_path": "c0", "name": "A", "description": "x",
            "sections": [{"heading": "Problem", "text": "One. Two.", "cites": []}, {"heading": "Shared methods", "text": " "}]}))
        .unwrap();
        assert_eq!(
            statements_of(&sectioned),
            [Statement { text: "One. Two.".into(), section: Some("Problem".into()) }]
        );
        let none: Stored = serde_json::from_value(json!({"tree_path": "c0"})).unwrap();
        assert!(statements_of(&none).is_empty());
    }

    #[test]
    fn a_statement_is_embedded_in_the_models_query_form() {
        assert_eq!(query("Papers on X."), format!("Instruct: {TASK}\nQuery:Papers on X."));
        assert!(!TASK.contains('\n'));
    }

    #[test]
    fn papers_are_ranked_by_cosine_above_the_floor_ties_by_id_the_same_every_time() {
        let statement = [1.0, 0.0];
        let at = |angle: f32| vec![angle.cos(), angle.sin()];
        let vectors = [at(0.3), at(0.1), at(0.1), at(1.5), at(0.0), vec![0.0, 0.0]];
        let ids = ["p5", "p9", "p2", "p3", "p7", "p0"];
        let papers: Vec<(&str, &[f32])> = ids.iter().copied().zip(vectors.iter().map(Vec::as_slice)).collect();
        let found = ranked(&statement, &papers, 3, FLOOR);
        // p7 (cosine 1), then the two at 0.995 by id: p2 before p9
        assert_eq!(found.iter().map(|(place, _)| ids[*place]).collect::<Vec<_>>(), ["p7", "p2", "p9"]);
        assert_eq!(found[0].1, 1.0);
        assert_eq!(found[1].1, rounded(0.1_f32.cos()));
        // the most asked for, and never below the floor: at 1.5 radians the cosine is 0.07; a zero vector is 0
        let all = ranked(&statement, &papers, 10, FLOOR);
        assert_eq!(all.len(), 4);
        assert!(all.iter().all(|(_, similarity)| *similarity >= FLOOR));
        // nothing close: none
        assert!(ranked(&[0.0, 1.0], &papers[..3], 5, 0.99).is_empty());
        // the order the papers come in does not change what is found
        let mut reversed = papers.clone();
        reversed.reverse();
        let again: Vec<&str> =
            ranked(&statement, &reversed, 3, FLOOR).iter().map(|(place, _)| reversed[*place].0).collect();
        assert_eq!(again, ["p7", "p2", "p9"]);
        // a difference in the last bits (another device) does not reorder papers equally close
        let nudged = [vec![0.995_004_2, 0.099_833_4], vec![0.995_004_1, 0.099_833_5]];
        let close: Vec<(&str, &[f32])> = vec![("b", &nudged[0]), ("a", &nudged[1])];
        assert_eq!(
            ranked(&statement, &close, 2, 0.0).iter().map(|(place, _)| close[*place].0).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn the_supporting_sentence_is_the_closest_and_the_first_of_equals() {
        let candidates = vec![vec![0.0, 1.0], vec![1.0, 0.1], vec![1.0, 0.1], vec![0.5, 0.5]];
        assert_eq!(closest_of(&[1.0, 0.0], &candidates), Some(1));
        assert_eq!(closest_of(&[1.0, 0.0], &[]), None);
    }

    fn records() -> Vec<Paper> {
        let made = |id: &str, title: &str, text: &str| {
            let mut made = paper(id);
            made.title = title.into();
            made.r#abstract = text.into();
            made
        };
        vec![
            made(
                "p1",
                "Routing tokens to experts at scale",
                "We train sparse mixtures of experts. A balanced router \
                 sends each token to two experts. Results improve.",
            ),
            made("p2", "Short", "Graph networks pass messages between nodes. Node features are updated by neighbours."),
            made(
                "p3",
                "Experts",
                "Mixtures of experts route tokens with a learned router. Load balancing keeps experts busy.",
            ),
        ]
    }

    #[test]
    fn a_papers_candidates_are_its_own_sentences_long_enough_to_quote() {
        let papers = records();
        assert_eq!(
            candidates(&papers[0]),
            [
                "We train sparse mixtures of experts.",
                "A balanced router sends each token to two experts.",
                "Routing tokens to experts at scale"
            ]
        );
        // "Results improve." is too short to show anything, and so is the title "Short"
        assert_eq!(candidates(&papers[1]).len(), 2);
    }

    /// An embedding of made-up vectors: one dimension per word of a small vocabulary, counted (no model needed).
    pub(crate) struct Words {
        pub asked: Mutex<Vec<String>>,
    }

    pub(crate) const VOCABULARY: [&str; 8] =
        ["expert", "router", "token", "graph", "node", "message", "balanc", "train"];

    impl Embedding for Words {
        fn prepare(&self, _: &str, _: &std::path::Path, _: &Tracker<'_>, _: &Board) -> Result<StepOutcome, RunError> {
            Ok(StepOutcome::Kept("made up".into()))
        }

        fn encode(&self, _: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, RunError> {
            self.asked.lock().unwrap().extend(texts.iter().map(|text| (*text).to_owned()));
            Ok(texts
                .iter()
                .map(|text| {
                    // the query's instruction is not counted: only what it asks about
                    let text = text.rsplit("Query:").next().unwrap_or(text).to_lowercase();
                    #[allow(clippy::cast_precision_loss)]
                    let vector: Vec<f32> = VOCABULARY.iter().map(|word| text.matches(word).count() as f32).collect();
                    vector
                })
                .collect())
        }

        fn device(&self) -> String {
            "made up".into()
        }
    }

    #[test]
    fn each_statement_gets_its_closest_members_and_their_sentence_located_in_the_paper() {
        let papers = records();
        let words = Words { asked: Mutex::new(Vec::new()) };
        let vectors = words
            .encode(
                "m",
                &papers.iter().map(Paper::text).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .unwrap();
        words.asked.lock().unwrap().clear();
        let statements = [
            Statement {
                text: "The papers route tokens to experts with a router.".into(),
                section: Some("Problem".into()),
            },
            Statement { text: "Nothing of the vocabulary at all.".into(), section: None },
        ];
        let asked = Asked { model: "m", statements: &statements, members: &papers, vectors: &vectors };
        let lines = Mutex::new(Vec::new());
        let log = |line: &str| lines.lock().unwrap().push(line.to_owned());
        let progress = |_: Progress| {};
        let cancel = Cancel::new();
        let tracker = Tracker::new(&log, &progress, &cancel);
        let mut found = Vec::new();
        find(&words, &asked, &tracker, &mut |statement, kept| {
            found.push((statement.clone(), kept));
            Ok(())
        })
        .unwrap();
        assert!(words.asked.lock().unwrap()[0].starts_with("Instruct: "), "the statement, as a query");
        let (_, first) = &found[0];
        let ids: Vec<&str> = first.papers.iter().map(|found| found.paper.as_str()).collect();
        assert_eq!(ids, ["p1", "p3"], "the two papers on experts and routers, closest first; not the graph one");
        assert_eq!(first.papers[0].sentence, "A balanced router sends each token to two experts.");
        assert!(found[1].1.papers.is_empty(), "a statement nothing is close to has none");
        // shown: the sentence located in the paper's own words, with the paper's title
        let records: HashMap<String, Paper> = papers.iter().map(|paper| (paper.id.clone(), paper.clone())).collect();
        let shown = shown(&statements[0], first, &records);
        assert_eq!(shown.section.as_deref(), Some("Problem"));
        assert_eq!(shown.papers[0].title, "Routing tokens to experts at scale");
        assert_eq!(
            shown.papers[0].evidence.quote(),
            "A balanced router sends each token to two experts",
            "as the evidence module words a quote"
        );
        assert!(matches!(
            shown.papers[0].evidence.established(),
            crate::evidence::Established::Abstract { field: crate::evidence::Field::Abstract, .. }
        ));
        // a paper whose words changed so that its sentence is gone is not shown with it
        let mut changed = records.clone();
        changed.get_mut("p1").unwrap().r#abstract = "Something else entirely now.".into();
        assert_eq!(super::shown(&statements[0], first, &changed).papers.len(), 1);
    }

    /// The real model on a real run, when both are on this computer: `CONSTELLATE_MODEL` (the model's folder) and
    /// `CONSTELLATE_DATA` (a data folder with a described run). Prints, for the first communities, how close the
    /// closest and the least close members are to each statement and how long finding them took: what [`FLOOR`]
    /// was chosen from. `cargo test closest::tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the model's files and a data folder: set CONSTELLATE_MODEL and CONSTELLATE_DATA"]
    fn measure_the_closest_papers_of_a_real_run() {
        use crate::pipeline::steps::embedding::Onnx;
        use crate::pipeline::steps::model_files::Network;
        let model_folder = std::path::PathBuf::from(std::env::var("CONSTELLATE_MODEL").unwrap());
        let data = std::path::PathBuf::from(std::env::var("CONSTELLATE_DATA").unwrap());
        let library = Library::open(&data.join("library")).unwrap();
        let run = std::fs::read_dir(data.join("runs")).unwrap().next().unwrap().unwrap().path();
        let file: Value = serde_json::from_slice(&std::fs::read(run.join("run.json")).unwrap()).unwrap();
        let ids: Vec<String> =
            file["paper_ids"].as_array().unwrap().iter().map(|id| id.as_str().unwrap().to_owned()).collect();
        let model = file["settings"]["embedding_model"].as_str().unwrap().to_owned();
        let tree = super::super::read_tree(&run).unwrap().unwrap();
        let onnx = Onnx::new(std::sync::Arc::new(Network::default()));
        onnx.use_folder(model_folder);
        let (lines, cancel) = (Mutex::new(Vec::<String>::new()), Cancel::new());
        let log = |line: &str| lines.lock().unwrap().push(line.to_owned());
        let progress = |_: Progress| {};
        let tracker = Tracker::new(&log, &progress, &cancel);
        let mut least_all = Vec::new();
        let mut closest_all = Vec::new();
        for community in tree.communities.iter().take(6) {
            let statements = statements_of(community);
            let members: Vec<&str> =
                community.members.iter().filter_map(|&index| ids.get(index)).map(String::as_str).collect();
            let papers: Vec<Paper> = library.papers(&members).unwrap().into_iter().flatten().collect();
            let rows: Vec<Vec<f32>> =
                library.vectors(&model, &papers).unwrap().outer_iter().map(|row| row.to_vec()).collect();
            let started = std::time::Instant::now();
            let asked = Asked { model: &model, statements: &statements, members: &papers, vectors: &rows };
            let mut found = Vec::new();
            find(&onnx, &asked, &tracker, &mut |_, kept| {
                found.push(kept);
                Ok(())
            })
            .unwrap();
            let took = started.elapsed();
            // every member's cosine with each statement, for the spread
            let queries: Vec<String> = statements.iter().map(|statement| query(&statement.text)).collect();
            let vectors = onnx.encode(&model, &queries.iter().map(String::as_str).collect::<Vec<_>>()).unwrap();
            for (statement, vector) in statements.iter().zip(&vectors) {
                let mut all: Vec<f32> = rows.iter().map(|row| cosine(vector, row)).collect();
                all.sort_by(|a, b| b.total_cmp(a));
                let at = |share: f32| {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                    let place = ((all.len() - 1) as f32 * share) as usize;
                    all[place]
                };
                eprintln!(
                    "{} ({} papers) \"{}\": top {:.3} {:.3} {:.3} {:.3} {:.3}; median {:.3}; least {:.3}",
                    community.tree_path,
                    papers.len(),
                    statement.text.chars().take(70).collect::<String>(),
                    at(0.0),
                    all.get(1).copied().unwrap_or(0.0),
                    all.get(2).copied().unwrap_or(0.0),
                    all.get(3).copied().unwrap_or(0.0),
                    all.get(4).copied().unwrap_or(0.0),
                    at(0.5),
                    at(1.0)
                );
                closest_all.push(all[0]);
                least_all.push(at(1.0));
            }
            for kept in &found {
                for paper in &kept.papers {
                    eprintln!(
                        "    {:.3} {} | {}",
                        paper.cosine,
                        paper.paper,
                        paper.sentence.chars().take(90).collect::<String>()
                    );
                }
            }
            eprintln!("  {} statements found in {took:?} on {}", statements.len(), onnx.device());
        }
        eprintln!("closest per statement {closest_all:?}\nleast per statement {least_all:?}");
    }

    #[test]
    fn what_is_found_is_kept_by_model_statement_and_members_and_found_again_when_they_change() {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(&records(), None, Known::Keep).unwrap();
        let subject = Subject::Community { run: "r1".into(), community: "c0".into() };
        let statement = Statement { text: "A claim.".into(), section: None };
        let kept = Kept {
            statement: "A claim.".into(),
            papers: vec![KeptPaper {
                paper: "p1".into(),
                cosine: 0.5,
                sentence: "We train sparse mixtures of experts.".into(),
            }],
        };
        let members = ["p1", "p2", "p3"];
        assert_eq!(kept_of(&library, "m", &members, std::slice::from_ref(&statement)).unwrap(), [None]);
        keep(&library, &subject, "m", &members, &kept).unwrap();
        assert_eq!(
            kept_of(&library, "m", &["p3", "p1", "p2"], std::slice::from_ref(&statement)).unwrap(),
            [Some(kept.clone())]
        );
        // another model, another membership, another wording: not this one's
        assert_eq!(kept_of(&library, "other", &members, std::slice::from_ref(&statement)).unwrap(), [None]);
        assert_eq!(kept_of(&library, "m", &members[..2], std::slice::from_ref(&statement)).unwrap(), [None]);
        let reworded = Statement { text: "A claim, reworded.".into(), section: None };
        assert_eq!(kept_of(&library, "m", &members, &[reworded]).unwrap(), [None]);
    }
}
