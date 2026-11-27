//! What kind of citation each citing sentence makes, as the language model reads it, and what those come to for a
//! paper and a community: "cited 14 times in this library: 6 as a baseline, 3 used, 1 contrasted, 4 mentions".
//!
//! The lesson of scite (docs/research-tools-survey.md, section 4) is binding here. Its labels are 92.6% "mentioning",
//! and an independent study found it misses most supporting and contrasting citations; a label alone is a verdict
//! nobody can check. So:
//!
//! - **A label is never shown without its sentence**: what the interface gets is the citing sentence's evidence
//!   with the label beside it, never a label alone.
//! - **A label other than "mention" must be justified** by words the model quotes from the citing sentence or
//!   its neighbours, and those words are looked for there ([`judge`]): not found, and the citation counts as a
//!   mention that was "not justified" ([`Verdict::NotJustified`]), shown and counted as such. The words found are
//!   evidence too (the paper's own wording at its place).
//! - **Counts are counts**, each with what it is out of ([`Tally`]): never a score.
//!
//! The model is asked about many sentences at once ([`PER_CALL`]), grouped by the paper they cite, with that
//! paper's title and abstract so the model knows what is cited. Each sentence's reading is kept under its own key
//! (citing paper, cited paper, the sentence's address, the model, the prompt's version: [`sentence_key`]), so a
//! sentence is never paid for twice, whatever batch it was asked in. A batch's whole answer is kept too (under a
//! key made of its sentences' keys and its question), so that a job stopped half way loses nothing of what was
//! answered. A reading is kept with the sentence it is of, and counts only for that sentence: a PDF replaced since
//! may have another sentence at the same place ([`kept_reading`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence::{Evidence, SHORTEST_QUOTE};
use crate::features::citation_contexts::{Context, Grouping, Section};
use crate::jobs::Tracker;
use crate::library::{AnswerKey, Feature, Library, LibraryError, Slot, Subject};
use crate::model::batch::{self, Asking, Item, Outcome};
use crate::model::chosen::Chosen;
use crate::model::language::{Ask, Budget, Form};
use crate::text::pdf::passages::{Address, Match, Passages, letters_in, locate_in};

/// The version of the prompt and the answer's shape. Raise it when either changes: kept readings are asked again.
pub const PROMPT: u32 = 1;
/// Citing sentences asked about in one call.
pub const PER_CALL: usize = 25;
/// The cited papers a first, sample run is about: the most cited of the community.
pub const SAMPLE: usize = 20;
/// Letters and digits a justifying quote needs: what any quote needs to be evidence ([`SHORTEST_QUOTE`]); fewer
/// ("uses") could be found anywhere.
pub const SHORTEST_JUSTIFICATION: usize = SHORTEST_QUOTE;
/// Characters of the cited paper's abstract put in the question: enough to know what it is.
const ABSTRACT_CHARS: usize = 1_200;
/// What one sentence's reading is likely to take, and at most, in tokens (thinking included).
const LIKELY_PER_SENTENCE: u32 = 70;
const MOST_PER_SENTENCE: u32 = 160;
/// Thinking and the answer's frame, per call.
const LIKELY_PER_CALL: u32 = 400;
const MOST_PER_CALL: u32 = 2_000;

/// What a citation does, for machine-learning papers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationKind {
    /// Builds on its method, model or code.
    Uses,
    /// Uses its dataset or benchmark.
    UsesData,
    /// Compares against it as a baseline.
    Baseline,
    /// Extends it.
    Extends,
    /// Agrees with its finding, or confirms it.
    Supports,
    /// Reports a different or contradicting result, or criticises it.
    Contrasts,
    /// Background: any other mention.
    Mention,
}

impl CitationKind {
    pub const ALL: [CitationKind; 7] = [
        CitationKind::Uses,
        CitationKind::UsesData,
        CitationKind::Baseline,
        CitationKind::Extends,
        CitationKind::Supports,
        CitationKind::Contrasts,
        CitationKind::Mention,
    ];

    /// As the model is asked to write it, and as the interface is told it.
    pub const fn name(self) -> &'static str {
        match self {
            CitationKind::Uses => "uses",
            CitationKind::UsesData => "uses_data",
            CitationKind::Baseline => "baseline",
            CitationKind::Extends => "extends",
            CitationKind::Supports => "supports",
            CitationKind::Contrasts => "contrasts",
            CitationKind::Mention => "mention",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name.trim())
    }
}

/// What the model said about one citation, as it said it: kept, and judged again whenever it is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classified {
    pub kind: CitationKind,
    /// The words of the sentence or its neighbours that show it; empty for a mention.
    pub quote: String,
    /// The citing sentence the model was asked about. Its key is its place in the paper ([`sentence_key`]), and a
    /// PDF replaced since can have another sentence at that very place: the reading counts only for the sentence
    /// asked about ([`kept_reading`]). None in readings kept before it was kept with them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentence: Option<String>,
}

/// What a reading of a citation is worth, once its quote was looked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A kind other than mention, with the place of the words that show it and those words as the sentence holds
    /// them (what the paper's page must say at that place for it to be shown as justified: [`Judgement::of`]).
    Justified { kind: CitationKind, at: Address, words: String },
    /// The model read it as a mention.
    Mention,
    /// The model said `claimed`, but its words were not in the sentence or its neighbours: a mention, not justified.
    NotJustified { claimed: CitationKind },
    /// Not asked yet, or the model's answer said nothing usable about it.
    NotClassified,
}

