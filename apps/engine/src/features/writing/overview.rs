//! An overview of one community: what its papers have in common, every point shown with quotes found in the
//! papers it names (`overview.py`, made to keep the engine's evidence rule).
//!
//! ```text
//! community ──prompt──> its papers' titles and abstracts (most central first, at most PAPER_TOKENS) and what the
//!                       similarity graph says of it (centre, most linked, neighbours, parts)
//!           ──estimate──> shown before anything is sent; the request accepts its most
//!           ──batch::ask_all (one item, budgeted, kept)──> Said: sections of points, each with quotes
//!           ──checked──> Overview: the points whose quotes were found in at least MIN_CITED papers
//! ```
//!
//! # The rule a point is shown by
//!
//! Repetition is what makes a point worth stating: one paper saying something is that paper's claim, several
//! saying it is a pattern of the community. The Python asked each point to cite at least three papers by number
//! and kept those whose numbers were members ([`MIN_CITED`]): a paper cited for something it does not say passed.
//! Here each point carries a quote of each paper it rests on, copied from the title or abstract it was given, and
//! is shown only when quotes from at least [`MIN_CITED`] distinct members were found in those papers
//! ([`evidence::check`] with [`Rules::pattern`]). A point whose quotes fail is left out and counted
//! ([`Counts`]); so is the summary, which follows the same rule. The quotes shown are the papers' words.
//!
//! # What is kept
//!
//! The model's answer, under the provider and model, [`PROMPT`] and the community's members ([`key_of`]): the
//! same papers asked of the same model are never paid for twice, an overview kept for another provider or model is
//! not this one's, and the names of neighbouring communities (which describing may change) do not make it
//! another. It is checked again whenever it is shown.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{DATA_RULE, Stored, StoredTree, cut, data_section, one_block};
use crate::analysis::links::central_first;
use crate::evidence::{self, Claim, Evidence, PaperText, Papers, Quote, Rules};
use crate::jobs::{Finished, Phase, Snapshot, Tracker};
use crate::library::{AnswerKey, Feature, Kept, Library, LibraryError, Paper, Slot, Subject};
use crate::model::batch::{self, Asking, BatchEstimate, Failure, Item, Outcome as Asked, Stopped};
use crate::model::chosen::Chosen;
use crate::model::language::{Ask, Budget, CHARS_PER_TOKEN, Form, Provider};

/// The version of the prompt and the answer's shape: raised when either changes, so kept answers are asked again.
pub const PROMPT: u32 = 1;
/// What the ledger calls these calls (as the Python did).
pub const PURPOSE: &str = "community overviews";
/// Distinct papers whose quotes a point needs (the Python's `MIN_CITED`).
pub const MIN_CITED: usize = 3;
/// The most the papers' titles and abstracts may take in the prompt, in tokens reckoned at [`CHARS_PER_TOKEN`]:
/// about 150 abstracts. A larger community is given its most central papers, and the overview says how many.
pub const PAPER_TOKENS: u64 = 60_000;
/// The most of one abstract, and of one title, in the prompt, in characters.
pub const LONGEST_ABSTRACT: usize = 3_000;
pub const LONGEST_TITLE: usize = 300;
/// Papers listed by id in each line of the graph's facts.
const LISTED: usize = 12;
/// Papers listed by id for each part of the community.
const LISTED_PER_PART: usize = 20;
/// The longest name or term list put in the facts, in characters.
const LONGEST_LABEL: usize = 120;
/// The most the answer may take, thinking included (as the Python).
pub const MAX_TOKENS: u32 = 32_000;
/// What the answer is likely to take, for the estimate.
pub const LIKELY_TOKENS: u32 = 10_000;
/// What the job writing an overview is called in `/api/status`.
pub const JOB: &str = "writing an overview";

/// The sections, in the order shown: the key the answer has them under, and their heading.
pub const SECTIONS: [(&str, &str); 5] = [
    ("common_methods", "Common methods"),
    ("recurring_findings", "Recurring findings"),
    ("promising_directions", "Promising directions"),
    ("structure_patterns", "Patterns in the structure"),
    ("open_questions", "Open questions and disagreements"),
];

/// The instructions. They name no vendor's features: any model that can answer in JSON can follow them.
pub const SYSTEM: &str = "You are writing an overview of one research community for a researcher who wants to know what is going on in it without reading every abstract.

You are given the titles and abstracts of the community's papers, each labelled with the paper's id, and facts about its structure from a similarity graph.

