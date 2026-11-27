//! What a paper's authors declare in its text, whatever its venue.
//!
//! Declarations come in two forms, and a paper is read for both:
//!
//! - **a checklist**: NeurIPS papers end with the "NeurIPS Paper Checklist", numbered questions answered
//!   `[Yes]` / `[No]` / `[NA]` with a justification. Among them: are limitations discussed, are code and data
//!   open, were human subjects involved, were language models an important part of the method.
//! - **statements**: other venues ask for free-text sections instead (an "Impact Statement" at ICML, an "Ethics
//!   Statement" or "Reproducibility Statement" at ICLR, a section on the use of language models), or authors add
//!   them unasked. These are found by their headings.
//!
//! From both come a few [`Flag`]s, which are what the interface shows and what a run can be filtered by. A flag is
//! the authors' own statement, so it is as reliable as they are, and the absence of a flag is not a statement:
//! NeurIPS asks for no declaration when a model was only used for writing, while a free-text statement elsewhere
//! often declares exactly that.
//!
//! The text comes out of a PDF with broken lines, page numbers, ligatures and stray spaces, so the reading is
//! tolerant and, where it cannot be sure, says nothing rather than guess: a checklist answer that is not clearly
//! Yes / No / NA is left out.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use super::{char_len_at, first_chars, pattern, squash};

/// The version of this reader. Raise it when the reading changes: what an older version read is read again.
pub const PARSER: u32 = 2;

/// Where the checklist starts; the last mention counts, since the paper may name it earlier in passing.
const CHECKLIST: &str = "Paper Checklist";
/// An item's title runs into its question within this many characters, or the line is not an item.
const TITLE_REACH: usize = 320;
const MOST_ITEMS: u32 = 30;
const JUSTIFICATION_LENGTH: usize = 600;
/// How far after its heading a statement is looked for, and how much of it is kept.
const STATEMENT_REACH: usize = 2500;
const STATEMENT_LENGTH: usize = 1500;
/// Fewer characters than this under a heading is a table-of-contents line, not a statement.
const SHORTEST_STATEMENT: usize = 40;
/// A numbered section on language models before this share of the paper is its subject, not a declaration.
const SUBJECT_SHARE: f64 = 0.6;
/// How much of a statement on language models is judged for whether it declares their use.
const JUDGED_LENGTH: usize = 700;

/// How an item of the checklist was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Answer {
    Yes,
    No,
    #[serde(rename = "NA")]
    NotApplicable,
}

impl Answer {
    /// The answer as written, whichever way: "[Y es]" (kerned), "[N/A]", "No". "[TODO]" and "[Yes/No]" are none.
    fn read(written: &str) -> Option<Self> {
        let letters: String = written.to_lowercase().chars().filter(char::is_ascii_lowercase).collect();
        match letters.as_str() {
            "yes" => Some(Self::Yes),
            "no" => Some(Self::No),
            "na" => Some(Self::NotApplicable),
            _ => None,
        }
    }
}

/// What a statement is about, whatever the venue calls its heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatementKind {
    /// The use of language models.
    Llm,
    Impact,
    Ethics,
    Reproducibility,
    Limitations,
}

impl StatementKind {
    /// In the order the headings are tried.
    pub const ALL: [Self; 5] = [Self::Llm, Self::Impact, Self::Ethics, Self::Reproducibility, Self::Limitations];

    /// The name of its group in the heading pattern (and in the JSON).
    fn group(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Impact => "impact",
            Self::Ethics => "ethics",
            Self::Reproducibility => "reproducibility",
            Self::Limitations => "limitations",
        }
    }

    /// How its heading reads, in any case.
    fn heading(self) -> &'static str {
        match self {
            Self::Llm => {
                r"(?:Declaration (?:of|on) |Statement (?:of|on) (?:the )?|The |Disclosure of )?(?:Use|Usage) of (?:Large Language Models|LLMs?|Generative AI|AI(?: Tools| Assistants?)?)(?: \(LLMs?\))?(?: Statement)?|LLM (?:Usage|Use)(?: Statement| Declaration| Disclosure)?|AI (?:Usage|Assistance|Use) (?:Statement|Disclosure|Declaration)"
            }
            Self::Impact => r"(?:Broader )?Impact Statements?|Broader Impacts?|Societal Impacts?",
            Self::Ethics => r"Ethics Statement|Ethical (?:Considerations|Statement)",
            Self::Reproducibility => r"Reproducibility Statement",
            Self::Limitations => r"Limitations(?: and (?:Future Work|Broader Impacts?|Societal Impacts?|Discussion))?",
        }
    }
}

/// What a paper's declarations say about it, as shown with the paper and as a run can be filtered by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    LlmDeclared,
    ClosedCode,
    NoLimitations,
    HumanSubjects,
}

impl Flag {
    /// In the order they are shown.
    pub const ALL: [Self; 4] = [Self::LlmDeclared, Self::ClosedCode, Self::NoLimitations, Self::HumanSubjects];