/// Is the model's reading of `context` justified by words found in the citing sentence or its neighbours?
pub fn judge(context: &Context, classified: Option<&Classified>) -> Verdict {
    let Some(classified) = classified else { return Verdict::NotClassified };
    if classified.kind == CitationKind::Mention {
        return Verdict::Mention;
    }
    let claimed = classified.kind;
    if letters_in(&classified.quote) < SHORTEST_JUSTIFICATION {
        return Verdict::NotJustified { claimed };
    }
    context
        .window()
        .find_map(|(address, sentence)| {
            // word for word only: the evidence made of it says it was found exactly
            let (span, words, matched) = locate_in(sentence, &classified.quote)?;
            (matched == Match::Exact).then_some(())?;
            Some((
                Address { start: address.start + span.start, end: address.start + span.end, ..address.clone() },
                words,
            ))
        })
        .map_or(Verdict::NotJustified { claimed }, |(at, words)| Verdict::Justified { kind: claimed, at, words })
}

/// A verdict as the interface shows it, beside the sentence: `CitationJudgement` in the contracts. The words that
/// justify a kind are evidence, made from their place in the citing paper.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Judgement {
    Justified { kind: CitationKind, because: Evidence },
    Mention,
    NotJustified { claimed: CitationKind },
    NotClassified,
}

impl Judgement {
    /// `verdict` with its words as evidence of the citing paper (`paper`, read as `passages`). Words that can no
    /// longer be had from the paper's pages, or that the page says otherwise at their place, make it not justified.
    pub fn of(verdict: &Verdict, paper: &str, passages: &Passages) -> Self {
        match verdict {
            Verdict::Justified { kind, at, words } => {
                match Evidence::at(paper, passages, at).filter(|because| because.quote() == words) {
                    Some(because) => Judgement::Justified { kind: *kind, because },
                    None => Judgement::NotJustified { claimed: *kind },
                }
            }
            Verdict::Mention => Judgement::Mention,
            Verdict::NotJustified { claimed } => Judgement::NotJustified { claimed: *claimed },
            Verdict::NotClassified => Judgement::NotClassified,
        }
    }
}

/// How a paper is cited, in counts of citing sentences: `CitationTally` in the contracts. `sentences` is what every
/// other count is out of; the counts add up to it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    pub sentences: usize,
    /// Distinct papers those sentences are in.
    pub papers: usize,
    /// Justified kinds other than mention.
    pub kinds: BTreeMap<CitationKind, usize>,
    /// Read as mentions.
    pub mentions: usize,
    /// Read as another kind without words to show it: counted apart, as mentions.
    pub not_justified: usize,
    pub not_classified: usize,
}

impl Tally {
    pub fn add(&mut self, verdict: &Verdict) {
        self.sentences += 1;
        match verdict {
            Verdict::Justified { kind, .. } => *self.kinds.entry(*kind).or_default() += 1,
            Verdict::Mention => self.mentions += 1,
            Verdict::NotJustified { .. } => self.not_justified += 1,
            Verdict::NotClassified => self.not_classified += 1,
        }
    }

    pub fn of(&self, kind: CitationKind) -> usize {
        self.kinds.get(&kind).copied().unwrap_or(0)
    }

    /// A judgement as it was shown, counted: the same counts as its verdict's, but a kind whose words were not
    /// had from the paper's pages counts as not justified, as it was shown.
    pub fn add_shown(&mut self, judgement: &Judgement) {
        self.sentences += 1;
        match judgement {
            Judgement::Justified { kind, .. } => *self.kinds.entry(*kind).or_default() += 1,
            Judgement::Mention => self.mentions += 1,
            Judgement::NotJustified { .. } => self.not_justified += 1,
            Judgement::NotClassified => self.not_classified += 1,
        }
    }
}

// ------------------------------------------------------------------------------------------------ asking

const SYSTEM: &str = "You read how machine-learning papers cite other papers. Each citation is given as the \
sentence that cites, with the sentence before and after it when there are any, and the cited paper's title and \
abstract so that you know what is cited. Say what each citation does, as one of these kinds:\n\
- uses: the citing paper uses the cited paper's method, model or code, or builds directly on it\n\
- uses_data: it uses the cited paper's dataset or benchmark\n\
- baseline: it compares against the cited paper as a baseline\n\
- extends: it extends or generalises the cited paper's work\n\
- supports: it reports a finding that agrees with or confirms the cited paper's\n\
- contrasts: it reports a different or contradicting result, or criticises the cited paper\n\
- mention: anything else: background, related work, one of a list of examples\n\
Most citations are mentions; choose another kind only when the words say so. For every kind other than mention, \
copy into \"quote\" the words that show it, exactly as they are written in one of the given sentences (at least \
four words, from one sentence only). For a mention, leave \"quote\" empty. Use only the text given. Answer for \
every citation, by its id.";

/// The JSON the answer must be.
pub fn schema() -> Value {
    let kinds: Vec<&str> = CitationKind::ALL.iter().map(|kind| kind.name()).collect();
    json!({
        "type": "object", "additionalProperties": false, "required": ["citations"],
        "properties": {"citations": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["id", "kind", "quote"],
            "properties": {
                "id": {"type": "string"},
                "kind": {"type": "string", "enum": kinds},
                "quote": {"type": "string"}
            }
        }}}
    })
}