What counts is repetition. A method, result or idea is worth a point when several papers share it; something only one paper says is that paper's claim, not a pattern. Every point must be supported by at least 3 papers that really do share it, and should be supported by more when more share it (up to about 8, the clearest first). For each supporting paper give one quote: words copied exactly from that paper's title or abstract as given (at least five words, at most one sentence, never changed, joined or shortened), with that paper's id. Do not pad a point with papers that are merely on the same topic. If fewer than 3 papers support something, leave it out.

Sections:
- common_methods: techniques, architectures, training recipes, benchmarks or analysis tools that recur.
- recurring_findings: empirical or theoretical results that several papers report or agree on.
- promising_directions: ideas that several recent papers converge on and that look like they are gaining momentum; say what makes each promising, from what the papers report.
- structure_patterns: what the graph facts show: how the parts of the community differ, what the central and most-linked papers have in common, which neighbouring communities this one borders and through which kind of paper.
- open_questions: places where papers disagree, report conflicting results, or name the same unsolved problem.

Each point has a title, which is a specific claim in at most 12 words (not a topic label: \"Reward shaping from verifier feedback replaces human labels\", not \"Reward design\"), an insight of 1 to 3 sentences that states what the supporting papers share, concretely, and the quotes. Order the points of a section by how many papers support them, most first. 3 to 7 points per section; fewer if the evidence is thin. Also give a title for the whole overview (at most 10 words) and a summary of 2 to 4 sentences, supported by quotes of at least 3 papers in the same way. Write plainly; state only what the abstracts support.

";

/// The JSON schema of an answer ([`Said`]): every object closed and every property required.
pub fn schema() -> Value {
    let strict = |properties: Value, required: &[&str]| json!({"type": "object", "additionalProperties": false, "required": required, "properties": properties});
    let quote = strict(json!({"paper": {"type": "string"}, "quote": {"type": "string"}}), &["paper", "quote"]);
    let quotes = json!({"type": "array", "items": quote});
    let point = strict(
        json!({"title": {"type": "string"}, "insight": {"type": "string"}, "quotes": quotes.clone()}),
        &["title", "insight", "quotes"],
    );
    let summary = strict(json!({"text": {"type": "string"}, "quotes": quotes}), &["text", "quotes"]);
    let mut properties = serde_json::Map::new();
    properties.insert("title".to_owned(), json!({"type": "string"}));
    properties.insert("summary".to_owned(), summary);
    for (key, _) in SECTIONS {
        properties.insert(key.to_owned(), json!({"type": "array", "items": point.clone()}));
    }
    let mut required = vec!["title", "summary"];
    required.extend(SECTIONS.iter().map(|(key, _)| *key));
    strict(Value::Object(properties), &required)
}

fn system() -> String {
    format!("{SYSTEM}{DATA_RULE}")
}

/// The model's answer as kept: each point as it came, read on its own when shown (one malformed point fails only
/// itself).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Said {
    pub title: String,
    pub summary: Value,
    #[serde(default)]
    pub common_methods: Vec<Value>,
    #[serde(default)]
    pub recurring_findings: Vec<Value>,
    #[serde(default)]
    pub promising_directions: Vec<Value>,
    #[serde(default)]
    pub structure_patterns: Vec<Value>,
    #[serde(default)]
    pub open_questions: Vec<Value>,
}

impl Said {
    fn section(&self, key: &str) -> &[Value] {
        match key {
            "common_methods" => &self.common_methods,
            "recurring_findings" => &self.recurring_findings,
            "promising_directions" => &self.promising_directions,
            "structure_patterns" => &self.structure_patterns,
            "open_questions" => &self.open_questions,
            _ => &[],
        }
    }
}

/// A point as the model wrote it.
#[derive(Deserialize)]
struct SaidPoint {
    title: String,
    insight: String,
    quotes: Vec<Quote>,
}

#[derive(Deserialize)]
struct SaidSummary {
    text: String,
    quotes: Vec<Quote>,
}

/// The run's graph, as much of it as an overview needs (`graph.json`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<(usize, usize, f64)>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(default)]
    pub title: String,
}

/// The key an overview of the papers `members` (ids) by `chosen` is kept under.
pub fn key_of(chosen: &Chosen, members: &[&str]) -> AnswerKey {
    chosen.key(Feature::Overview, PROMPT, members, "")
}

/// An overview ready to be estimated and asked: its prompt and what its answer is kept under.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub chosen: Chosen,
    pub key: AnswerKey,
    pub subject: Subject,
    /// The community's papers, by id, in its order.
    pub members: Vec<String>,
    /// Of those, the papers in the prompt, most central first.
    pub given: Vec<String>,
    question: String,
    system: String,
    schema: Value,
}

