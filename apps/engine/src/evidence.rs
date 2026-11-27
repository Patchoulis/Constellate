//! Evidence: "this statement rests on that place in that paper", in one type that every feature shows its
//! statements with, so that whatever is shown can be clicked back to the words in the original paper.
//!
//! The rule it exists for: **no quote is shown as evidence unless it was found in the paper it is attributed
//! to.** A language model asked to support what it says with quotes will now and then write a quote no paper
//! contains, paraphrase one, or give the right words the wrong paper. So what a model returns is never evidence by
//! itself. It is a [`Claim`]: a statement with the quotes it says support it. [`check`] looks for each quote in the
//! text of the paper it names ([`crate::text::passages`]), keeps the statements whose quotes were found, with the
//! place and the paper's own wording ([`Evidence`]), and drops or counts the rest.
//!
//! [`Evidence`] has no variant for "not checked", and its fields can only be set here: by [`check`], by
//! [`Evidence::locate`], or by [`Evidence::at`] for a place read from the paper itself (a citing sentence). So a
//! fabricated quote, or a real one attributed to the wrong paper, cannot become something the interface shows.
//!
//! How each piece of evidence was established is one of:
//!
//! | [`Established`] | the quote was found in                                           |
//! |-----------------|------------------------------------------------------------------|
//! | `Verified`      | the paper's own text, read from its PDF: it has a page to open at |
//! | `Abstract`      | the paper's title or abstract only, since its PDF has not been read |
//!
//! This is `_checked` and `MIN_CITED` of the Python overview (`apps/backend/rp/overview.py`) made stricter: that
//! one checked only that a cited paper was one of the community's; here the words themselves must be in it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::text::passages::{Address, Located, Match, Passages, Span, chars_of, letters_in, locate_in};

/// A quote with fewer letters and digits than this proves nothing ("the model", "we show"), and is not evidence.
pub const SHORTEST_QUOTE: usize = 20;

/// Which part of a paper's record a quote was found in, when only the record has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Title,
    Abstract,
}

/// How a piece of evidence was established. There is deliberately no variant for a quote that was not found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Established {
    /// Found in the paper's text as read from its PDF, at `address`.
    Verified { address: Address, matched: Match },
    /// Found in its title or abstract, at `start..end` (characters) of that field: its PDF has not been read.
    Abstract { field: Field, start: u32, end: u32, matched: Match },
}

/// One piece of evidence: a place in a paper, and the paper's own words there. `Evidence` in the contracts.
///
/// It is serialised for the interface but never read back from JSON: that would make evidence nothing checked.
/// A feature that keeps its answers keeps the model's [`Claim`]s and checks them again when it loads them
/// (cheap: the paper's passages are kept).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Evidence {
    paper: String,
    /// The paper's own wording, which may differ from the model's quote by spacing, hyphenation, punctuation and
    /// (when [`Match::Close`]) a few letters: what is shown is always this.
    quote: String,
    established: Established,
}

impl Evidence {
    pub fn paper(&self) -> &str {
        &self.paper
    }

    pub fn quote(&self) -> &str {
        &self.quote
    }

    pub fn established(&self) -> &Established {
        &self.established
    }

    /// Where it is in the paper's text, when the paper was read (not for evidence from an abstract).
    pub fn address(&self) -> Option<&Address> {
        match &self.established {
            Established::Verified { address, .. } => Some(address),
            Established::Abstract { .. } => None,
        }
    }

    /// `quote` looked for in `paper`'s text; the evidence when it is there, why not when it is not.
    pub fn locate(paper: &str, text: &PaperText, quote: &str) -> Result<Evidence, Rejection> {
        if letters_in(quote) < SHORTEST_QUOTE {
            return Err(Rejection::TooShort);
        }
        match text {
            PaperText::Read(passages) => passages.locate(quote).map(|found| Self::verified(paper, found)),
            PaperText::Unread { title, abstract_text } => {
                let field = |field: Field, text: &str| {
                    locate_in(text, quote)
                        .map(|(span, quote, matched)| Self::from_abstract(paper, field, span, quote, matched))
                };
                field(Field::Abstract, abstract_text).or_else(|| field(Field::Title, title))
            }
        }
        .ok_or(Rejection::NotInPaper)
    }