/// One citation as the model answered it, before its shape is checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaidCitation {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub quote: String,
}

/// The model's answer for one call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Said {
    pub citations: Vec<SaidCitation>,
}

/// The model's reading of each of `asked` citations of one call, in the order asked (ids "1" to "n"). An id it
/// did not answer, answered twice, or with a kind that is not one of [`CitationKind`], has none.
pub fn read_said(said: &Said, asked: usize) -> Vec<Option<Classified>> {
    let mut seen: HashMap<usize, Option<Classified>> = HashMap::new();
    for citation in &said.citations {
        let Some(at) = citation.id.trim().parse::<usize>().ok().filter(|id| (1..=asked).contains(id)) else {
            continue;
        };
        let reading = CitationKind::named(&citation.kind).map(|kind| Classified {
            kind,
            quote: citation.quote.trim().to_owned(),
            sentence: None,
        });
        // the same id answered twice is not an answer
        seen.entry(at).and_modify(|known| *known = None).or_insert(reading);
    }
    (1..=asked).map(|id| seen.remove(&id).flatten()).collect()
}

fn section_name(section: &Section) -> String {
    match section {
        Section::Abstract => "abstract".to_owned(),
        Section::Introduction => "introduction".to_owned(),
        Section::RelatedWork => "related work".to_owned(),
        Section::Background => "background".to_owned(),
        Section::Method => "method".to_owned(),
        Section::Experiments => "experiments".to_owned(),
        Section::Discussion => "discussion".to_owned(),
        Section::Appendix => "appendix".to_owned(),
        Section::Other { title } => format!("\"{title}\""),
    }
}

/// The question about `contexts`, which all cite the paper `title` (`abstract_text`).
pub fn question(title: &str, abstract_text: &str, contexts: &[&Context]) -> String {
    let abstract_text: String = abstract_text.chars().take(ABSTRACT_CHARS).collect();
    let mut question = format!("The cited paper: {title}\nIts abstract: {abstract_text}\n\nIts citations:\n");
    for (at, context) in contexts.iter().enumerate() {
        let together = match context.grouping {
            Grouping::Alone => String::new(),
            Grouping::Grouped { with } => format!(", together with {with} other works"),
        };
        question.push_str(&format!(
            "\n[id {}] cited as \"{}\"{together}, in the {} section\n",
            at + 1,
            context.written,
            section_name(&context.section)
        ));
        if let Some(before) = &context.before {
            question.push_str(&format!("Before: {}\n", before.sentence));
        }
        question.push_str(&format!("Sentence: {}\n", context.sentence));
        if let Some(after) = &context.after {
            question.push_str(&format!("After: {}\n", after.sentence));
        }
    }
    question
}

/// The key one citing sentence's reading by `chosen` is kept under.
pub fn sentence_key(chosen: &Chosen, citing: &str, cited: &str, address: &Address) -> AnswerKey {
    let place = format!(
        "citing sentence at page {} characters {} to page {} character {}",
        address.page, address.start, address.end_page, address.end
    );
    chosen.key(Feature::CitationKinds, PROMPT, &[citing, cited], &place)
}

/// One citation to be read: who cites, whom, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub citing: String,
    pub cited: String,
    pub context: Context,
}

/// The cited paper as the question names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitedRecord {
    pub title: String,
    pub abstract_text: String,
}

/// One call: citations of one paper.
#[derive(Clone, Debug)]
pub struct Call {
    pub cited: String,
    /// Indices into the plan's cases.
    pub cases: Vec<usize>,
    pub question: String,
    pub key: AnswerKey,
}

/// The calls that read the citations not read yet, before anything is asked: what the estimate is made from and
/// the job asks.
#[derive(Debug)]
pub struct Plan {
    /// The model chosen in the settings, of its provider.
    pub chosen: Chosen,
    pub cases: Vec<Case>,
    /// Each case's own key.
    pub keys: Vec<AnswerKey>,
    pub calls: Vec<Call>,
    /// Cases whose reading is kept already: they are not asked.
    pub kept: usize,
    schema: Value,
}