/// The facts of a community the prompt gives besides its papers, and its papers' records by id.
pub struct Community<'a> {
    pub run: &'a str,
    pub tree: &'a StoredTree,
    pub community: &'a Stored,
    pub graph: &'a Graph,
    /// The library's records of the community's members (those it has).
    pub records: &'a HashMap<String, Paper>,
}

impl Community<'_> {
    fn member_ids(&self) -> Vec<String> {
        self.community
            .members
            .iter()
            .filter_map(|&index| self.graph.nodes.get(index))
            .map(|node| node.id.clone())
            .collect()
    }
}

/// The ids of `indices` that are among `given`, in their order, at most `most`, as a list for the prompt.
fn listed(graph: &Graph, indices: impl IntoIterator<Item = usize>, given: &HashSet<&str>, most: usize) -> String {
    let ids: Vec<&str> = indices
        .into_iter()
        .filter_map(|index| graph.nodes.get(index))
        .map(|node| node.id.as_str())
        .filter(|id| given.contains(id))
        .take(most)
        .collect();
    if ids.is_empty() { "none of the papers given".to_owned() } else { ids.join(", ") }
}

fn terms(community: &Stored, most: usize) -> String {
    cut(&one_block(&community.keywords.iter().take(most).cloned().collect::<Vec<_>>().join(", ")), LONGEST_LABEL)
}

fn label(community: &Stored) -> String {
    cut(&one_block(&community.label()), LONGEST_LABEL)
}