    /// Its id, as stored and as settings name it.
    pub fn id(self) -> &'static str {
        match self {
            Self::LlmDeclared => "llm_declared",
            Self::ClosedCode => "closed_code",
            Self::NoLimitations => "no_limitations",
            Self::HumanSubjects => "human_subjects",
        }
    }

    /// What it says about a paper, as shown.
    pub fn label(self) -> &'static str {
        match self {
            Self::LlmDeclared => "Authors declare the use of language models",
            Self::ClosedCode => "Code and data not openly available",
            Self::NoLimitations => "No limitations discussed",
            Self::HumanSubjects => "Involves human subjects or crowdsourcing",
        }
    }

    /// What raises it, for whoever wonders what the tag on a paper means.
    pub fn note(self) -> &'static str {
        match self {
            Self::LlmDeclared => {
                "The paper's checklist answers Yes to language models being an important part of the method, or the paper has a statement saying they were used (which, outside NeurIPS, often means for writing)."
            }
            Self::ClosedCode => "The paper's checklist answers No to providing open access to its data and code.",
            Self::NoLimitations => "The paper's checklist answers No to discussing the limitations of the work.",
            Self::HumanSubjects => {
                "The paper's checklist answers Yes to crowdsourcing or research with human subjects."
            }
        }
    }

    /// Does this checklist item raise it? An item is known by its title, since its number changes between years
    /// and tracks.
    fn raised_by(self, item: &ChecklistItem) -> bool {
        let (about, answer): (&Regex, Answer) = match self {
            Self::LlmDeclared => (&ABOUT_LLMS, Answer::Yes),
            Self::ClosedCode => (&ABOUT_OPEN_ACCESS, Answer::No),
            Self::NoLimitations => (&ABOUT_LIMITATIONS, Answer::No),
            Self::HumanSubjects => (&ABOUT_CROWDSOURCING, Answer::Yes),
        };
        item.answer == answer && about.is_match(&item.title)
    }
}

/// One answered item of the checklist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub number: u32,
    pub title: String,
    pub answer: Answer,
    pub justification: String,
}

/// A statement found under its heading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: StatementKind,
    /// The heading as the paper writes it.
    pub title: String,
    pub text: String,
}

/// What the authors declare in a paper: `Declarations` in the contracts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declarations {
    pub checklist: Vec<ChecklistItem>,
    pub statements: Vec<Statement>,
    pub flags: Vec<Flag>,
}

static ABOUT_LLMS: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\bLLM|language model"));
static ABOUT_OPEN_ACCESS: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)open access"));
static ABOUT_LIMITATIONS: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^limitations"));
static ABOUT_CROWDSOURCING: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)^crowdsourcing"));

/// "N. Title": the capital letter is matched too, and is where the title starts.
static ITEM_START: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?m)^[ \t]*([0-9]{1,2})\.[ \t]+[A-Z]"));
static ANSWER: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)Answer:\s*(?:\[([^\]\n]{1,10})\]|(Yes|No|N\s?/?\s?A)\b)"));
static JUSTIFICATION_END: LazyLock<Regex> = LazyLock::new(|| pattern(r"\n\s*Guidelines:"));
static PAGE_NUMBER: LazyLock<Regex> = LazyLock::new(|| pattern(r"\A\s*\d{1,3}\s*\z"));

/// A statement's heading, with a section number or letter in front or not, and maybe its first words after a
/// full stop or colon on the same line.
fn heading_pattern() -> String {
    let kinds: Vec<String> =
        StatementKind::ALL.iter().map(|kind| format!("(?P<{}>{})", kind.group(), kind.heading())).collect();
    format!(
        r"^[ \t]*(?P<prefix>(?:[A-Z]|\d{{1,2}})?(?:\.\d{{1,2}})*\.?)[ \t]*(?P<title>{})[ \t]*(?:$|[.:][ \t]*(?P<same_line>[^\n]*)$)",
        kinds.join("|")
    )
}
static HEADING: LazyLock<Regex> = LazyLock::new(|| pattern(&format!("(?mi){}", heading_pattern())));
static WHOLE_HEADING: LazyLock<Regex> = LazyLock::new(|| pattern(&format!(r"(?i)\A(?:{})\z", heading_pattern())));
/// Sections that end the paper's own text.
static CLOSING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)\A(?:References|Bibliography|Acknowledge?ments?|Appendix\b.{0,40}|Appendices|(?:NeurIPS )?Paper Checklist|Supplementary Material)\.?\z",
    )
});
static SECTION: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"\A(?P<prefix>[A-Z]|\d{1,2})(?P<rest>(?:\.\d{1,2})*)(?P<dot>\.?)[ \t]+(?P<title>[A-Z].{2,68})\z")
});
static NEGATION: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?i)\b(?:not|no|never|without|none|neither|nor)\b|n't"));
static USED: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)\b(?:us(?:e|ed|es|ing)|employ(?:ed|s)?|utili[sz]ed?|leverag(?:e|ed)|assist(?:ed|ance)?|help(?:ed|s)?|edit(?:ed|ing)?|proofread(?:ing)?|correct(?:ed|ing|ions?)?|refin(?:e|ed|ing)|improv(?:e|ed|ing)|rephras(?:e|ed|ing)|rel(?:y|ied)|generat(?:e|ed)|polish(?:ed)?|aid(?:ed)?)\b",
    )
});
/// Where a sentence turns: "used it to polish, not to write", "used it for X and did not use it for Y". The
/// sentence is cut where one of these starts.
static CONTRAST: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i),\s*not\b|\band (?:\w+ )?not\b|\bbut\b|\bhowever\b|\balthough\b|\bthough\b"));
/// "not used, except for polishing", "no use beyond grammar": an exception to a denial is a declared use.
static EXCEPT: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"(?i)\b(?:except|other than|apart from|aside from|beyond|limited to|(?:only|solely) (?:for|to|as))\b")
});
/// Lines a PDF repeats on every page, which are not part of what the authors say.
static FURNITURE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)\A(?:\d{1,3}|Published as a conference paper at .*|Under review as .*|Preprint\b.*|Proceedings of the .*|\d+(?:st|nd|rd|th) (?:International )?Conference on .*)\z",
    )
});