impl Plan {
    /// The calls for those of `cases` whose reading `is_kept` does not find, grouped by cited paper (in the
    /// order of their first case), [`PER_CALL`] at a time. A case whose cited paper has no record is left out.
    pub fn new(
        chosen: &Chosen,
        cases: Vec<Case>,
        records: &HashMap<String, CitedRecord>,
        is_kept: impl Fn(&AnswerKey, &Case) -> Result<bool, LibraryError>,
    ) -> Result<Self, LibraryError> {
        let cases: Vec<Case> = cases.into_iter().filter(|case| records.contains_key(&case.cited)).collect();
        let keys: Vec<AnswerKey> =
            cases.iter().map(|case| sentence_key(chosen, &case.citing, &case.cited, &case.context.address)).collect();
        let mut by_cited: Vec<(String, Vec<usize>)> = Vec::new();
        let mut seen: HashSet<&AnswerKey> = HashSet::new();
        let mut kept = 0;
        for (index, (case, key)) in cases.iter().zip(&keys).enumerate() {
            if !seen.insert(key) {
                continue; // the same sentence twice: asked once
            }
            if is_kept(key, case)? {
                kept += 1;
                continue;
            }
            match by_cited.iter_mut().find(|(cited, _)| *cited == case.cited) {
                Some((_, indices)) => indices.push(index),
                None => by_cited.push((case.cited.clone(), vec![index])),
            }
        }
        let mut calls = Vec::new();
        for (cited, indices) in by_cited {
            let Some(record) = records.get(&cited) else { continue };
            for chunk in indices.chunks(PER_CALL) {
                let contexts: Vec<&Context> = chunk.iter().map(|&index| &cases[index].context).collect();
                let names: Vec<&str> = chunk.iter().map(|&index| keys[index].as_str()).collect();
                let question = question(&record.title, &record.abstract_text, &contexts);
                // the question with them: an answer kept for other sentences at the same places (a PDF replaced
                // since) is not this call's
                let key =
                    chosen.key(Feature::CitationKinds, PROMPT, &[&cited], &format!("{}\n{question}", names.join("\n")));
                calls.push(Call { question, cited: cited.clone(), cases: chunk.to_vec(), key });
            }
        }
        Ok(Plan { chosen: chosen.clone(), cases, keys, calls, kept, schema: schema() })
    }

    /// Citations to be asked about.
    pub fn to_ask(&self) -> usize {
        self.calls.iter().map(|call| call.cases.len()).sum()
    }

    /// The calls as [`batch`] asks them.
    pub fn items(&self) -> Vec<Item<'_>> {
        self.calls
            .iter()
            .map(|call| {
                let sentences = u32::try_from(call.cases.len()).unwrap_or(u32::MAX);
                Item {
                    slot: Slot {
                        feature: Feature::CitationKinds,
                        key: call.key.clone(),
                        subject: Subject::Paper(call.cited.clone()),
                    },
                    prompt: PROMPT,
                    ask: Ask {
                        purpose: "citation kinds",
                        model: self.chosen.model(),
                        system: SYSTEM,
                        shared: None,
                        question: &call.question,
                        form: Form::Json(&self.schema),
                        effort: self.chosen.effort(),
                        max_tokens: MOST_PER_CALL + MOST_PER_SENTENCE * sentences,
                    },
                    likely_tokens: LIKELY_PER_CALL + LIKELY_PER_SENTENCE * sentences,
                }
            })
            .collect()
    }
}

/// What a job reading citation kinds came to, for its log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Classifying {
    pub calls: usize,
    /// Citations read and kept now (from this job's calls, or a batch kept from an earlier one).
    pub read: usize,
    /// Citations a call was answered for, whose reading was missing or malformed in the answer.
    pub unanswered: usize,
    /// Calls with no usable answer.
    pub failed: usize,
    /// Calls not started (budget, stop, the API unavailable).
    pub not_started: usize,
    /// Citations of the calls that failed or were not started: not asked, or asked with no usable answer. They
    /// stay not classified, and a later run asks them.
    pub not_asked: usize,
}

/// Ask every call of `plan`, within `budget`, and keep each citation's reading under its own key.
pub fn classify(
    asking: Asking<'_>,
    plan: &Plan,
    budget: &Budget,
    tracker: &Tracker<'_>,
) -> Result<Classifying, LibraryError> {
    let library: &Library = asking.library;
    let items = plan.items();
    let outcomes = batch::ask_all::<Said>(asking, &items, budget, tracker)?;
    let mut done = Classifying { calls: plan.calls.len(), ..Classifying::default() };
    for (call, outcome) in plan.calls.iter().zip(outcomes) {
        let (said, model) = match outcome {
            Outcome::Answered(answered) => (answered.answer, answered.model),
            Outcome::Kept(kept) => (kept.answer, kept.model),
            Outcome::Failed(_) => {
                done.failed += 1;
                done.not_asked += call.cases.len();
                continue;
            }
            Outcome::NotStarted(_) => {
                done.not_started += 1;
                done.not_asked += call.cases.len();
                continue;
            }
        };
        for (&index, reading) in call.cases.iter().zip(read_said(&said, call.cases.len())) {
            let Some(reading) = reading else {
                done.unanswered += 1;
                continue;
            };
            let reading = Classified { sentence: Some(plan.cases[index].context.sentence.clone()), ..reading };
            let slot = Slot {
                feature: Feature::CitationKinds,
                key: plan.keys[index].clone(),
                subject: Subject::Paper(call.cited.clone()),
            };
            library.keep(&slot, &model, PROMPT, &reading)?;
            done.read += 1;
        }
    }
    Ok(done)
}

/// The reading kept under `key` for `case`, when it is a reading of that case's sentence (one kept for another
/// sentence at the same place, of a PDF replaced since, is none: it is asked again).
pub fn kept_reading(library: &Library, key: &AnswerKey, case: &Case) -> Result<Option<Classified>, LibraryError> {
    Ok(library
        .kept::<Classified>(Feature::CitationKinds, key)?
        .map(|kept| kept.answer)
        .filter(|reading| reading.sentence.as_ref().is_none_or(|sentence| *sentence == case.context.sentence)))
}