/// The prompt for `about`, asked of `chosen` (`overview.py`'s `_prompt`, the papers by id).
pub fn prepare(chosen: &Chosen, about: &Community<'_>) -> Prepared {
    let community = about.community;
    let graph = about.graph;
    let members = about.member_ids();
    let node_ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let room = (PAPER_TOKENS as f64 * CHARS_PER_TOKEN) as usize;
    let mut papers_text = String::new();
    let mut used = 0;
    let mut given = Vec::new();
    for id in central_first(&members, &node_ids, &graph.edges) {
        let Some(paper) = about.records.get(&id) else { continue };
        let entry = format!(
            "[paper {id}] {}\n{}\n\n",
            cut(&one_block(&paper.title), LONGEST_TITLE),
            cut(&one_block(&paper.r#abstract), LONGEST_ABSTRACT)
        );
        let length = entry.chars().count();
        if used + length > room {
            break;
        }
        used += length;
        papers_text.push_str(&entry);
        given.push(id);
    }
    let given_set: HashSet<&str> = given.iter().map(String::as_str).collect();

    // what the graph says, as overview.py worked it out
    let inside: HashSet<usize> = community.members.iter().copied().collect();
    let mut root_of: HashMap<usize, &Stored> = HashMap::new();
    for root in &about.tree.communities {
        for &member in &root.members {
            root_of.insert(member, root);
        }
    }
    let mut degree: BTreeMap<usize, usize> = BTreeMap::new();
    let mut bridges: BTreeMap<String, usize> = BTreeMap::new();
    for &(a, b, _) in &graph.edges {
        for (here, there) in [(a, b), (b, a)] {
            if inside.contains(&here) {
                *degree.entry(here).or_default() += 1;
                if !inside.contains(&there) {
                    if let Some(root) = root_of.get(&there) {
                        *bridges.entry(label(root)).or_default() += 1;
                    }
                }
            }
        }
    }
    let mut most_linked: Vec<(usize, usize)> = degree.into_iter().collect();
    most_linked.sort_by(|(a, linked_a), (b, linked_b)| linked_b.cmp(linked_a).then(a.cmp(b)));
    let mut neighbours: Vec<(String, usize)> = bridges.into_iter().collect();
    neighbours.sort_by(|(a, links_a), (b, links_b)| links_b.cmp(links_a).then_with(|| a.cmp(b)));
    let index_of: HashMap<&str, usize> = node_ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
    let central = community
        .nearest_to_centroid
        .iter()
        .filter_map(|nearest| index_of.get(nearest.id.as_str()).copied())
        .filter(|index| inside.contains(index));

    let mut lines = vec![
        format!(
            "Community: {} ({} papers; the {} most central of them are given below)",
            label(community),
            members.len(),
            given.len()
        ),
        format!("Its characteristic terms: {}", terms(community, 12)),
        String::new(),
        "STRUCTURE (from the similarity graph)".to_owned(),
        format!(
            "Papers closest to the community's mean embedding, i.e. its centre: {}",
            listed(graph, central, &given_set, LISTED)
        ),
        format!(
            "Papers with the most links to other papers of the community: {}",
            listed(graph, most_linked.iter().map(|(index, _)| *index), &given_set, LISTED)
        ),
        format!(
            "Its links to other communities go to: {}",
            if neighbours.is_empty() {
                "none".to_owned()
            } else {
                neighbours
                    .iter()
                    .take(6)
                    .map(|(name, count)| format!("{name} ({count} links)"))
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        ),
    ];
    for child in &community.children {
        let child_ids: Vec<String> =
            child.members.iter().filter_map(|&index| graph.nodes.get(index)).map(|node| node.id.clone()).collect();
        let ordered = central_first(&child_ids, &node_ids, &graph.edges);
        let shown: Vec<&str> =
            ordered.iter().map(String::as_str).filter(|id| given_set.contains(id)).take(LISTED_PER_PART).collect();
        lines.push(format!(
            "Sub-community \"{}\" ({} papers; terms: {}): {}",
            label(child),
            child.members.len(),
            terms(child, 8),
            if shown.is_empty() { "none of the papers given".to_owned() } else { shown.join(", ") }
        ));
    }
    lines.push(String::new());
    lines.push("PAPERS".to_owned());
    let body = format!("{}\n{papers_text}", lines.join("\n"));
    let question = format!("{}\n\nWrite the overview of this community.", data_section(&body));
    let ids: Vec<&str> = members.iter().map(String::as_str).collect();
    Prepared {
        chosen: chosen.clone(),
        key: key_of(chosen, &ids),
        subject: Subject::Community { run: about.run.to_owned(), community: community.tree_path.clone() },
        members,
        given,
        question,
        system: system(),
        schema: schema(),
    }
}

impl Prepared {
    /// The one item of the batch it is asked as.
    pub fn item(&self) -> Item<'_> {
        Item {
            slot: Slot { feature: Feature::Overview, key: self.key.clone(), subject: self.subject.clone() },
            prompt: PROMPT,
            ask: Ask {
                purpose: PURPOSE,
                model: self.chosen.model(),
                system: &self.system,
                shared: None,
                question: &self.question,
                form: Form::Json(&self.schema),
                effort: self.chosen.effort(),
                max_tokens: MAX_TOKENS,
            },
            likely_tokens: LIKELY_TOKENS,
        }
    }

    /// What asking would cost (nothing when its answer is kept). Nothing is sent.
    pub fn estimate(&self, library: &Library) -> Result<BatchEstimate, LibraryError> {
        batch::estimate_batch::<Said>(library, &[self.item()], &self.chosen)
    }

    /// Its answer, when kept.
    pub fn kept(&self, library: &Library) -> Result<Option<Kept<Said>>, LibraryError> {
        library.kept::<Said>(Feature::Overview, &self.key)
    }

    /// The prompt as sent: for the tests.
    pub fn question(&self) -> &str {
        &self.question
    }

    /// The first line of the log of the job writing it: how a view tells its own job from another.
    pub fn job_line(&self) -> String {
        format!("{}{}.", job_prefix(&self.subject), self.key.as_str())
    }
}

/// The start of the first log line of a job writing an overview of `subject`, whatever its key.
pub fn job_prefix(subject: &Subject) -> String {
    match subject {
        Subject::Community { run, community } => format!("Writing the overview of {community} of {run}: "),
        Subject::Paper(id) => format!("Writing the overview of paper {id}: "),
        Subject::Run(run) => format!("Writing the overview of {run}: "),
    }
}

/// Why an overview was not written.
#[derive(Debug, thiserror::Error)]
pub enum NotWritten {
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error(transparent)]
    Failed(Failure),
    #[error(transparent)]
    Stopped(Stopped),
}

/// Ask for it (one call, within `budget`, its answer kept with its cost as it arrives), or hand back the one kept.
pub fn ask(
    asking: Asking<'_>,
    prepared: &Prepared,
    budget: &Budget,
    tracker: &Tracker<'_>,
) -> Result<Said, NotWritten> {
    tracker.log(&prepared.job_line());
    tracker.log(&format!(
        "Reading {} of the community's {} papers with {} ...",
        prepared.given.len(),
        prepared.members.len(),
        prepared.chosen
    ));
    match batch::ask_all::<Said>(asking, &[prepared.item()], budget, tracker)?.pop() {
        Some(Asked::Kept(kept)) => Ok(kept.answer),
        Some(Asked::Answered(answered)) => Ok(answered.answer),
        Some(Asked::Failed(why)) => Err(NotWritten::Failed(why)),
        Some(Asked::NotStarted(why)) => Err(NotWritten::Stopped(why)),
        None => Err(NotWritten::Stopped(Stopped::Cancelled)),
    }
}

/// A paper a point rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CitedPaper {
    pub id: String,
    pub title: String,
}