/// Words a heading does not contain: "3 We then use".
const RUNNING: [&str; 15] =
    ["we", "then", "our", "is", "are", "was", "were", "this", "that", "which", "it", "use", "used", "show", "shows"];
/// Words a heading does not end on: "A Large fraction of".
const DANGLING: [&str; 13] = ["of", "the", "a", "an", "in", "to", "and", "for", "with", "on", "by", "from", "as"];

/// What the authors declare in a paper's text.
pub fn read_declarations(text: &str) -> Declarations {
    // ligatures ("Justiﬁcation") and odd spaces, as PDFs have them
    let text: String = text.nfkc().collect();
    let checklist = checklist(&text);
    let statements = statements(&text);
    let declares_llm_use = statements
        .iter()
        .find(|statement| statement.kind == StatementKind::Llm)
        .is_some_and(|statement| declares_use(&statement.text));
    let flags = Flag::ALL
        .into_iter()
        .filter(|flag| {
            checklist.iter().any(|item| flag.raised_by(item)) || (*flag == Flag::LlmDeclared && declares_llm_use)
        })
        .collect();
    Declarations { checklist, statements, flags }
}

/// The checklist's items, each read from its own stretch of text so that one unreadable item cannot take the
/// next one's answer. An item counts only if it starts "N. Title" followed shortly by "Question:".
fn checklist(text: &str) -> Vec<ChecklistItem> {
    let Some(start) = text.rfind(CHECKLIST) else {
        return Vec::new();
    };
    let section = &text[start..];
    // (number, where the item's line starts, where its title starts)
    let starts: Vec<(u32, usize, usize)> = ITEM_START
        .captures_iter(section)
        .filter_map(|found| {
            let line = found.get(0)?;
            let number: u32 = found.get(1)?.as_str().parse().ok()?;
            let title = line.end() - 1; // the capital letter, which is one byte
            ((1..=MOST_ITEMS).contains(&number) && runs_into_question(&section[title..])).then_some((
                number,
                line.start(),
                title,
            ))
        })
        .collect();
    let mut items: BTreeMap<u32, ChecklistItem> = BTreeMap::new();
    for (index, &(number, _, title_start)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(section.len(), |&(_, next, _)| next);
        let block = &section[title_start..end];
        let (title, rest) = block.split_once("Question:").unwrap_or((block, ""));
        if items.contains_key(&number) {
            continue;
        }
        let Some(answer) = ANSWER.captures(rest) else {
            continue;
        };
        let written = answer.get(1).or_else(|| answer.get(2)).map_or("", |written| written.as_str());
        let Some(said) = Answer::read(written) else {
            continue;
        };
        let after = answer.get(0).map_or("", |whole| &rest[whole.end()..]);
        let title: Vec<&str> = title.split('\n').filter(|line| !PAGE_NUMBER.is_match(line)).collect();
        items.insert(
            number,
            ChecklistItem {
                number,
                title: squash(&title.join(" ")),
                answer: said,
                justification: justification(after),
            },
        );
    }
    items.into_values().collect()
}

/// Does an item's title run straight into its question?
fn runs_into_question(title_onwards: &str) -> bool {
    match first_chars(title_onwards, TITLE_REACH).split_once("Question:") {
        Some((title, _)) => !["Answer:", "Justification:", "Guidelines:"].iter().any(|label| title.contains(label)),
        None => false,
    }
}