/// The reading kept for each case (by `chosen`), in the cases' order.
pub fn kept_readings(
    library: &Library,
    chosen: &Chosen,
    cases: &[Case],
) -> Result<Vec<Option<Classified>>, LibraryError> {
    cases
        .iter()
        .map(|case| {
            kept_reading(library, &sentence_key(chosen, &case.citing, &case.cited, &case.context.address), case)
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ what they come to

/// The cited papers a sample run reads first: the `most` cited by distinct citing papers among `cases` (then by
/// citing sentences, then by id, so the choice is the same every time).
pub fn most_cited(cases: &[Case], most: usize) -> Vec<String> {
    let mut citers: HashMap<&str, (BTreeSet<&str>, usize)> = HashMap::new();
    for case in cases {
        let entry = citers.entry(case.cited.as_str()).or_default();
        entry.0.insert(&case.citing);
        entry.1 += 1;
    }
    let mut ranked: Vec<(&str, usize, usize)> =
        citers.into_iter().map(|(cited, (papers, sentences))| (cited, papers.len(), sentences)).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(b.0)));
    ranked.into_iter().take(most).map(|(cited, _, _)| cited.to_owned()).collect()
}

/// Each case judged with its kept reading ([`judge`]): on every core, in the cases' order whatever their number.
pub fn verdicts(cases: &[Case], readings: &[Option<Classified>]) -> Vec<Verdict> {
    cases.par_iter().zip(readings.par_iter()).map(|(case, reading)| judge(&case.context, reading.as_ref())).collect()
}

/// Each cited paper's tally over `cases`, with their `verdicts` (by cited paper's id).
pub fn tallies(cases: &[Case], verdicts: &[Verdict]) -> BTreeMap<String, Tally> {
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut papers: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (case, verdict) in cases.iter().zip(verdicts) {
        tallies.entry(case.cited.clone()).or_default().add(verdict);
        papers.entry(&case.cited).or_default().insert(&case.citing);
    }
    for (cited, tally) in &mut tallies {
        tally.papers = papers.get(cited.as_str()).map_or(0, BTreeSet::len);
    }
    tallies
}

/// A cited paper ranked by how often it is cited one way: `RankedCitation` in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ranked {
    pub paper: String,
    /// Sentences read as the kind ranked by.
    pub count: usize,
    pub tally: Tally,
}

/// Papers listed per kind.
pub const RANKED: usize = 8;