/// A point of an overview, shown: `OverviewPoint` in the contracts, with its evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Point {
    pub title: String,
    pub insight: String,
    /// The papers whose quotes were found, in the order of its quotes.
    pub papers: Vec<CitedPaper>,
    /// The quotes found, as the papers word them: each opens the paper at its place.
    pub evidence: Vec<Evidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Section {
    pub heading: String,
    pub points: Vec<Point>,
}

/// How many points were written, shown and left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Points the model wrote, well formed or not.
    pub written: usize,
    pub shown: usize,
    /// Left out: their quotes were not found in at least [`MIN_CITED`] of the papers they name.
    pub not_found: usize,
    /// Left out: not of the shape asked for.
    pub malformed: usize,
    /// The summary was left out, its quotes not found in enough papers.
    pub summary_left_out: bool,
}

/// An overview, checked: `Overview` in the contracts, with its evidence and what was left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Overview {
    pub title: String,
    /// Empty when it was left out ([`Counts::summary_left_out`]).
    pub summary: String,
    pub summary_evidence: Vec<Evidence>,
    pub sections: Vec<Section>,
    /// The model that wrote it, as its provider named it.
    pub model: String,
    pub provider: Provider,
    /// The community's papers, and of those, how many the model was given.
    pub papers: usize,
    pub papers_given: usize,
    pub counts: Counts,
}

/// The statement of a point while it is checked: its section, its place, and what it says.
type Statement = (usize, usize, String, String);

/// Check `said` against `papers` (the community's members' titles and abstracts), as the module says.
pub fn checked(
    said: &Said,
    papers: &Papers,
    titles: &HashMap<String, String>,
) -> (String, Vec<Evidence>, Vec<Section>, Counts) {
    let mut counts = Counts::default();
    let mut claims: Vec<Claim<Statement>> = Vec::new();
    for (section, (key, _)) in SECTIONS.iter().enumerate() {
        for (place, value) in said.section(key).iter().enumerate() {
            counts.written += 1;
            match serde_json::from_value::<SaidPoint>(value.clone()) {
                Ok(point) if !point.title.trim().is_empty() => claims.push(Claim {
                    statement: (section, place, one_block(&point.title), one_block(&point.insight)),
                    quotes: point.quotes,
                }),
                Ok(_) | Err(_) => counts.malformed += 1,
            }
        }
    }
    let kept = evidence::check(claims, papers, Rules::pattern(MIN_CITED));
    counts.shown = kept.cited.len();
    counts.not_found = counts.written - counts.malformed - counts.shown;
    let mut by_section: Vec<Vec<Point>> = vec![Vec::new(); SECTIONS.len()];
    for cited in kept.cited {
        let (section, _, title, insight) = cited.statement;
        let mut ids: Vec<&str> = Vec::new();
        for found in &cited.evidence {
            if !ids.contains(&found.paper()) {
                ids.push(found.paper());
            }
        }
        let papers = ids
            .iter()
            .map(|id| CitedPaper { id: (*id).to_owned(), title: titles.get(*id).cloned().unwrap_or_default() })
            .collect();
        by_section[section].push(Point { title, insight, papers, evidence: cited.evidence });
    }
    let sections = by_section
        .into_iter()
        .zip(SECTIONS)
        .filter(|(points, _)| !points.is_empty())
        .map(|(mut points, (_, heading))| {
            // most supported first, as the Python; the model's order among equals
            points.sort_by_key(|point| std::cmp::Reverse(point.papers.len()));
            Section { heading: heading.to_owned(), points }
        })
        .collect();
    let summary = serde_json::from_value::<SaidSummary>(said.summary.clone())
        .ok()
        .map(|summary| Claim { statement: one_block(&summary.text), quotes: summary.quotes })
        .filter(|claim| !claim.statement.is_empty());
    let (summary, summary_evidence) =
        match summary.map(|claim| evidence::check(vec![claim], papers, Rules::pattern(MIN_CITED))) {
            Some(mut checked) if !checked.cited.is_empty() => {
                let cited = checked.cited.remove(0);
                (cited.statement, cited.evidence)
            }
            _ => {
                counts.summary_left_out = true;
                (String::new(), Vec::new())
            }
        };
    (summary, summary_evidence, sections, counts)
}