/// The justification given after an answer: up to the item's guidelines, or to the end of its stretch of text.
fn justification(after_answer: &str) -> String {
    const LABEL: &str = "Justification:";
    let Some(found) = after_answer.find(LABEL) else {
        return String::new();
    };
    let said = after_answer[found + LABEL.len()..].trim_start();
    let end = JUSTIFICATION_END.find(said).map_or(said.len(), |guidelines| guidelines.start());
    first_chars(&squash(&said[..end]), JUSTIFICATION_LENGTH).to_owned()
}

/// A heading reads as a title: "Limitations", not the last word of a sentence that wrapped ("limitations.").
fn titled(heading: &Captures) -> bool {
    heading.name("title").and_then(|title| title.as_str().chars().next()).is_some_and(char::is_uppercase)
}

/// Each kind of statement once, the first place it is found outside the checklist.
fn statements(text: &str) -> Vec<Statement> {
    let body = text.rfind(CHECKLIST).map_or(text, |end| &text[..end]);
    let length = body.chars().count() as f64;
    let mut found: Vec<Statement> = Vec::new();
    for heading in HEADING.captures_iter(body) {
        let Some(kind) = StatementKind::ALL
            .into_iter()
            .find(|kind| heading.name(kind.group()).is_some_and(|group| !group.is_empty()))
        else {
            continue;
        };
        let (Some(whole), Some(title)) = (heading.get(0), heading.name("title")) else {
            continue;
        };
        if found.iter().any(|statement| statement.kind == kind) || !titled(&heading) {
            continue;
        }
        let numbered =
            heading.name("prefix").and_then(|prefix| prefix.as_str().chars().next()).is_some_and(char::is_numeric);
        if kind == StatementKind::Llm
            && numbered
            && (body[..whole.start()].chars().count() as f64) < SUBJECT_SHARE * length
        {
            continue;
        }
        let lines: Vec<&str> = first_chars(&body[whole.end()..], STATEMENT_REACH)
            .split('\n')
            .skip(1) // what is left of the heading's own line
            .filter(|line| !FURNITURE.is_match(line.trim()))
            .collect();
        let stop = lines.iter().position(|line| is_heading(line)).unwrap_or(lines.len());
        let same_line = heading.name("same_line").map_or("", |same_line| same_line.as_str());
        let said = squash(&format!("{same_line} {}", lines[..stop].join(" ")));
        if said.chars().count() >= SHORTEST_STATEMENT {
            found.push(Statement {
                kind,
                title: squash(title.as_str()),
                text: first_chars(&said, STATEMENT_LENGTH).to_owned(),
            });
        }
    }
    found
}

/// Does this line start another section? Text from two-column papers has short lines that begin "A Large
/// fraction of ..." or "3 We then ...", so a letter or number in front is not enough: the rest must read as a
/// title.
fn is_heading(line: &str) -> bool {
    let line = line.trim();
    if !(3..=72).contains(&line.chars().count()) {
        return false;
    }
    if CLOSING.is_match(line) || WHOLE_HEADING.captures(line).is_some_and(|statement| titled(&statement)) {
        return true;
    }
    let letters: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() >= 8 && letters.iter().all(|c| c.is_uppercase()) {
        return true; // REFERENCES, A  PROOFS OF THEOREMS
    }
    let Some(section) = SECTION.captures(line) else {
        return false;
    };
    if line.ends_with(['.', ',', ';', ':']) {
        return false;
    }
    let words: Vec<&str> = section.name("title").map_or("", |title| title.as_str()).split_whitespace().collect();
    let numbered_deeper = |group: &str| section.name(group).is_some_and(|found| !found.is_empty());
    if numbered_deeper("dot") || numbered_deeper("rest") {
        return true;
    }
    let lowered: Vec<String> = words.iter().map(|word| word.to_lowercase().trim_matches(',').to_owned()).collect();
    if lowered.iter().any(|word| RUNNING.contains(&word.as_str()))
        || lowered.last().is_some_and(|word| DANGLING.contains(&word.as_str()))
    {
        return false;
    }
    words.len() <= 3
        || words.iter().all(|word| {
            let first = word.chars().next();
            word.chars().count() < 4 || !first.is_some_and(char::is_alphabetic) || first.is_some_and(char::is_uppercase)
        })
}

/// The parts of a sentence between the places it turns. A part begins where a turn begins, as a split before a
/// look-ahead would cut it.
fn turns(sentence: &str) -> Vec<&str> {
    let mut cuts = vec![0];
    let mut from = 0;
    while let Some(turn) = CONTRAST.find_at(sentence, from) {
        cuts.push(turn.start());
        from = turn.start() + char_len_at(sentence, turn.start());
    }
    cuts.push(sentence.len());
    cuts.windows(2).map(|pair| &sentence[pair[0]..pair[1]]).collect()
}