/// The cited papers most often read as any of `kinds`, most first (ties by id), those with none left out.
pub fn ranked(tallies: &BTreeMap<String, Tally>, kinds: &[CitationKind]) -> Vec<Ranked> {
    let mut ranked: Vec<Ranked> = tallies
        .iter()
        .map(|(paper, tally)| Ranked {
            paper: paper.clone(),
            count: kinds.iter().map(|kind| tally.of(*kind)).sum(),
            tally: tally.clone(),
        })
        .filter(|ranked| ranked.count > 0)
        .collect();
    ranked.sort_by(|a, b| b.count.cmp(&a.count).then(a.paper.cmp(&b.paper)));
    ranked.truncate(RANKED);
    ranked
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::features::citation_contexts::Neighbour;
    use crate::jobs::{Cancel, Progress};
    use crate::model::batch::estimate_batch;
    use crate::model::language::fake::{Fake, message, ok};
    use crate::model::language::{Endpoint, KEY_VARIABLE, Limit};
    use crate::text::pdf::passages::Marker;
    use crate::usage::{Call as Spent, Usd};

    const MODEL: &str = "claude-opus-5-5";

    fn address(page: u32, start: u32, end: u32) -> Address {
        Address { page, section: "4 Experiments".to_owned(), paragraph: 1, start, end_page: page, end }
    }

    fn context(sentence: &str, before: &str, start: u32) -> Context {
        let length = |text: &str| u32::try_from(text.chars().count()).unwrap();
        let before_start = start - length(before) - 1;
        Context {
            address: address(3, start, start + length(sentence)),
            sentence: sentence.to_owned(),
            before: Some(Neighbour { address: address(3, before_start, start - 1), sentence: before.to_owned() }),
            after: None,
            section: Section::Experiments,
            grouping: Grouping::Alone,
            marker: Marker::Number,
            written: "[4]".to_owned(),
        }
    }

    fn case(citing: &str, cited: &str, sentence: &str, start: u32) -> Case {
        Case {
            citing: citing.to_owned(),
            cited: cited.to_owned(),
            context: context(sentence, "We evaluate on five tasks.", start),
        }
    }

    fn classified(kind: CitationKind, quote: &str) -> Classified {
        Classified { kind, quote: quote.to_owned(), sentence: None }
    }

    #[test]
    fn a_kind_is_justified_only_by_words_found_in_the_sentence_or_its_neighbours() {
        let context =
            context("We compare against TIES merging [4] as our main baseline.", "We evaluate on five tasks.", 100);
        let verdict = judge(&context, Some(&classified(CitationKind::Baseline, "compare against TIES merging")));
        let Verdict::Justified { kind, at, words } = verdict else { panic!("{verdict:?}") };
        assert_eq!((kind, at.start, at.end), (CitationKind::Baseline, 103, 131));
        assert_eq!(words, "compare against TIES merging");
        // in the sentence before
        assert!(matches!(
            judge(&context, Some(&classified(CitationKind::UsesData, "We evaluate on five tasks"))),
            Verdict::Justified { at, .. } if at.start < 100
        ));
        // a fabricated quote, words from the cited paper's abstract, or a quote too short to show anything
        for quote in
            ["TIES merging is the best method ever written", "resolves sign conflicts between tasks", "baseline"]
        {
            assert_eq!(
                judge(&context, Some(&classified(CitationKind::Contrasts, quote))),
                Verdict::NotJustified { claimed: CitationKind::Contrasts },
                "{quote}"
            );
        }
        // close is not enough: the words must be the sentence's own
        assert_eq!(
            judge(&context, Some(&classified(CitationKind::Baseline, "compare against TIES mergng as our"))),
            Verdict::NotJustified { claimed: CitationKind::Baseline }
        );
        assert_eq!(judge(&context, Some(&classified(CitationKind::Mention, ""))), Verdict::Mention);
        assert_eq!(judge(&context, None), Verdict::NotClassified);
    }

    #[test]
    fn a_tally_counts_each_reading_once_and_adds_up_to_its_sentences() {
        let cases = vec![
            case("a", "ties", "We compare against TIES merging [4] as our main baseline.", 100),
            case("b", "ties", "We compare against TIES merging [4] in every table.", 100),
            case("b", "ties", "TIES merging [4] resolves conflicts between tasks.", 300),
            case("c", "ties", "Others merge models too [4].", 100),
        ];
        let readings = vec![
            Some(classified(CitationKind::Baseline, "compare against TIES merging")),
            Some(classified(CitationKind::Baseline, "a quote that is in no sentence at all")),
            Some(classified(CitationKind::Mention, "")),
            None,
        ];
        let tallies = tallies(&cases, &verdicts(&cases, &readings));
        let tally = &tallies["ties"];
        assert_eq!((tally.sentences, tally.papers), (4, 3));
        assert_eq!(
            (tally.of(CitationKind::Baseline), tally.mentions, tally.not_justified, tally.not_classified),
            (1, 1, 1, 1)
        );
        let json = serde_json::to_value(tally).unwrap();
        assert_eq!(json["kinds"], json!({"baseline": 1}));
        let top = ranked(&tallies, &[CitationKind::Baseline]);
        assert_eq!((top[0].paper.as_str(), top[0].count), ("ties", 1));
        assert!(ranked(&tallies, &[CitationKind::Contrasts]).is_empty(), "a paper never contrasted is not listed");
    }

    #[test]
    fn tallies_are_the_same_whatever_the_number_of_threads() {
        let cases: Vec<Case> = (0..400)
            .map(|n| {
                case(
                    &format!("p{}", n % 37),
                    &format!("c{}", n % 11),
                    "We compare against TIES merging [4] here.",
                    100 + n,
                )
            })
            .collect();
        let readings: Vec<Option<Classified>> = (0..400)
            .map(|n| match n % 3 {
                0 => Some(classified(CitationKind::Baseline, "compare against TIES merging")),
                1 => Some(classified(CitationKind::Contrasts, "nothing like this is written")),
                _ => None,
            })
            .collect();
        let on = |threads: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| tallies(&cases, &verdicts(&cases, &readings)))
        };
        let one = on(1);
        assert_eq!(one, on(4));
        assert_eq!(one, on(13));
        assert_eq!(most_cited(&cases, 3), most_cited(&cases.iter().rev().cloned().collect::<Vec<_>>(), 3));
    }

    #[test]
    fn the_answer_is_read_by_id_and_a_malformed_reading_is_that_citations_alone() {
        let said = Said {
            citations: vec![
                SaidCitation { id: "1".into(), kind: "baseline".into(), quote: " compare against TIES ".into() },
                SaidCitation { id: "2".into(), kind: "praises".into(), quote: String::new() },
                SaidCitation { id: "3".into(), kind: "mention".into(), quote: String::new() },
                SaidCitation { id: "3".into(), kind: "uses".into(), quote: "uses it".into() },
                SaidCitation { id: "9".into(), kind: "uses".into(), quote: "out of range".into() },
            ],
        };
        assert_eq!(
            read_said(&said, 4),
            [Some(classified(CitationKind::Baseline, "compare against TIES")), None, None, None]
        );
    }

    #[test]
    fn the_question_names_the_cited_paper_and_gives_each_sentence_with_its_neighbours() {
        let first =
            context("We compare against TIES merging [4] as our main baseline.", "We evaluate on five tasks.", 100);
        let second = Context {
            grouping: Grouping::Grouped { with: 2 },
            written: "[4, 7, 9]".into(),
            after: Some(Neighbour { address: address(3, 400, 420), sentence: "It wins.".into() }),
            ..first.clone()
        };
        let asked = question("TIES-Merging", "We resolve interference when merging models.", &[&first, &second]);
        assert_eq!(
            asked,
            "The cited paper: TIES-Merging\nIts abstract: We resolve interference when merging models.\n\nIts citations:\n\n\
[id 1] cited as \"[4]\", in the experiments section\nBefore: We evaluate on five tasks.\nSentence: We compare against \
TIES merging [4] as our main baseline.\n\n[id 2] cited as \"[4, 7, 9]\", together with 2 other works, in the \
experiments section\nBefore: We evaluate on five tasks.\nSentence: We compare against TIES merging [4] as our main \
baseline.\nAfter: It wins.\n"
        );
        let schema = schema();
        assert_eq!(schema["properties"]["citations"]["items"]["required"], json!(["id", "kind", "quote"]));
        assert_eq!(
            schema["properties"]["citations"]["items"]["properties"]["kind"]["enum"].as_array().unwrap().len(),
            7
        );
        assert!(!SYSTEM.contains("Claude"), "the prompt names no vendor");
    }

    fn records() -> HashMap<String, CitedRecord> {
        ["ties", "arith"]
            .into_iter()
            .map(|id| {
                (
                    id.to_owned(),
                    CitedRecord { title: format!("title of {id}"), abstract_text: format!("abstract of {id}") },
                )
            })
            .collect()
    }

    fn cases() -> Vec<Case> {
        let mut cases: Vec<Case> = (0..30)
            .map(|n| case(&format!("p{n}"), "ties", "We compare against TIES merging [4] as our main baseline.", 100))
            .collect();
        cases.push(case("p0", "arith", "We build on task arithmetic [4] for every model.", 500));
        cases.push(case("p0", "nowhere", "A paper of no record [4].", 700));
        cases
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        (folder, library)
    }

    fn not_kept(library: &Library) -> impl Fn(&AnswerKey, &Case) -> Result<bool, LibraryError> + '_ {
        |key, case| Ok(kept_reading(library, key, case)?.is_some())
    }

    #[test]
    fn citations_are_asked_in_calls_by_cited_paper_and_the_estimate_sends_nothing() {
        let (_folder, library) = library();
        let plan = Plan::new(&Chosen::claude(MODEL), cases(), &records(), not_kept(&library)).unwrap();
        assert_eq!(plan.cases.len(), 31, "the case whose cited paper has no record is left out");
        assert_eq!(
            plan.calls.iter().map(|call| (call.cited.as_str(), call.cases.len())).collect::<Vec<_>>(),
            [("ties", 25), ("ties", 5), ("arith", 1)]
        );
        assert_eq!(plan.to_ask(), 31);
        let items = plan.items();
        assert!(items.iter().all(|item| item.ask.model == MODEL && item.ask.shared.is_none()));
        let estimate = estimate_batch::<Said>(&library, &items, &plan.chosen).unwrap();
        assert_eq!((estimate.items, estimate.to_ask, estimate.cost.calls), (3, 3, 3));
        assert!(estimate.cost.usd.unwrap() > Usd::ZERO && estimate.cost.usd <= estimate.cost.most_usd);
        // another model is another key: nothing kept by one is taken for the other's
        let other = Plan::new(&Chosen::claude("claude-sonnet-5"), cases(), &records(), not_kept(&library)).unwrap();
        assert_ne!(other.keys[0], plan.keys[0]);
    }

    fn reply(citations: Value) -> Result<crate::model::language::Reply, crate::model::language::TransportError> {
        let usage = json!({"input_tokens": 900, "output_tokens": 300});
        Ok(ok(&message(MODEL, &json!({"citations": citations}).to_string(), "end_turn", usage)))
    }

    fn run(library: &Library, transport: &Fake, plan: &Plan) -> Classifying {
        let calls: Mutex<Vec<Spent>> = Mutex::new(Vec::new());
        let cancel = Cancel::new();
        let log = |_: &str| {};
        let progress = |_: Progress| {};
        let tracker = Tracker::new(&log, &progress, &cancel);
        let endpoint =
            Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap();
        let asking = Asking { library, transport, endpoint, recorder: &calls, in_flight: 1 };
        let budget = Budget::new(Limit::Dollars(Usd::from_picodollars(5_000_000_000_000)));
        classify(asking, plan, &budget, &tracker).unwrap()
    }

    #[test]
    fn each_citation_is_kept_under_its_own_key_and_not_asked_again() {
        let (_folder, library) = library();
        let mut ties_answer: Vec<Value> = (1..=25)
            .map(|id| json!({"id": id.to_string(), "kind": "baseline", "quote": "compare against TIES merging"}))
            .collect();
        // a fabricated quote is kept as said, and judged not justified when shown
        ties_answer[1] = json!({"id": "2", "kind": "contrasts", "quote": "TIES merging fails on every task we tried"});
        // one reading missing from the answer
        ties_answer.pop();
        let replies = vec![
            reply(Value::Array(ties_answer)),
            // a malformed answer: not the JSON asked for, a failure of this call only
            Ok(ok(&message(
                MODEL,
                "I think they are all baselines.",
                "end_turn",
                json!({"input_tokens": 10, "output_tokens": 10}),
            ))),
            reply(json!([{"id": "1", "kind": "uses", "quote": "build on task arithmetic"}])),
        ];
        let transport = Fake::new(replies);
        let plan = Plan::new(&Chosen::claude(MODEL), cases(), &records(), not_kept(&library)).unwrap();
        let done = run(&library, &transport, &plan);
        assert_eq!(done, Classifying { calls: 3, read: 25, unanswered: 1, failed: 1, not_started: 0, not_asked: 5 });
        let sent = transport.sent();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0].body["model"], MODEL);
        assert!(
            sent[0].body["messages"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("The cited paper: title of ties")
        );

        let readings = kept_readings(&library, &Chosen::claude(MODEL), &plan.cases).unwrap();
        let verdicts: Vec<Verdict> =
            plan.cases.iter().zip(&readings).map(|(case, reading)| judge(&case.context, reading.as_ref())).collect();
        assert!(matches!(verdicts[0], Verdict::Justified { kind: CitationKind::Baseline, .. }));
        assert_eq!(verdicts[1], Verdict::NotJustified { claimed: CitationKind::Contrasts }, "dropped and counted");
        assert_eq!(verdicts[24], Verdict::NotClassified, "missing from the answer");
        assert!(verdicts[25..30].iter().all(|verdict| *verdict == Verdict::NotClassified), "its call failed");
        assert!(matches!(verdicts[30], Verdict::Justified { kind: CitationKind::Uses, .. }));

        // asked again: only what has no reading, in new calls; what was read is not paid for twice
        let again = Plan::new(&Chosen::claude(MODEL), cases(), &records(), not_kept(&library)).unwrap();
        assert_eq!((again.kept, again.to_ask()), (25, 6));
        assert_eq!(again.calls.iter().map(|call| call.cases.len()).collect::<Vec<_>>(), [6]);
    }

    #[test]
    fn a_batch_kept_from_a_stopped_job_is_split_without_being_asked_again() {
        let (_folder, library) = library();
        let plan = Plan::new(&Chosen::claude(MODEL), cases()[30..31].to_vec(), &records(), not_kept(&library)).unwrap();
        let said = Said {
            citations: vec![SaidCitation {
                id: "1".into(),
                kind: "uses".into(),
                quote: "build on task arithmetic".into(),
            }],
        };
        let items = plan.items();
        library.keep(&items[0].slot, MODEL, PROMPT, &said).unwrap();
        let transport = Fake::new(vec![]);
        let done = run(&library, &transport, &plan);
        assert_eq!((done.read, transport.sent().len()), (1, 0));
        assert!(kept_readings(&library, &Chosen::claude(MODEL), &plan.cases).unwrap()[0].is_some());
    }

    #[test]
    fn a_reading_of_another_sentence_at_the_same_place_is_not_taken_for_this_ones() {
        let (_folder, library) = library();
        let before = cases()[30..31].to_vec();
        let plan = Plan::new(&Chosen::claude(MODEL), before.clone(), &records(), not_kept(&library)).unwrap();
        let transport =
            Fake::new(vec![reply(json!([{"id": "1", "kind": "uses", "quote": "build on task arithmetic"}]))]);
        assert_eq!(run(&library, &transport, &plan).read, 1);
        let kept = kept_readings(&library, &Chosen::claude(MODEL), &before).unwrap();
        assert_eq!(kept[0].as_ref().unwrap().sentence.as_deref(), Some(before[0].context.sentence.as_str()));

        // the PDF replaced: another sentence, of the same length, at the very same place
        let mut after = before.clone();
        after[0].context.sentence = "We doubt the task arithmetic [4] of every model.".to_owned();
        assert_eq!(after[0].context.sentence.chars().count(), before[0].context.sentence.chars().count());
        assert_eq!(kept_readings(&library, &Chosen::claude(MODEL), &after).unwrap(), [None]);
        let again = Plan::new(&Chosen::claude(MODEL), after, &records(), not_kept(&library)).unwrap();
        assert_eq!((again.kept, again.to_ask()), (0, 1), "asked again");
        assert_ne!(again.calls[0].key, plan.calls[0].key, "the batch kept for the old sentence is not this call's");
        // a reading kept before readings were kept with their sentence still counts
        let old = Classified { kind: CitationKind::Mention, quote: String::new(), sentence: None };
        library
            .keep(
                &Slot {
                    feature: Feature::CitationKinds,
                    key: plan.keys[0].clone(),
                    subject: Subject::Paper("arith".into()),
                },
                MODEL,
                PROMPT,
                &old,
            )
            .unwrap();
        assert_eq!(kept_readings(&library, &Chosen::claude(MODEL), &before).unwrap(), [Some(old)]);
    }

    #[test]
    fn a_kind_is_shown_as_justified_only_when_the_page_says_its_words_at_their_place() {
        let pages = vec!["We compare against TIES merging [4] as our main baseline.".to_owned()];
        let passages = crate::text::pdf::passages::read_passages(&pages);
        let mut context =
            context("We compare against TIES merging [4] as our main baseline.", "We evaluate on five tasks.", 100);
        context.before = None;
        context.address = Address { page: 1, end_page: 1, start: 0, end: 57, ..context.address };
        let verdict = judge(&context, Some(&classified(CitationKind::Baseline, "compare against TIES merging")));
        let Judgement::Justified { because, .. } = Judgement::of(&verdict, "seed", &passages) else { panic!() };
        assert_eq!(because.quote(), "compare against TIES merging");
        // the page says other words at that place than the sentence kept: not shown as justified
        let Verdict::Justified { kind, at, .. } = verdict else { panic!() };
        let other = Verdict::Justified { kind, at, words: "compare against DARE merging".to_owned() };
        assert_eq!(Judgement::of(&other, "seed", &passages), Judgement::NotJustified { claimed: kind });
    }

    #[test]
    fn the_sample_is_the_most_cited_by_distinct_papers() {
        let mut all = cases();
        all.push(case("p1", "arith", "We build on task arithmetic [4] again.", 900));
        assert_eq!(most_cited(&all, 1), ["ties"]);
        assert_eq!(most_cited(&all, 5), ["ties", "arith", "nowhere"]);
    }
}