/// The papers an overview of `members` may cite, as the model was given them: their titles and abstracts, never a
/// PDF's text (which it was not given), and their titles by id.
pub fn citable(members: &[String], records: &HashMap<String, Paper>) -> (Papers, HashMap<String, String>) {
    let mut papers = Papers::new();
    let mut titles = HashMap::new();
    for id in members {
        if let Some(paper) = records.get(id) {
            papers.insert(
                id.clone(),
                PaperText::Unread { title: paper.title.clone(), abstract_text: paper.r#abstract.clone() },
            );
            titles.insert(id.clone(), paper.title.clone());
        }
    }
    (papers, titles)
}

/// The overview kept under `prepared`'s key, checked again: what is shown.
pub fn shown(kept: &Kept<Said>, prepared: &Prepared, records: &HashMap<String, Paper>) -> Overview {
    let (papers, titles) = citable(&prepared.members, records);
    let (summary, summary_evidence, sections, counts) = checked(&kept.answer, &papers, &titles);
    Overview {
        title: one_block(&kept.answer.title),
        summary,
        summary_evidence,
        sections,
        model: kept.model.clone(),
        provider: prepared.chosen.provider(),
        papers: prepared.members.len(),
        papers_given: prepared.given.len(),
        counts,
    }
}

/// Where an overview stands for the model chosen now.
#[derive(Clone, Debug, PartialEq)]
pub enum Standing {
    /// Not written for this model, and not being written: it can be asked for (at its estimate).
    Idle,
    /// Being written now: by this model, or by the one chosen when it was started (nothing else starts meanwhile).
    Running,
    /// Kept, and checked again.
    Done(Box<Overview>),
    /// The last attempt for this model failed, for this reason.
    Failed(String),
}

impl Standing {
    /// From what is kept and the engine's job: kept wins; then a job writing an overview of this community,
    /// whichever key it is for (`prefix`); then a failure of the job for exactly this one (`line`).
    pub fn of(kept: Option<Overview>, job: &Snapshot<Finished>, prefix: &str, line: &str) -> Self {
        if let Some(overview) = kept {
            return Standing::Done(Box::new(overview));
        }
        let first = job.log.first().map(String::as_str);
        // a job just started has not written its first line yet: for that moment, any overview's job is shown so
        let ours = job.kind == JOB && first.is_none_or(|first| first.starts_with(prefix));
        match (job.state, ours) {
            (Phase::Running, true) => Standing::Running,
            (Phase::Failed, true) if first == Some(line) => {
                Standing::Failed(job.error.clone().unwrap_or_else(|| "the overview could not be written".to_owned()))
            }
            _ => Standing::Idle,
        }
    }

    pub fn phase(&self) -> Phase {
        match self {
            Standing::Idle => Phase::Idle,
            Standing::Running => Phase::Running,
            Standing::Done(_) => Phase::Done,
            Standing::Failed(_) => Phase::Failed,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library::papers::tests::paper;
    use crate::model::language::{Effort, Unavailable, strict_ready};

    const MODEL: &str = "claude-opus-5-5";

    fn chosen(provider: Provider, model: &str) -> Chosen {
        Chosen::of(provider, model, Effort::Medium, Err(Unavailable::NoKey))
    }

    fn records(count: usize) -> HashMap<String, Paper> {
        (0..count)
            .map(|index| {
                let mut made = paper(&format!("p{index}"));
                made.title = format!("Title {index}");
                made.r#abstract = format!(
                    "Abstract {index}. We train sparse mixtures of experts with a router that balances the load. \
                     Paper {index} reports that the balanced router improves accuracy."
                );
                (made.id.clone(), made)
            })
            .collect()
    }

    fn graph(count: usize, edges: Vec<(usize, usize, f64)>) -> Graph {
        Graph {
            nodes: (0..count).map(|index| Node { id: format!("p{index}"), title: format!("Title {index}") }).collect(),
            edges,
        }
    }

    fn tree() -> StoredTree {
        serde_json::from_value(json!({"communities": [
            {"tree_path": "c0", "name": "Mine", "keywords": ["a", "b"], "members": [0, 1, 2],
             "nearest_to_centroid": [{"id": "p1", "in_community": true}, {"id": "p4", "in_community": false}],
             "children": [{"tree_path": "c0.0", "name": "Half", "keywords": ["k"], "members": [0, 1]}]},
            {"tree_path": "c1", "name": "Theirs", "keywords": ["z"], "members": [3, 4, 5]}
        ]}))
        .unwrap()
    }

    #[test]
    fn the_schema_is_strict_ready() {
        assert!(strict_ready(&schema()), "every object closed and every property required");
    }

    #[test]
    fn the_prompt_gives_every_member_by_id_and_reports_the_structure() {
        let tree = tree();
        let records = records(6);
        let graph = graph(6, vec![(0, 1, 0.9), (2, 3, 0.5)]);
        let about =
            Community { run: "r1", tree: &tree, community: tree.find("c0").unwrap(), graph: &graph, records: &records };
        let prepared = prepare(&chosen(Provider::Anthropic, MODEL), &about);
        let prompt = prepared.question();
        for index in 0..3 {
            assert!(prompt.contains(&format!("[paper p{index}] Title {index}")), "{prompt}");
        }
        assert!(!prompt.contains("Title 4"), "not a member");
        assert!(prompt.contains("Theirs (1 links)") && prompt.contains("Sub-community \"Half\""), "{prompt}");
        assert!(prompt.contains("its centre: p1\n"), "{prompt}");
        assert_eq!(prepared.given.len(), 3);
        // the papers are in the data section, and the instructions say what that is
        let end = prompt.find(super::super::DATA_ENDS).unwrap();
        assert!(prompt.find("[paper p2]").unwrap() < end);
        assert!(prepared.system.contains(DATA_RULE));
        assert!(!SYSTEM.contains("Claude"));
    }

    #[test]
    fn a_large_community_is_given_its_most_central_papers_within_the_bound() {
        let count = 400;
        let mut records = records(count);
        for paper in records.values_mut() {
            paper.r#abstract = "word ".repeat(1_000);
        }
        // paper 7 is linked to every other: the most central
        let edges = (0..count).filter(|other| *other != 7).map(|other| (7, other, 0.5)).collect();
        let graph = graph(count, edges);
        let tree: StoredTree = serde_json::from_value(
            json!({"communities": [{"tree_path": "c0", "members": (0..count).collect::<Vec<_>>()}]}),
        )
        .unwrap();
        let about =
            Community { run: "r1", tree: &tree, community: &tree.communities[0], graph: &graph, records: &records };
        let prepared = prepare(&chosen(Provider::Anthropic, MODEL), &about);
        assert_eq!(prepared.given[0], "p7");
        assert!(prepared.given.len() < count);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        let room = (PAPER_TOKENS as f64 * CHARS_PER_TOKEN) as usize;
        assert!(prepared.question().chars().count() < room + 5_000);
        assert_eq!(prepared.members.len(), count);
    }

    fn said(points: Vec<Value>, summary: Value) -> Said {
        Said {
            title: "T".into(),
            summary,
            common_methods: points,
            recurring_findings: Vec::new(),
            promising_directions: Vec::new(),
            structure_patterns: Vec::new(),
            open_questions: Vec::new(),
        }
    }

    fn quote(paper: usize) -> Value {
        json!({"paper": format!("p{paper}"), "quote": format!("Paper {paper} reports that the balanced router improves accuracy")})
    }

    #[test]
    fn a_point_is_shown_only_with_quotes_found_in_enough_of_the_papers_it_names() {
        let members: Vec<String> = [2, 4, 6, 8].iter().map(|index| format!("p{index}")).collect();
        let (papers, titles) = citable(&members, &records(10));
        let point = |title: &str, quotes: Vec<Value>| json!({"title": title, "insight": "i", "quotes": quotes});
        let misquote =
            |paper: &str| json!({"paper": paper, "quote": "transformers were never needed for any of this work"});
        let points = vec![
            // a repeat counts once
            point("well supported", vec![quote(2), quote(4), quote(6), quote(6), quote(8)]),
            point("too thin", vec![quote(2), quote(4)]),
            // papers that are not the community's: refused, whatever they say
            point("invented papers", vec![quote(2), quote(4), quote(3), quote(99)]),
            // the Python kept this one: three members cited, but the words are in none of them
            point("misquoted", vec![misquote("p2"), misquote("p4"), misquote("p6")]),
            json!({"title": "no quotes field"}),
        ];
        let summary = json!({"text": "Most papers balance a router.", "quotes": [quote(2), quote(4), quote(8)]});
        let (summary, summary_evidence, sections, counts) = checked(&said(points, summary), &papers, &titles);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading, "Common methods");
        let [point] = sections[0].points.as_slice() else { panic!("{sections:?}") };
        assert_eq!(point.title, "well supported");
        assert_eq!(point.papers.iter().map(|paper| paper.id.as_str()).collect::<Vec<_>>(), ["p2", "p4", "p6", "p8"]);
        assert_eq!(point.papers[0].title, "Title 2");
        assert_eq!(point.evidence.len(), 4);
        assert!(point.evidence.iter().all(|found| found.quote().contains("balanced router")));
        assert_eq!(counts, Counts { written: 5, shown: 1, not_found: 3, malformed: 1, summary_left_out: false });
        assert_eq!(summary, "Most papers balance a router.");
        assert_eq!(summary_evidence.len(), 3);
        // a summary resting on fewer papers is left out, and said to be
        let thin = json!({"text": "One paper says so.", "quotes": [quote(2)]});
        let (summary, _, _, counts) = checked(&said(Vec::new(), thin), &papers, &titles);
        assert_eq!((summary.as_str(), counts.summary_left_out), ("", true));
    }

    #[test]
    fn an_overview_is_kept_by_provider_model_and_members_not_by_their_order() {
        let anthropic = chosen(Provider::Anthropic, MODEL);
        assert_eq!(key_of(&anthropic, &["a", "b", "c"]), key_of(&anthropic, &["c", "a", "b"]));
        assert_ne!(key_of(&anthropic, &["a", "b", "c"]), key_of(&anthropic, &["a", "b"]));
        let haiku = chosen(Provider::Anthropic, "claude-haiku-5-5");
        assert_ne!(key_of(&anthropic, &["a", "b"]), key_of(&haiku, &["a", "b"]));
        assert_ne!(key_of(&anthropic, &["a", "b"]), key_of(&chosen(Provider::OpenRouter, MODEL), &["a", "b"]));
    }

    fn snapshot(kind: &str, state: Phase, first: Option<&str>, error: Option<&str>) -> Snapshot<Finished> {
        Snapshot {
            id: 1,
            kind: kind.into(),
            about: None,
            state,
            result: None,
            error: error.map(str::to_owned),
            log: first.map(|line| vec![line.to_owned()]).unwrap_or_default(),
            total: 1,
            progress: None,
            papers: None,
        }
    }

    #[test]
    fn where_an_overview_stands_follows_what_is_kept_and_the_engines_job() {
        let subject = Subject::Community { run: "r1".into(), community: "c0".into() };
        let prefix = job_prefix(&subject);
        let ours = format!("{prefix}aaaa.");
        let theirs = format!("{prefix}bbbb."); // the same community, another model's key
        let other = format!("{}aaaa.", job_prefix(&Subject::Community { run: "r1".into(), community: "c1".into() }));
        let idle = snapshot("", Phase::Idle, None, None);
        assert_eq!(Standing::of(None, &idle, &prefix, &ours), Standing::Idle);
        // being written: by this model, or by the one chosen when it began
        for line in [&ours, &theirs] {
            let running = snapshot(JOB, Phase::Running, Some(line), None);
            assert_eq!(Standing::of(None, &running, &prefix, &ours), Standing::Running);
        }
        // another community's, or another kind of job: not this one's
        let elsewhere = snapshot(JOB, Phase::Running, Some(&other), None);
        assert_eq!(Standing::of(None, &elsewhere, &prefix, &ours), Standing::Idle);
        let a_run = snapshot("a run", Phase::Running, Some(&ours), None);
        assert_eq!(Standing::of(None, &a_run, &prefix, &ours), Standing::Idle);
        // a failure is told only for the overview it was for
        let failed = snapshot(JOB, Phase::Failed, Some(&ours), Some("rate limited"));
        assert_eq!(Standing::of(None, &failed, &prefix, &ours), Standing::Failed("rate limited".into()));
        let failed_other_model = snapshot(JOB, Phase::Failed, Some(&theirs), Some("rate limited"));
        assert_eq!(Standing::of(None, &failed_other_model, &prefix, &ours), Standing::Idle);
        // and one that finished for another model says nothing of this one
        let done_other_model = snapshot(JOB, Phase::Done, Some(&theirs), None);
        assert_eq!(Standing::of(None, &done_other_model, &prefix, &ours), Standing::Idle);
        // kept wins
        let overview = Overview {
            title: "T".into(),
            summary: String::new(),
            summary_evidence: Vec::new(),
            sections: Vec::new(),
            model: MODEL.into(),
            provider: Provider::Anthropic,
            papers: 3,
            papers_given: 3,
            counts: Counts::default(),
        };
        let running = snapshot(JOB, Phase::Running, Some(&theirs), None);
        assert_eq!(Standing::of(Some(overview.clone()), &running, &prefix, &ours), Standing::Done(Box::new(overview)));
    }
}