/// Does a statement on language models say they were used? Judged sentence by sentence:
///
/// | sentence                                               | judged                                     |
/// |--------------------------------------------------------|--------------------------------------------|
/// | "We used one to polish the text, not to write it."     | a use that nothing before it negates: yes  |
/// | "No language models were used."                        | a use that is negated: no                  |
/// | "Not used, except for polishing the grammar."          | an exception to a denial is a use: yes     |
/// | "This section is left empty."                          | says neither: declares nothing             |
///
/// A sentence is cut where it turns ("..., not ...", "... and did not ...", "but"), so that a denial in one half
/// does not cancel a use in the other.
fn declares_use(statement: &str) -> bool {
    first_chars(statement, JUDGED_LENGTH).split(['.', ';', ':']).any(|sentence| {
        if NEGATION.is_match(sentence) && USED.is_match(sentence) && EXCEPT.is_match(sentence) {
            return true;
        }
        turns(sentence)
            .into_iter()
            .any(|part| USED.find(part).is_some_and(|used| !NEGATION.is_match(&part[..used.start()])))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    use Answer::{No, NotApplicable as NA, Yes};

    const NEURIPS: &str = "
5 Conclusion
We presented a method.

Limitations and Future Work
Our analysis assumes bounded gradients, which may not hold for very deep networks trained at scale.

References
[1] A. Author. A paper. 2024.

NeurIPS Paper Checklist
1. Claims
Question: Do the main claims made in the abstract and introduction accurately reflect the paper's contributions?
Answer: [Yes]
Justification: The claims match the results.
Guidelines:
• The answer NA means that the abstract and introduction do not include the claims.
2. Limitations
Question: Does the paper discuss the limitations of the work performed by the authors?
Answer: [No]
Justification: We did not have space.
Guidelines:
• The answer NA means that the paper has no limitation.
5. Open access to data and code
Question: Does the paper provide open access to the data and code?
Answer: [No]
Justification: The code is proprietary.
Guidelines:
• The answer NA means that paper does not include experiments requiring code.
14. Crowdsourcing and research with human subjects
Question: For crowdsourcing experiments and research with human subjects, does the paper include instructions?
Answer: [NA]
Justification: The paper does not involve crowdsourcing nor research with human subjects.
Guidelines:
• The answer NA means that the paper does not involve crowdsourcing.
15. Institutional review board (IRB) approvals or equivalent for research with human
subjects
Question: Does the paper describe potential risks incurred by study participants?
Answer: [NA]
Justification: Not applicable.
Guidelines:
• The answer NA means that the paper does not involve crowdsourcing.
16. Declaration of LLM usage
20
Question: Does the paper describe the usage of LLMs if it is an important component of the core methods?
Answer: [Yes]
Justification: An LLM generates the candidate programs that our method searches over.
Guidelines:
• The answer NA means that the core method development does not involve LLMs.
";

    const ICML: &str = "
6. Conclusion
We presented a method.

Impact Statement
This paper presents work whose goal is to advance the field of Machine Learning. There are many potential
societal consequences of our work, none which we feel must be specifically highlighted here.

References
Author, A. A paper. 2024.
";

    const ICLR: &str = "
ETHICS STATEMENT
This work uses only public datasets and raises no privacy concerns that we are aware of at this time.

REPRODUCIBILITY STATEMENT
All code and configuration files are released with the paper, and every experiment has a fixed random seed.

THE USE OF LARGE LANGUAGE MODELS
We used a large language model to polish the wording of the manuscript and to help draft plotting code.

REFERENCES
";

    /// The ICLR layout again, with a conclusion before and a reference after that uses the words "be used".
    const ICLR_WITH_REFERENCE: &str = "
5 CONCLUSION
We presented a method.

ETHICS STATEMENT
This work uses only public datasets and raises no privacy concerns that we are aware of at this time.

REPRODUCIBILITY STATEMENT
All code and configuration files are released with the paper, and every experiment has a fixed random seed.

THE USE OF LARGE LANGUAGE MODELS
We used a large language model to polish the wording of the manuscript and to help draft plotting code.

REFERENCES
A. Author. Models that do not need to be used twice. 2024.
";

    const DENIES: &str = "
A. Use of LLMs
We did not use large language models in any part of this research or in writing the paper itself.

B Additional Experiments
More results.
";

    fn item_with(number: u32, title: &str, answer: &str, justification: &str) -> String {
        format!(
            "{number}. {title}\nQuestion: Does the paper do this?\n{answer}\n{justification}Guidelines:\n• The answer NA means that it does not apply.\n"
        )
    }

    fn item(number: u32, title: &str, answer: &str) -> String {
        item_with(number, title, answer, "Justification: Because.\n")
    }

    fn checklist_of(items: &[String]) -> String {
        format!("Some paper text.\n\nNeurIPS Paper Checklist\n{}", items.concat())
    }

    fn answers(text: &str) -> BTreeMap<u32, Answer> {
        read_declarations(text).checklist.into_iter().map(|entry| (entry.number, entry.answer)).collect()
    }

    fn by_number(text: &str) -> BTreeMap<u32, ChecklistItem> {
        read_declarations(text).checklist.into_iter().map(|entry| (entry.number, entry)).collect()
    }

    fn flags(text: &str) -> Vec<Flag> {
        read_declarations(text).flags
    }

    fn statements_of(text: &str) -> BTreeMap<StatementKind, String> {
        read_declarations(text).statements.into_iter().map(|statement| (statement.kind, statement.text)).collect()
    }

    fn only_statement(text: &str) -> Statement {
        let mut statements = read_declarations(text).statements;
        assert_eq!(statements.len(), 1, "{statements:?}");
        statements.remove(0)
    }

    fn expected(pairs: &[(u32, Answer)]) -> BTreeMap<u32, Answer> {
        pairs.iter().copied().collect()
    }

    /// Is the use of language models declared by a statement under the usual heading?
    fn declared(statement: &str) -> bool {
        flags(&format!("\nUse of LLMs\n{statement}\n\nReferences\n")) == [Flag::LlmDeclared]
    }

    // ---------------------------------------------------------------- the formats of the conferences

    #[test]
    fn the_neurips_checklist_is_read_with_multi_line_titles_and_page_numbers() {
        let declared = read_declarations(NEURIPS);
        assert_eq!(answers(NEURIPS), expected(&[(1, Yes), (2, No), (5, No), (14, NA), (15, NA), (16, Yes)]));
        let items = by_number(NEURIPS);
        assert_eq!(
            items[&15].title,
            "Institutional review board (IRB) approvals or equivalent for research with human subjects"
        );
        assert_eq!(items[&16].title, "Declaration of LLM usage"); // the page number "20" is not part of it
        assert!(items[&16].justification.starts_with("An LLM generates the candidate programs"));
        assert_eq!(declared.flags, [Flag::LlmDeclared, Flag::ClosedCode, Flag::NoLimitations]);
    }

    #[test]
    fn statements_are_found_outside_the_checklist_only() {
        let statement = only_statement(NEURIPS);
        assert_eq!(statement.kind, StatementKind::Limitations);
        assert!(statement.text.starts_with("Our analysis assumes bounded gradients"));
        assert!(!statement.text.contains("References") && !statement.text.contains("Checklist"));
    }

    #[test]
    fn an_icml_impact_statement_is_read() {
        let declared = read_declarations(ICML);
        assert!(declared.checklist.is_empty() && declared.flags.is_empty());
        let statement = only_statement(ICML);
        assert_eq!(statement.kind, StatementKind::Impact);
        assert!(statement.text.starts_with("This paper presents work whose goal"));
        assert!(!statement.text.contains("Author, A."));
    }

    #[test]
    fn iclr_style_statements_in_capitals_and_a_stated_llm_use_are_read() {
        let kinds: Vec<StatementKind> = statements_of(ICLR).into_keys().collect();
        assert_eq!(kinds, [StatementKind::Llm, StatementKind::Ethics, StatementKind::Reproducibility]);
        assert_eq!(flags(ICLR), [Flag::LlmDeclared]);
    }

    #[test]
    fn a_statement_that_says_no_llm_was_used_raises_no_flag() {
        let statement = only_statement(DENIES);
        assert_eq!(statement.kind, StatementKind::Llm);
        assert!(!statement.text.contains("Additional Experiments"));
        assert!(flags(DENIES).is_empty());
    }

    #[test]
    fn a_section_that_merely_starts_with_the_word_is_not_a_statement() {
        let text = "\n4.2 Limitations of Linear Pooling Representations\nExisting methods pool tokens linearly, which loses order.\n";
        assert_eq!(read_declarations(text), Declarations::default());
        assert_eq!(read_declarations("Just a paper with nothing declared."), Declarations::default());
    }

    #[test]
    fn what_is_read_has_the_shape_the_interface_reads() {
        let declared = read_declarations(&checklist_of(&[item(
            15,
            "Institutional review board (IRB) approvals",
            "Answer: [N/A]",
        )]));
        assert_eq!(
            serde_json::to_value(&declared).unwrap(),
            json!({
                "checklist": [{"number": 15, "title": "Institutional review board (IRB) approvals", "answer": "NA",
                               "justification": "Because."}],
                "statements": [],
                "flags": [],
            })
        );
        let statement = serde_json::to_value(only_statement(DENIES)).unwrap();
        assert_eq!((statement["kind"].as_str(), statement["title"].as_str()), (Some("llm"), Some("Use of LLMs")));
        assert_eq!(
            serde_json::to_value(Flag::ALL).unwrap(),
            json!(["llm_declared", "closed_code", "no_limitations", "human_subjects"])
        );
        for flag in Flag::ALL {
            assert_eq!(serde_json::to_value(flag).unwrap(), flag.id());
            assert!(!flag.label().is_empty() && !flag.note().is_empty());
        }
        let back: Declarations = serde_json::from_value(serde_json::to_value(&declared).unwrap()).unwrap();
        assert_eq!(back, declared);
    }

    // ---------------------------------------------------------------- the checklist, as PDFs really give it

    #[test]
    fn an_answer_written_another_way_is_still_that_items_answer() {
        let text = checklist_of(&[
            item(15, "Institutional review board (IRB) approvals", "Answer: [N/A]"),
            item(16, "Declaration of LLM usage", "Answer: [Yes]"),
        ]);
        assert_eq!(answers(&text), expected(&[(15, NA), (16, Yes)]));
        assert_eq!(flags(&text), [Flag::LlmDeclared]);

        let kerned = checklist_of(&[
            item(2, "Limitations", "Answer: [Y es]"),
            item(5, "Open access to data and code", "Answer: [No]"),
        ]);
        assert_eq!(answers(&kerned), expected(&[(2, Yes), (5, No)]));
        assert_eq!(flags(&kerned), [Flag::ClosedCode]);

        let bare = checklist_of(&[
            item(2, "Limitations", "Answer: No"),
            item(5, "Open access to data and code", "Answer: [Yes] See Section 3."),
        ]);
        assert_eq!(answers(&bare), expected(&[(2, No), (5, Yes)]));
        assert_eq!(flags(&bare), [Flag::NoLimitations]);
    }

    #[test]
    fn an_item_that_cannot_be_read_is_left_out_and_takes_nothing_from_the_next() {
        let text = checklist_of(&[
            item(2, "Limitations", "Answer: [TODO]"),
            item(5, "Open access to data and code", "Answer: [No]"),
        ]);
        assert_eq!(answers(&text), expected(&[(5, No)]));
        assert_eq!(flags(&text), [Flag::ClosedCode]);

        let unjustified = checklist_of(&[
            item_with(2, "Limitations", "Answer: [Yes]", ""),
            item_with(5, "Open access to data and code", "Answer: [No]", "Justification: Proprietary.\n"),
        ]);
        let read = by_number(&unjustified);
        assert_eq!(read[&2].justification, "");
        assert_eq!(read[&5].justification, "Proprietary.");
    }

    #[test]
    fn ligatures_and_wrapped_lines_inside_the_checklist_are_read() {
        let text = checklist_of(&[
            item_with(
                2,
                "Limitations",
                "Answer: [Yes]",
                "Justiﬁcation: We discuss them, see Section\n3. We have several.\n",
            ),
            item(5, "Open access to data and code", "Answer: [No]"),
        ]);
        let read = by_number(&text);
        assert_eq!(read.keys().copied().collect::<Vec<_>>(), [2, 5]); // "3. We have several." is not an item
        assert_eq!(read[&2].justification, "We discuss them, see Section 3. We have several.");
        assert_eq!(read[&5].answer, No);
    }

    #[test]
    fn flags_follow_what_an_item_is_about_not_its_number() {
        let moved = checklist_of(&[
            item(3, "Limitations", "Answer: [No]"),
            item(17, "Crowdsourcing and research with human subjects", "Answer: [Yes]"),
            item(
                18,
                "Institutional review board (IRB) approvals or equivalent for research with human subjects",
                "Answer: [Yes]",
            ),
            item(19, "Declaration of LLM usage", "Answer: [NA]"),
        ]);
        assert_eq!(flags(&moved), [Flag::NoLimitations, Flag::HumanSubjects]);
    }

    #[test]
    fn an_answer_on_the_line_after_its_label_is_read() {
        let text = checklist_of(&[
            item(2, "Limitations", "Answer:\n[No]"),
            item(5, "Open access to data and code", "Answer:  \n [Yes]"),
        ]);
        assert_eq!(answers(&text), expected(&[(2, No), (5, Yes)]));
        assert_eq!(flags(&text), [Flag::NoLimitations]);
    }

    // ---------------------------------------------------------------- where a statement ends

    #[test]
    fn a_statement_ends_at_the_next_heading_also_in_capitals() {
        let said = statements_of(ICLR_WITH_REFERENCE);
        assert!(said[&StatementKind::Ethics].ends_with("at this time."));
        assert!(said[&StatementKind::Reproducibility].ends_with("fixed random seed."));
        assert!(said[&StatementKind::Llm].ends_with("plotting code."));
        assert_eq!(flags(ICLR_WITH_REFERENCE), [Flag::LlmDeclared]); // the reference's "do not need to be used" is not theirs
    }

    #[test]
    fn a_short_line_of_running_text_does_not_end_a_statement() {
        let text = "\nImpact Statement\nThis paper presents work whose goal is to advance machine learning.\n\
                    A Large fraction of the training data comes from public sources,\nwhich we consider low risk overall.\n\
                    \nReferences\nAuthor, A. 2024.\n";
        assert!(only_statement(text).text.ends_with("low risk overall."));

        let appendix = "\nImpact Statement\nThis paper presents work whose goal is to advance machine learning.\nB Additional Experiments\nMore.\n";
        assert!(only_statement(appendix).text.ends_with("machine learning."));
    }

    #[test]
    fn a_sentence_that_wrapped_onto_the_word_is_not_a_heading() {
        let text = "\nWe now turn to the method and then to its\nlimitations.\nThe next paragraph talks about something else entirely, at some length.\n";
        assert!(read_declarations(text).statements.is_empty());
    }

    #[test]
    fn common_other_headings_are_statements() {
        let text = "\n7 Limitations and Broader Impacts\nOur method needs a GPU with a lot of memory, which limits who can use it today.\n\nReferences\n";
        assert_eq!(only_statement(text).kind, StatementKind::Limitations);
    }

    #[test]
    fn what_reads_as_a_heading() {
        for (line, heading) in [
            ("A Large fraction of", false),
            ("3 We then use", false),
            ("B Additional Experiments", true),
            ("6 Related work", true),
            ("A. Proofs", true),
            ("4.2 Results on ImageNet", true),
            ("REFERENCES", true),
            ("Acknowledgments", true),
            ("the limitations.", false),
            ("A Proofs of the Main Theorems", true),
        ] {
            assert_eq!(is_heading(line), heading, "{line}");
        }
    }

    #[test]
    fn page_furniture_is_not_part_of_a_statement() {
        let text = "\nETHICS STATEMENT\nThis work uses only public datasets and we thought about it\n12\n\
                    Published as a conference paper at ICLR 2026\ncarefully before releasing anything at all.\n\nREFERENCES\n";
        assert_eq!(
            only_statement(text).text,
            "This work uses only public datasets and we thought about it carefully before releasing anything at all."
        );
    }

    #[test]
    fn a_numbered_section_about_language_models_is_the_papers_subject() {
        let body = format!(
            "\n2 Use of Large Language Models\nPractitioners use large language models for many tasks, which we survey here.\n{}",
            "Text.\n".repeat(400)
        );
        assert!(flags(&body).is_empty());
        let late = format!(
            "{}\n9 Use of Large Language Models\nWe used an LLM to polish the wording of the manuscript before submission.\n",
            "Text.\n".repeat(400)
        );
        assert_eq!(flags(&late), [Flag::LlmDeclared]);
    }

    // ---------------------------------------------------------------- whether a statement declares use

    #[test]
    fn whether_a_statement_declares_use() {
        for (statement, use_declared) in [
            (
                "We used an LLM only to polish the writing; it was not used for research ideation or for the experiments.",
                true,
            ),
            ("LLMs were used to check grammar in this manuscript. No other use was made of them in this work.", true),
            ("Large language models helped with editing the text of this paper for clarity and concision.", true),
            (
                "We did not use large language models in any part of this research or in writing the paper itself.",
                false,
            ),
            ("No LLMs were used in the preparation of this manuscript or in the research it describes at all.", false),
            ("This work was carried out without the assistance of any large language model at any stage of it.", false),
        ] {
            assert_eq!(declared(statement), use_declared, "{statement}");
        }
    }

    #[test]
    fn exceptions_to_a_denial_are_declared_use_and_an_empty_statement_declares_nothing() {
        for (statement, use_declared) in [
            (
                "We did not use LLMs for anything other than polishing the writing of this manuscript before submission.",
                true,
            ),
            (
                "LLMs were not used in this work, except for polishing the grammar of a few paragraphs of the paper.",
                true,
            ),
            (
                "Apart from grammar editing of the final manuscript, no LLM was used at any point of this research.",
                true,
            ),
            ("We made no use of language models beyond minor grammar corrections to the text of the manuscript.", true),
            (
                "ChatGPT helped with wording in several sections of the manuscript; all content is the authors' own.",
                true,
            ),
            ("This section is intentionally left empty by the authors, as required by the submission template.", false),
        ] {
            assert_eq!(declared(statement), use_declared, "{statement}");
        }
    }

    #[test]
    fn a_denial_in_one_half_of_a_sentence_does_not_cancel_a_use_in_the_other() {
        for (statement, use_declared) in [
            ("LLMs were not involved in any part of this research, which is limited to classical methods.", false),
            ("We do not use LLMs. Our analysis is limited to the datasets described in Section 4 of the paper.", false),
            ("No language models were used. The code is released only for research purposes, under a licence.", false),
            ("We used GPT-4 to polish the writing of this manuscript, not to generate any of its ideas.", true),
            ("We used ChatGPT to polish writing and did not use it for research ideation or the experiments.", true),
            ("LLMs were not used for writing, editing and proofreading of any part of this paper at all.", false),
            ("No LLM was used at any point of this work, not even for editing the text of the manuscript.", false),
            ("We did not use LLMs for ideation, but used them for polishing the final text of the paper.", true),
        ] {
            assert_eq!(declared(statement), use_declared, "{statement}");
        }
    }

    #[test]
    fn a_sentence_is_cut_where_it_turns() {
        assert_eq!(turns("used it, not for that"), ["used it", ", not for that"]);
        assert_eq!(turns("polish and did not write"), ["polish ", "and did not write"]);
        assert_eq!(turns("nothing turns here"), ["nothing turns here"]);
    }
}