    /// The evidence at a place in a paper that was read: its words are read from the page, so it is verified by
    /// construction. `None` when the place is not in the paper's pages (an address from another reading).
    pub fn at(paper: &str, passages: &Passages, address: &Address) -> Option<Evidence> {
        let pages = |number: u32| passages.pages.get(usize::try_from(number).ok()?.checked_sub(1)?);
        let length = |text: &str| u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
        let first = pages(address.page)?;
        let quote = if address.end_page == address.page {
            (address.start < address.end && address.end <= length(&first.text))
                .then(|| chars_of(&first.text, address.start, address.end))?
        } else {
            // only on to the very next page, and inside both pages' text
            let last = pages(address.end_page).filter(|_| address.end_page == address.page + 1)?;
            let inside = address.start < length(&first.text) && address.end > 0 && address.end <= length(&last.text);
            inside.then(|| {
                format!("{} {}", chars_of(&first.text, address.start, u32::MAX), chars_of(&last.text, 0, address.end))
            })?
        };
        let address = address.clone();
        (!quote.trim().is_empty()).then(|| Evidence {
            paper: paper.to_owned(),
            quote,
            established: Established::Verified { address, matched: Match::Exact },
        })
    }

    fn verified(paper: &str, found: Located) -> Evidence {
        Evidence {
            paper: paper.to_owned(),
            quote: found.wording,
            established: Established::Verified { address: found.address, matched: found.matched },
        }
    }

    fn from_abstract(paper: &str, field: Field, span: Span, quote: String, matched: Match) -> Evidence {
        Evidence {
            paper: paper.to_owned(),
            quote,
            established: Established::Abstract { field, start: span.start, end: span.end, matched },
        }
    }
}

/// What a paper's quotes are checked against: its text when its PDF was read, its record when not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaperText {
    Read(Passages),
    Unread { title: String, abstract_text: String },
}

/// The papers a feature may cite, by id: a community's members, a run's papers, the one paper a question is
/// about. A quote attributed to any other paper is refused, whatever it says.
pub type Papers = HashMap<String, PaperText>;

/// A quote a model gave, and the paper it says the quote is from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub paper: String,
    pub quote: String,
}

/// A statement as a model returned it, with the quotes it says support it. `T` is the statement: a sentence, a
/// table cell, an answer, whatever the feature asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim<T> {
    pub statement: T,
    pub quotes: Vec<Quote>,
}

/// A statement with the evidence it rests on, every piece of which was found: what a feature shows. `Cited<T>`
/// in the contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Cited<T> {
    pub statement: T,
    pub evidence: Vec<Evidence>,
}

/// Why a quote is not evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    #[error("the quote names a paper that is not one of those it may cite")]
    UnknownPaper,
    #[error("the quote is too short to show anything")]
    TooShort,
    #[error("the quote is not in the paper it is attributed to")]
    NotInPaper,
}

/// A quote that was not evidence, and why: kept for the log and the counts, never shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub quote: Quote,
    pub why: Rejection,
}

/// What a statement needs to be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Distinct papers with found quotes: 1 for a statement about one paper, more for "a pattern of the community"
    /// (the overview's `MIN_CITED` was 3).
    pub papers: usize,
}

impl Rules {
    /// A statement shown when at least one of its quotes was found.
    pub const ONE_PAPER: Self = Self { papers: 1 };

    /// A statement about what several papers share, shown only when quotes from `papers` of them were found.
    pub const fn pattern(papers: usize) -> Self {
        Self { papers }
    }
}

/// A statement that is not shown, with what its quotes were found to be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dropped<T> {
    pub statement: T,
    /// Distinct papers whose quotes were found, fewer than the rules ask for.
    pub papers_found: usize,
    pub rejected: Vec<Rejected>,
}

/// How a check went, in numbers: for the log, and for a feature that wants to say "3 statements were left out
/// because their quotes could not be found".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub statements: usize,
    pub kept: usize,
    pub dropped: usize,
    pub quotes: usize,
    pub verified: usize,
    pub from_abstract: usize,
    pub unknown_paper: usize,
    pub too_short: usize,
    pub not_in_paper: usize,
}

/// What [`check`] returns: the statements to show, those not to, and the counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Checked<T> {
    pub cited: Vec<Cited<T>>,
    pub dropped: Vec<Dropped<T>>,
    /// The quotes of kept statements that were not found: never shown, counted.
    pub rejected: Vec<Rejected>,
    pub counts: Counts,
}

/// Check what a model returned against the papers' texts. A statement is kept with the evidence that was found
/// when quotes from at least `rules.papers` distinct papers were found; the quotes of a kept statement that were
/// not found are left out of its evidence (and listed in `rejected`); a statement with too few is dropped. The
/// same quote of the same paper given twice counts once. Statements keep their order.
pub fn check<T>(claims: Vec<Claim<T>>, papers: &Papers, rules: Rules) -> Checked<T> {
    let mut counts = Counts { statements: claims.len(), ..Counts::default() };
    // each paper's quotes are looked for together, so a paper's text is prepared once
    let mut by_paper: HashMap<&str, Vec<&str>> = HashMap::new();
    for claim in &claims {
        for quote in &claim.quotes {
            by_paper.entry(quote.paper.as_str()).or_default().push(quote.quote.as_str());
        }
    }
    let mut found: HashMap<(String, String), Result<Evidence, Rejection>> = HashMap::new();
    for (paper, quotes) in by_paper {
        let text = papers.get(paper);
        let outcomes: Vec<Result<Evidence, Rejection>> = match text {
            None => quotes.iter().map(|_| Err(Rejection::UnknownPaper)).collect(),
            Some(PaperText::Read(passages)) => {
                let long: Vec<&str> =
                    quotes.iter().copied().filter(|quote| letters_in(quote) >= SHORTEST_QUOTE).collect();
                let mut located = passages.locate_all(&long).into_iter();
                quotes
                    .iter()
                    .map(|quote| {
                        if letters_in(quote) < SHORTEST_QUOTE {
                            return Err(Rejection::TooShort);
                        }
                        located
                            .next()
                            .flatten()
                            .map(|found| Evidence::verified(paper, found))
                            .ok_or(Rejection::NotInPaper)
                    })
                    .collect()
            }
            Some(unread) => quotes.iter().map(|quote| Evidence::locate(paper, unread, quote)).collect(),
        };
        for (quote, outcome) in quotes.into_iter().zip(outcomes) {
            found.insert((paper.to_owned(), quote.to_owned()), outcome);
        }
    }

    let mut cited = Vec::new();
    let mut dropped = Vec::new();
    let mut rejected_of_kept = Vec::new();
    for claim in claims {
        let mut evidence: Vec<Evidence> = Vec::new();
        let mut rejected: Vec<Rejected> = Vec::new();
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for quote in &claim.quotes {
            if seen.contains(&(quote.paper.as_str(), quote.quote.as_str())) {
                continue;
            }
            seen.push((quote.paper.as_str(), quote.quote.as_str()));
            counts.quotes += 1;
            match found.get(&(quote.paper.clone(), quote.quote.clone())) {
                Some(Ok(found)) => {
                    match found.established {
                        Established::Verified { .. } => counts.verified += 1,
                        Established::Abstract { .. } => counts.from_abstract += 1,
                    }
                    // two quotes found at the same place are one piece of evidence
                    if !evidence
                        .iter()
                        .any(|known| known.paper == found.paper && known.established == found.established)
                    {
                        evidence.push(found.clone());
                    }
                }
                Some(Err(why)) => {
                    match why {
                        Rejection::UnknownPaper => counts.unknown_paper += 1,
                        Rejection::TooShort => counts.too_short += 1,
                        Rejection::NotInPaper => counts.not_in_paper += 1,
                    }
                    rejected.push(Rejected { quote: quote.clone(), why: *why });
                }
                None => {}
            }
        }
        let mut distinct: Vec<&str> = evidence.iter().map(Evidence::paper).collect();
        distinct.sort_unstable();
        distinct.dedup();
        let papers_found = distinct.len();
        if papers_found >= rules.papers.max(1) {
            counts.kept += 1;
            rejected_of_kept.extend(rejected);
            cited.push(Cited { statement: claim.statement, evidence });
        } else {
            counts.dropped += 1;
            dropped.push(Dropped { statement: claim.statement, papers_found, rejected });
        }
    }
    Checked { cited, dropped, rejected: rejected_of_kept, counts }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::text::passages::read_passages;

    fn read(text: &str) -> PaperText {
        PaperText::Read(read_passages(&[text.to_owned()]))
    }

    fn papers() -> Papers {
        let mut papers = Papers::new();
        papers.insert(
            "2501.00001".to_owned(),
            read("1. Introduction\n\nWe show that merging task vectors counts shared directions twice. The cure is\nto rescale the inflated singular values.\n"),
        );
        papers.insert(
            "2501.00002".to_owned(),
            read(
                "1. Method\n\nOur method distils a large teacher into a small student with no labelled data at all.\n",
            ),
        );
        papers.insert(
            "icml-2026-1".to_owned(),
            PaperText::Unread {
                title: "Calibrated Merging of Task Vectors".to_owned(),
                abstract_text: "Merging fine-tuned models inflates the spectrum of shared directions.".to_owned(),
            },
        );
        papers
    }

    fn quote(paper: &str, quote: &str) -> Quote {
        Quote { paper: paper.to_owned(), quote: quote.to_owned() }
    }

    fn claim(statement: &str, quotes: Vec<Quote>) -> Claim<String> {
        Claim { statement: statement.to_owned(), quotes }
    }

    #[test]
    fn a_quote_found_in_its_paper_becomes_evidence_with_the_paper_s_own_wording() {
        let checked = check(
            vec![claim(
                "Merging counts shared directions twice.",
                vec![quote("2501.00001", "merging task vectors counts shared directions twice")],
            )],
            &papers(),
            Rules::ONE_PAPER,
        );
        assert_eq!(checked.counts.kept, 1);
        let evidence = &checked.cited[0].evidence[0];
        assert_eq!(evidence.paper(), "2501.00001");
        assert_eq!(evidence.quote(), "merging task vectors counts shared directions twice");
        let address = evidence.address().unwrap();
        assert_eq!((address.page, address.section.as_str()), (1, "1. Introduction"));
        assert!(matches!(evidence.established(), Established::Verified { matched: Match::Exact, .. }));
    }

    #[test]
    fn a_fabricated_quote_is_not_evidence_and_its_statement_is_dropped() {
        let checked = check(
            vec![claim(
                "Merging always helps.",
                vec![quote("2501.00001", "merging always improves every task by ten points")],
            )],
            &papers(),
            Rules::ONE_PAPER,
        );
        assert!(checked.cited.is_empty());
        assert_eq!(checked.dropped[0].rejected[0].why, Rejection::NotInPaper);
        assert_eq!((checked.counts.dropped, checked.counts.not_in_paper), (1, 1));
    }

    #[test]
    fn a_real_quote_attributed_to_the_wrong_paper_is_not_evidence() {
        let checked = check(
            vec![claim(
                "Distillation needs no labels.",
                vec![quote("2501.00001", "distils a large teacher into a small student")],
            )],
            &papers(),
            Rules::ONE_PAPER,
        );
        assert!(checked.cited.is_empty(), "the words are in 2501.00002, not in the paper named");
    }

    #[test]
    fn a_quote_that_leaves_out_the_paper_s_not_is_not_evidence() {
        let says =
            "1. Results\n\nCalibrating the merged spectrum does not improve the accuracy of any task we tried.\n";
        let papers = Papers::from([("2501.00003".to_owned(), read(says))]);
        let claim = |quote_text: &str| vec![claim("Calibration helps.", vec![quote("2501.00003", quote_text)])];
        let turned = check(
            claim("Calibrating the merged spectrum does improve the accuracy of any task we tried"),
            &papers,
            Rules::ONE_PAPER,
        );
        assert!(turned.cited.is_empty(), "the paper says the opposite");
        assert_eq!(turned.counts.not_in_paper, 1);
        let slipped = check(
            claim("Calibrating the merged spectrum does not improve the acuracy of any task we tried"),
            &papers,
            Rules::ONE_PAPER,
        );
        assert_eq!(
            slipped.cited[0].evidence[0].quote(),
            "Calibrating the merged spectrum does not improve the accuracy of any task we tried"
        );
    }

    #[test]
    fn a_quote_of_a_paper_that_may_not_be_cited_is_refused() {
        let checked = check(
            vec![claim("Something.", vec![quote("9999.99999", "merging task vectors counts shared directions twice")])],
            &papers(),
            Rules::ONE_PAPER,
        );
        assert_eq!(checked.dropped[0].rejected[0].why, Rejection::UnknownPaper);
        assert_eq!(checked.counts.unknown_paper, 1);
    }

    #[test]
    fn a_kept_statement_shows_only_its_found_quotes_and_the_rest_are_counted() {
        let checked = check(
            vec![claim(
                "Merging inflates shared directions.",
                vec![
                    quote("2501.00001", "counts shared directions twice"),
                    quote("2501.00001", "and this was proven for every architecture"),
                    quote("2501.00001", "counts shared directions twice"),
                    quote("2501.00002", "model"),
                ],
            )],
            &papers(),
            Rules::ONE_PAPER,
        );
        assert_eq!(checked.cited[0].evidence.len(), 1);
        assert_eq!(checked.rejected.len(), 2);
        assert_eq!(
            (checked.counts.quotes, checked.counts.verified, checked.counts.not_in_paper, checked.counts.too_short),
            (3, 1, 1, 1)
        );
    }

    #[test]
    fn a_pattern_needs_found_quotes_from_enough_distinct_papers() {
        let claims = || {
            vec![
                claim(
                    "Shared directions are inflated.",
                    vec![
                        quote("2501.00001", "counts shared directions twice"),
                        quote("icml-2026-1", "inflates the spectrum of shared directions"),
                    ],
                ),
                claim("Only one paper says this.", vec![quote("2501.00002", "with no labelled data at all")]),
            ]
        };
        let checked = check(claims(), &papers(), Rules::pattern(2));
        assert_eq!(checked.cited.len(), 1);
        assert_eq!(checked.cited[0].statement, "Shared directions are inflated.");
        assert_eq!(checked.dropped[0].papers_found, 1);
        assert_eq!(check(claims(), &papers(), Rules::ONE_PAPER).cited.len(), 2);
    }

    #[test]
    fn a_paper_whose_pdf_was_not_read_is_checked_against_its_abstract_and_says_so() {
        let checked = check(
            vec![claim(
                "Spectra inflate.",
                vec![quote("icml-2026-1", "Merging fine-tuned models inflates the spectrum")],
            )],
            &papers(),
            Rules::ONE_PAPER,
        );
        let evidence = &checked.cited[0].evidence[0];
        assert!(evidence.address().is_none());
        assert_eq!(
            evidence.established(),
            &Established::Abstract { field: Field::Abstract, start: 0, end: 47, matched: Match::Exact }
        );
        assert_eq!(checked.counts.from_abstract, 1);
        let title =
            Evidence::locate("icml-2026-1", &papers()["icml-2026-1"], "Calibrated merging of task vectors").unwrap();
        assert!(matches!(title.established(), Established::Abstract { field: Field::Title, .. }));
    }

    #[test]
    fn evidence_at_a_place_reads_the_paper_s_words_there_and_refuses_a_place_outside_it() {
        let PaperText::Read(passages) = &papers()["2501.00001"] else { panic!() };
        let sentence = &passages.passages[1];
        let span = sentence.sentences[0];
        let address = Address { start: span.start, end: span.end, ..sentence.address.clone() };
        let evidence = Evidence::at("2501.00001", passages, &address).unwrap();
        assert_eq!(evidence.quote(), "We show that merging task vectors counts shared directions twice.");
        let outside = Address { page: 9, ..address.clone() };
        assert_eq!(Evidence::at("2501.00001", passages, &outside), None);
        let past = Address { end: 100_000, ..address.clone() };
        assert_eq!(Evidence::at("2501.00001", passages, &past), None);
        let backwards = Address { end_page: 0, ..address.clone() };
        assert_eq!(Evidence::at("2501.00001", passages, &backwards), None);
        let beyond = Address { end_page: 2, ..address };
        assert_eq!(Evidence::at("2501.00001", passages, &beyond), None, "there is no page 2");
    }

    #[test]
    fn evidence_is_told_to_the_interface_with_its_place_and_how_it_was_found() {
        let checked = check(
            vec![claim("x", vec![quote("2501.00001", "rescale the inflated singular values")])],
            &papers(),
            Rules::ONE_PAPER,
        );
        let told = serde_json::to_value(&checked.cited[0]).unwrap();
        assert_eq!(told["statement"], "x");
        assert_eq!(told["evidence"][0]["paper"], "2501.00001");
        assert_eq!(told["evidence"][0]["established"]["kind"], "verified");
        assert_eq!(told["evidence"][0]["established"]["matched"]["kind"], "exact");
        assert_eq!(told["evidence"][0]["established"]["address"]["page"], 1);
    }

    /// The rule does not depend on which model answered: a quote is evidence only once found in its paper, from a
    /// small model on this computer as from the Claude API.
    #[test]
    fn a_fabricated_quote_from_any_provider_s_model_is_dropped_and_a_found_one_kept() {
        use serde_json::json;

        use crate::model::language::fake::{Fake, completion, endpoint, generated, message, ok, response};
        use crate::model::language::{Ask, Budget, Client, Effort, Form, Limit, Provider};

        #[derive(Deserialize)]
        struct Said {
            claims: Vec<Claim<String>>,
        }
        let schema = json!({"type": "object", "properties": {"claims": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["statement", "quotes"],
            "properties": {"statement": {"type": "string"}, "quotes": {"type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["paper", "quote"],
                "properties": {"paper": {"type": "string"}, "quote": {"type": "string"}}
            }}}
        }}}, "required": ["claims"], "additionalProperties": false});
        let said = json!({"claims": [
            {"statement": "Merging always helps.",
             "quotes": [{"paper": "2501.00001", "quote": "merging always improves every task by ten points"}]},
            {"statement": "The cure is rescaling.",
             "quotes": [{"paper": "2501.00001", "quote": "rescale the inflated singular values"}]}
        ]})
        .to_string();
        for provider in Provider::ALL {
            let model = provider.about().models.first().map_or("qwen3:32b", |model| model.id);
            let answer = match provider {
                Provider::Anthropic => {
                    message(model, &said, "end_turn", json!({"input_tokens": 9, "output_tokens": 9}))
                }
                Provider::OpenAi | Provider::Xai => {
                    response(model, &said, json!({"input_tokens": 9, "output_tokens": 9}))
                }
                Provider::Gemini => generated(model, &said, json!({"promptTokenCount": 9, "candidatesTokenCount": 9})),
                _ => completion(model, &said, json!({"prompt_tokens": 9, "completion_tokens": 9})),
            };
            let fake = Fake::new(vec![Ok(ok(&answer))]);
            let calls = std::sync::Mutex::new(Vec::new());
            let client = Client::new(&fake, endpoint(provider), &calls);
            let ask = Ask {
                purpose: "questions about a paper",
                model,
                system: "Quote the paper.",
                shared: None,
                question: "What does it claim?",
                form: Form::Json(&schema),
                effort: Effort::Medium,
                max_tokens: 2_000,
            };
            let answered = client.ask_json::<Said>(&ask, &Budget::new(Limit::Tokens(1_000_000))).unwrap();
            let checked = check(answered.answer.claims, &papers(), Rules::ONE_PAPER);
            assert_eq!(checked.cited.len(), 1, "{provider:?}");
            assert_eq!(checked.cited[0].statement, "The cure is rescaling.", "{provider:?}");
            assert_eq!(checked.dropped[0].rejected[0].why, Rejection::NotInPaper, "{provider:?}");
        }
    }
}
