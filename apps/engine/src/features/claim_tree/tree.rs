//! A paper's claims as a tree, from the model's answer: checked as a tree, and every node's quotes checked against
//! the paper, before anything of it is shown.
//!
//! ```text
//! Claim                       the paper's central claims, at the root (at most MAX_CLAIMS)
//! ├── Finding                 the contributions and findings that support it (at most MAX_FINDINGS each)
//! │   └── Evidence{kind}      what the paper offers for it: an experiment, a table, a figure, a theorem,
//! │                           an ablation, a citation, an argument (at most MAX_EVIDENCE each)
//! └── Evidence{kind}          evidence may also stand right under a claim
//! ```
//!
//! Three levels at most, [`MAX_NODES`] in all. The model answers a flat list of nodes, each naming its parent;
//! [`build`] makes the tree of it:
//!
//! 1. **Malformed** answers are refused whole ([`Malformed`]): two nodes with one id, or parents that go round in
//!    a circle. Nothing of such an answer is shown; the user can ask again.
//! 2. **A node that cannot stand where it is** is dropped with everything under it, and counted: one whose
//!    parent is not in the answer (`no_parent`), a finding or evidence at the root or a claim under something
//!    (`misplaced`).
//! 3. **A node whose quotes are not found in the paper** ([`evidence::check`]) is dropped and counted
//!    (`unquoted`). What was under it moves up to its parent when it may stand there (evidence under a dropped
//!    finding goes to the finding's claim); what may not (a finding whose claim was dropped) is dropped and
//!    counted (`orphaned`). A claim of the paper is not invented to hold it.
//! 4. **Past the bounds**, later siblings are dropped with what is under them, and counted (`over_bounds`).
//!
//! The nodes that stay are numbered again ("n1", "n2", ... in the tree's order), so that the same answer checked
//! against the same text always gives the same ids, which the mapping of reviews refers to.
//!
//! An evidence node that names a table, a figure or a theorem ("Table 2") also points at that caption or that
//! theorem's passage in the paper, as evidence read from the paper's own place ([`Evidence::at`]).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::evidence::{self, Claim, Evidence, PaperText, Papers, Quote, Rules};
use crate::text::first_chars;
use crate::text::passages::{Kind as PassageKind, Passages};

/// The paper's central claims, at most.
pub const MAX_CLAIMS: usize = 5;
/// Findings under one claim, at most.
pub const MAX_FINDINGS: usize = 6;
/// Evidence under one claim or finding, at most.
pub const MAX_EVIDENCE: usize = 5;
/// Nodes in all, at most.
pub const MAX_NODES: usize = 60;
/// Nodes of an answer read, at most: an answer of more is not a tree of one paper's claims.
pub const MAX_ANSWERED: usize = 400;
/// Quotes of one node sent to be checked, at most.
pub const MAX_QUOTES: usize = 4;
/// Characters of a node's statement kept.
pub const STATEMENT_LENGTH: usize = 600;

/// What evidence a paper offers for a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Experiment,
    Table,
    Figure,
    Theorem,
    Ablation,
    Citation,
    Argument,
}

/// What a node of the tree is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "evidence_kind", rename_all = "snake_case")]
pub enum NodeKind {
    /// One of the paper's central claims.
    Claim,
    /// A contribution or finding that supports a claim.
    Finding,
    Evidence(EvidenceKind),
}

impl NodeKind {
    /// May a node of this kind stand under one of `parent`'s kind (`None`: at the root)?
    fn may_stand_under(self, parent: Option<NodeKind>) -> bool {
        matches!(
            (self, parent),
            (NodeKind::Claim, None)
                | (NodeKind::Finding, Some(NodeKind::Claim))
                | (NodeKind::Evidence(_), Some(NodeKind::Claim | NodeKind::Finding))
        )
    }

    fn most_under(self) -> (usize, usize) {
        // (findings, evidence) a node of this kind may have under it
        match self {
            NodeKind::Claim => (MAX_FINDINGS, MAX_EVIDENCE),
            NodeKind::Finding => (0, MAX_EVIDENCE),
            NodeKind::Evidence(_) => (0, 0),
        }
    }
}

/// A node's kind as the model names it: one flat list, so that the answer's schema stays simple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaidKind {
    Claim,
    Finding,
    Experiment,
    Table,
    Figure,
    Theorem,
    Ablation,
    Citation,
    Argument,
}

impl From<SaidKind> for NodeKind {
    fn from(said: SaidKind) -> Self {
        match said {
            SaidKind::Claim => NodeKind::Claim,
            SaidKind::Finding => NodeKind::Finding,
            SaidKind::Experiment => NodeKind::Evidence(EvidenceKind::Experiment),
            SaidKind::Table => NodeKind::Evidence(EvidenceKind::Table),
            SaidKind::Figure => NodeKind::Evidence(EvidenceKind::Figure),
            SaidKind::Theorem => NodeKind::Evidence(EvidenceKind::Theorem),
            SaidKind::Ablation => NodeKind::Evidence(EvidenceKind::Ablation),
            SaidKind::Citation => NodeKind::Evidence(EvidenceKind::Citation),
            SaidKind::Argument => NodeKind::Evidence(EvidenceKind::Argument),
        }
    }
}

/// One node as the model answered it. Kept as it came (the library keeps the model's answer, never evidence).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaidNode {
    pub id: String,
    /// The id of the node it is under; `null` for a central claim.
    pub parent: Option<String>,
    pub kind: SaidKind,
    pub statement: String,
    /// Words copied from the paper.
    pub quotes: Vec<String>,
    /// The table, figure or theorem it rests on, as the paper names it ("Table 2"); `null` when none.
    pub label: Option<String>,
}

/// The model's answer to "what does this paper claim, and on what".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaidTree {
    pub nodes: Vec<SaidNode>,
}

/// The schema the answer must match (every object closed, as structured outputs require).
pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object", "additionalProperties": false, "required": ["nodes"],
        "properties": {"nodes": {"type": "array", "items": {
            "type": "object", "additionalProperties": false,
            "required": ["id", "parent", "kind", "statement", "quotes", "label"],
            "properties": {
                "id": {"type": "string"},
                "parent": {"type": ["string", "null"]},
                "kind": {"type": "string", "enum": [
                    "claim", "finding", "experiment", "table", "figure", "theorem", "ablation", "citation", "argument"]},
                "statement": {"type": "string"},
                "quotes": {"type": "array", "items": {"type": "string"}},
                "label": {"type": ["string", "null"]}
            }
        }}}
    })
}

/// Why an answer is not a tree at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "why", content = "id", rename_all = "snake_case")]
pub enum Malformed {
    #[error("two nodes of the model's answer have the id {0}")]
    DuplicateId(String),
    #[error("the parents of the model's answer go round in a circle at {0}")]
    Cycle(String),
    #[error("the model's answer has no claims")]
    Empty,
}

/// One node of the checked tree: what is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Node {
    /// "n1", "n2", ... in the tree's order.
    pub id: String,
    pub parent: Option<String>,
    #[serde(flatten)]
    pub kind: NodeKind,
    /// The model's reading of the paper, in its words.
    pub statement: String,
    /// The paper's own words supporting it: every piece found in the paper.
    pub evidence: Vec<Evidence>,
    /// The caption of the table or figure, or the theorem's passage, the node names, when the paper has it.
    pub points_at: Option<Evidence>,
}

/// What was left out of the model's answer, and why: counted, never shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LeftOut {
    /// Its quotes were not found in the paper.
    pub unquoted: usize,
    /// It stood under a node that was dropped and may not stand where that node stood.
    pub orphaned: usize,
    /// Its parent is not in the answer.
    pub no_parent: usize,
    /// It may not stand where the answer put it.
    pub misplaced: usize,
    /// Past the bounds on the tree's size.
    pub over_bounds: usize,
}

impl LeftOut {
    pub fn total(&self) -> usize {
        self.unquoted + self.orphaned + self.no_parent + self.misplaced + self.over_bounds
    }
}

/// The checked tree: its nodes in the tree's order (each node before what is under it), and what was left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub left_out: LeftOut,
}

impl Tree {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// The nodes right under `id` (`None`: the central claims), in order.
    pub fn children<'a>(&'a self, id: Option<&'a str>) -> impl Iterator<Item = &'a Node> + 'a {
        self.nodes.iter().filter(move |node| node.parent.as_deref() == id)
    }

    /// Does anything under `id`, at any depth, count as evidence?
    pub fn has_evidence_under(&self, id: &str) -> bool {
        self.children(Some(id))
            .any(|child| matches!(child.kind, NodeKind::Evidence(_)) || self.has_evidence_under(&child.id))
    }
}

/// A node on its way through the checks: by its index in the answer.
struct Draft {
    kind: NodeKind,
    parent: Option<usize>,
    statement: String,
    evidence: Vec<Evidence>,
    label: Option<String>,
}

/// The tree of `said`, every node's quotes looked for in `paper`'s text (`paper_id`'s), as the module says.
pub fn build(said: &SaidTree, paper_id: &str, text: &PaperText) -> Result<Tree, Malformed> {
    // what is read of the answer: past this many nodes, the rest is counted as past the bounds unread
    let answered = &said.nodes[..said.nodes.len().min(MAX_ANSWERED)];
    let mut left_out = LeftOut { over_bounds: said.nodes.len() - answered.len(), ..LeftOut::default() };
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (at, node) in answered.iter().enumerate() {
        if index.insert(node.id.as_str(), at).is_some() {
            return Err(Malformed::DuplicateId(first_chars(&node.id, 40).to_owned()));
        }
    }
    // parents by index; one that is not in the answer leaves its node with nowhere to stand
    let mut parents: Vec<Option<usize>> = Vec::with_capacity(answered.len());
    let mut dropped = vec![false; answered.len()];
    for (at, node) in answered.iter().enumerate() {
        match node.parent.as_deref() {
            None => parents.push(None),
            Some(parent) => match index.get(parent) {
                Some(&parent) => parents.push(Some(parent)),
                None => {
                    parents.push(None);
                    dropped[at] = true;
                    left_out.no_parent += 1;
                }
            },
        }
    }
    for (start, node) in answered.iter().enumerate() {
        let mut seen = HashSet::from([start]);
        let mut at = start;
        while let Some(parent) = parents[at] {
            if !seen.insert(parent) {
                return Err(Malformed::Cycle(first_chars(&node.id, 40).to_owned()));
            }
            at = parent;
        }
    }
    // a node under a dropped one is dropped with it (counted with its own reason when it has one)
    let kinds: Vec<NodeKind> = answered.iter().map(|node| node.kind.into()).collect();
    for at in order(&parents) {
        if dropped[at] {
            continue;
        }
        let parent = parents[at];
        if parent.is_some_and(|parent| dropped[parent]) {
            dropped[at] = true;
            left_out.no_parent += 1;
        } else if !kinds[at].may_stand_under(parent.map(|parent| kinds[parent])) {
            dropped[at] = true;
            left_out.misplaced += 1;
        }
    }

    // the quotes of the nodes that stand, looked for in the paper
    let standing: Vec<usize> = (0..answered.len()).filter(|at| !dropped[*at]).collect();
    let claims: Vec<Claim<usize>> = standing
        .iter()
        .map(|&at| Claim {
            statement: at,
            quotes: answered[at]
                .quotes
                .iter()
                .take(MAX_QUOTES)
                .map(|quote| Quote { paper: paper_id.to_owned(), quote: quote.clone() })
                .collect(),
        })
        .collect();
    let papers = Papers::from([(paper_id.to_owned(), text.clone())]);
    let checked = evidence::check(claims, &papers, Rules::ONE_PAPER);
    let mut drafts: Vec<Option<Draft>> = (0..answered.len()).map(|_| None).collect();
    for cited in checked.cited {
        let node = &answered[cited.statement];
        drafts[cited.statement] = Some(Draft {
            kind: kinds[cited.statement],
            parent: parents[cited.statement],
            statement: first_chars(node.statement.trim(), STATEMENT_LENGTH).to_owned(),
            evidence: cited.evidence,
            label: node.label.clone(),
        });
    }
    left_out.unquoted += checked.dropped.len();

    // what stood under an unquoted node moves up when it may; the tree's order puts parents first
    for at in order(&parents) {
        let Some(draft) = &drafts[at] else { continue };
        // the nearest node above that still stands (nodes above a standing one were not dropped, only unquoted
        // or orphaned)
        let mut parent = draft.parent;
        while let Some(up) = parent {
            if drafts[up].is_some() {
                break;
            }
            parent = parents[up];
        }
        let kind = draft.kind;
        let fits = match parent {
            Some(up) => drafts[up].as_ref().is_some_and(|above| kind.may_stand_under(Some(above.kind))),
            None => kind.may_stand_under(None),
        };
        // a node whose parent was unquoted, and none above it that stands, has nowhere to go
        let parent_gone = draft.parent.is_some() && parent.is_none();
        if fits && !parent_gone {
            if let Some(draft) = drafts[at].as_mut() {
                draft.parent = parent;
            }
        } else {
            drafts[at] = None;
            left_out.orphaned += 1;
        }
    }

    // the bounds, in the answer's order; then the ids of the tree's order
    let mut nodes: Vec<(usize, Draft)> = Vec::new();
    let mut count_under: HashMap<Option<usize>, (usize, usize)> = HashMap::new();
    let mut kept: HashSet<usize> = HashSet::new();
    for at in order(&parents) {
        let Some(draft) = drafts[at].take() else { continue };
        let parent = draft.parent;
        if parent.is_some_and(|parent| !kept.contains(&parent)) || nodes.len() >= MAX_NODES {
            left_out.over_bounds += 1;
            continue;
        }
        let (most_findings, most_evidence) = match parent {
            None => (MAX_CLAIMS, 0),
            Some(parent) => {
                nodes.iter().find(|(index, _)| *index == parent).map_or((0, 0), |(_, above)| above.kind.most_under())
            }
        };
        let under = count_under.entry(parent).or_default();
        let room = match draft.kind {
            NodeKind::Claim | NodeKind::Finding => {
                under.0 += 1;
                under.0 <= most_findings
            }
            NodeKind::Evidence(_) => {
                under.1 += 1;
                under.1 <= most_evidence
            }
        };
        if !room {
            left_out.over_bounds += 1;
            continue;
        }
        kept.insert(at);
        nodes.push((at, draft));
    }
    if said.nodes.is_empty() {
        return Err(Malformed::Empty);
    }
    let ids: HashMap<usize, String> =
        nodes.iter().enumerate().map(|(number, (at, _))| (*at, format!("n{}", number + 1))).collect();
    let passages = match text {
        PaperText::Read(passages) => Some(passages),
        PaperText::Unread { .. } => None,
    };
    let nodes = nodes
        .into_iter()
        .map(|(at, draft)| {
            let points_at = match (draft.kind, passages) {
                (NodeKind::Evidence(kind), Some(passages)) => {
                    let named = draft.label.as_deref().into_iter().chain(std::iter::once(draft.statement.as_str()));
                    let named = named.chain(draft.evidence.iter().map(Evidence::quote));
                    named.filter_map(|text| label_in(text, kind)).find_map(|label| place_of(paper_id, passages, &label))
                }
                _ => None,
            };
            Node {
                id: ids[&at].clone(),
                parent: draft.parent.map(|parent| ids[&parent].clone()),
                kind: draft.kind,
                statement: draft.statement,
                evidence: draft.evidence,
                points_at,
            }
        })
        .collect();
    Ok(Tree { nodes, left_out })
}

/// The answer's indices with every node after its parent, siblings in the answer's order (depth first). There
/// are no cycles by now.
fn order(parents: &[Option<usize>]) -> Vec<usize> {
    let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (at, parent) in parents.iter().enumerate() {
        children.entry(*parent).or_default().push(at);
    }
    let mut ordered = Vec::with_capacity(parents.len());
    let mut stack: Vec<usize> =
        children.get(&None).map(|roots| roots.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(at) = stack.pop() {
        ordered.push(at);
        if let Some(under) = children.get(&Some(at)) {
            stack.extend(under.iter().rev());
        }
    }
    ordered
}

/// What a table, a figure or a theorem is called, and its number.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Label {
    what: Labelled,
    number: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Labelled {
    Table,
    Figure,
    /// A theorem, lemma, proposition or corollary: its own word is matched.
    Statement(&'static str),
}

#[allow(clippy::unwrap_used)] // a pattern written here either always compiles or never does: the tests say which
static LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(tables?|tab\.|figures?|figs?\.|theorems?|thm\.|lemmas?|propositions?|prop\.|corollary|corollaries)[^\S\n]*([A-Z]?\d+(?:\.\d+)?)").unwrap()
});

fn label_in(text: &str, kind: EvidenceKind) -> Option<Label> {
    LABEL.captures_iter(text).find_map(|found| {
        let word = found.get(1)?.as_str().to_lowercase();
        let what = if word.starts_with("tab") {
            Labelled::Table
        } else if word.starts_with("fig") {
            Labelled::Figure
        } else if word.starts_with("lem") {
            Labelled::Statement("lemma")
        } else if word.starts_with("prop") {
            Labelled::Statement("proposition")
        } else if word.starts_with("cor") {
            Labelled::Statement("corollary")
        } else {
            Labelled::Statement("theorem")
        };
        // an experiment or an ablation may rest on a table or figure; a theorem on its statement
        let wanted = match kind {
            EvidenceKind::Table | EvidenceKind::Figure | EvidenceKind::Experiment | EvidenceKind::Ablation => {
                matches!(what, Labelled::Table | Labelled::Figure)
            }
            EvidenceKind::Theorem => matches!(what, Labelled::Statement(_)),
            EvidenceKind::Citation | EvidenceKind::Argument => false,
        };
        let number = found.get(2)?.as_str().to_owned();
        wanted.then_some(Label { what, number })
    })
}

/// Does `text` start by naming `label` ("Table 2:", "Fig. 2.", "Theorem 3.1 (Convergence).")?
fn starts_with_label(text: &str, label: &Label) -> bool {
    let Some(found) = LABEL.captures(text) else { return false };
    let (Some(whole), Some(word), Some(number)) = (found.get(0), found.get(1), found.get(2)) else { return false };
    if !text[..whole.start()].trim().is_empty() || number.as_str() != label.number {
        return false;
    }
    let word = word.as_str().to_lowercase();
    match label.what {
        Labelled::Table => word.starts_with("tab"),
        Labelled::Figure => word.starts_with("fig"),
        Labelled::Statement(name) => word.starts_with(&name[..3]),
    }
}

/// The caption of the table or figure `label` names, or the passage that states the theorem, as evidence at its
/// place in the paper.
fn place_of(paper_id: &str, passages: &Passages, label: &Label) -> Option<Evidence> {
    let wanted = |kind: PassageKind| match label.what {
        Labelled::Table => kind == PassageKind::TableCaption,
        Labelled::Figure => kind == PassageKind::FigureCaption,
        Labelled::Statement(_) => kind == PassageKind::Text,
    };
    passages
        .passages
        .iter()
        .find(|passage| wanted(passage.kind) && starts_with_label(&passage.text, label))
        .and_then(|passage| Evidence::at(paper_id, passages, &passage.address))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use crate::text::passages::read_passages;

    /// A made-up paper: every word of it invented for these tests.
    pub(crate) const PAPER: &str = "Sparse Routing Without Tears\n\n1. Introduction\n\nWe show that a routing rule with a learned temperature matches dense experts at a fraction of the compute.\n\nOur first contribution is a routing rule that keeps every expert busy during training.\n\n2. Method\n\nTheorem 1. The routing rule converges to a balanced assignment under mild conditions.\n\n3. Experiments\n\nOn the language modelling benchmark the rule reaches the perplexity of the dense model with a third of the compute.\n\nTable 2: Perplexity and compute of the routing rule and the baselines on the language modelling benchmark.\n\nRemoving the temperature raises the perplexity by four points, which isolates its effect.\n";

    pub(crate) fn text() -> PaperText {
        PaperText::Read(read_passages(&[PAPER.to_owned()]))
    }

    pub(crate) fn node(id: &str, parent: Option<&str>, kind: SaidKind, quote: &str) -> SaidNode {
        SaidNode {
            id: id.to_owned(),
            parent: parent.map(str::to_owned),
            kind,
            statement: format!("statement of {id}"),
            quotes: vec![quote.to_owned()],
            label: None,
        }
    }

    pub(crate) fn said() -> SaidTree {
        SaidTree {
            nodes: vec![
                node("c1", None, SaidKind::Claim, "matches dense experts at a fraction of the compute"),
                node(
                    "f1",
                    Some("c1"),
                    SaidKind::Finding,
                    "a routing rule that keeps every expert busy during training",
                ),
                SaidNode {
                    label: Some("Table 2".to_owned()),
                    ..node(
                        "e1",
                        Some("f1"),
                        SaidKind::Experiment,
                        "reaches the perplexity of the dense model with a third of the compute",
                    )
                },
                node(
                    "e2",
                    Some("f1"),
                    SaidKind::Ablation,
                    "Removing the temperature raises the perplexity by four points",
                ),
                node("e3", Some("c1"), SaidKind::Theorem, "converges to a balanced assignment under mild conditions"),
            ],
        }
    }

    fn kinds(tree: &Tree) -> Vec<(&str, Option<&str>, NodeKind)> {
        tree.nodes.iter().map(|node| (node.id.as_str(), node.parent.as_deref(), node.kind)).collect()
    }

    #[test]
    fn a_well_formed_answer_becomes_a_tree_with_every_quote_found_in_the_paper() {
        let tree = build(&said(), "p", &text()).unwrap();
        assert_eq!(
            kinds(&tree),
            [
                ("n1", None, NodeKind::Claim),
                ("n2", Some("n1"), NodeKind::Finding),
                ("n3", Some("n2"), NodeKind::Evidence(EvidenceKind::Experiment)),
                ("n4", Some("n2"), NodeKind::Evidence(EvidenceKind::Ablation)),
                ("n5", Some("n1"), NodeKind::Evidence(EvidenceKind::Theorem)),
            ]
        );
        assert_eq!(tree.left_out, LeftOut::default());
        assert_eq!(tree.nodes[0].evidence[0].quote(), "matches dense experts at a fraction of the compute");
        assert!(tree.has_evidence_under("n1") && tree.has_evidence_under("n2") && !tree.has_evidence_under("n3"));
    }

    #[test]
    fn an_evidence_node_points_at_the_caption_or_theorem_it_names() {
        let tree = build(&said(), "p", &text()).unwrap();
        let table = tree.nodes[2].points_at.as_ref().unwrap();
        assert!(table.quote().starts_with("Table 2: Perplexity and compute"), "{}", table.quote());
        assert!(table.address().is_some());
        // the theorem is named by its quote's own passage: "Theorem 1." found from the statement's passage
        let theorem = SaidTree {
            nodes: vec![
                node("c1", None, SaidKind::Claim, "matches dense experts at a fraction of the compute"),
                SaidNode {
                    label: Some("Theorem 1".to_owned()),
                    ..node(
                        "t",
                        Some("c1"),
                        SaidKind::Theorem,
                        "converges to a balanced assignment under mild conditions",
                    )
                },
            ],
        };
        let tree = build(&theorem, "p", &text()).unwrap();
        assert!(tree.nodes[1].points_at.as_ref().unwrap().quote().starts_with("Theorem 1. The routing rule converges"));
        // an ablation naming no table points at nothing
        assert_eq!(build(&said(), "p", &text()).unwrap().nodes[3].points_at, None);
    }

    #[test]
    fn a_fabricated_quote_drops_its_node_and_what_is_under_it_moves_up_or_is_counted() {
        let mut answer = said();
        answer.nodes[1].quotes = vec!["our rule halves the cost of every model ever trained".to_owned()];
        let tree = build(&answer, "p", &text()).unwrap();
        // the finding is gone; its evidence now stands under the claim
        assert_eq!(
            kinds(&tree),
            [
                ("n1", None, NodeKind::Claim),
                ("n2", Some("n1"), NodeKind::Evidence(EvidenceKind::Experiment)),
                ("n3", Some("n1"), NodeKind::Evidence(EvidenceKind::Ablation)),
                ("n4", Some("n1"), NodeKind::Evidence(EvidenceKind::Theorem)),
            ]
        );
        assert_eq!(tree.left_out, LeftOut { unquoted: 1, ..LeftOut::default() });

        // a claim whose quote is not found takes its finding with it: a finding cannot be a central claim
        let mut answer = said();
        answer.nodes[0].quotes = vec!["this paper solves routing for good and for everyone".to_owned()];
        answer.nodes.push(node("c2", None, SaidKind::Claim, "a routing rule with a learned temperature"));
        let tree = build(&answer, "p", &text()).unwrap();
        assert_eq!(kinds(&tree), [("n1", None, NodeKind::Claim)]);
        assert_eq!(tree.left_out, LeftOut { unquoted: 1, orphaned: 4, ..LeftOut::default() });
        assert_eq!(tree.left_out.total(), 5);
    }

    #[test]
    fn a_quote_from_another_paper_is_not_found_in_this_one() {
        let mut answer = said();
        answer.nodes[3].quotes = vec!["distils a large teacher into a small student".to_owned()];
        let tree = build(&answer, "p", &text()).unwrap();
        assert_eq!(tree.nodes.len(), 4);
        assert_eq!(tree.left_out.unquoted, 1);
    }

    #[test]
    fn a_node_that_cannot_stand_where_it_is_is_dropped_and_counted() {
        let mut answer = said();
        answer.nodes.push(node("f9", Some("nowhere"), SaidKind::Finding, "keeps every expert busy during training"));
        answer.nodes.push(node("e9", Some("f9"), SaidKind::Table, "Perplexity and compute of the routing rule"));
        answer.nodes.push(node("f8", None, SaidKind::Finding, "keeps every expert busy during training"));
        answer.nodes.push(node("c9", Some("f1"), SaidKind::Claim, "keeps every expert busy during training"));
        let tree = build(&answer, "p", &text()).unwrap();
        assert_eq!(tree.nodes.len(), 5);
        assert_eq!(tree.left_out, LeftOut { no_parent: 2, misplaced: 2, ..LeftOut::default() });
    }

    #[test]
    fn duplicate_ids_and_cycles_are_not_a_tree_at_all() {
        let mut twice = said();
        twice.nodes[2].id = "f1".to_owned();
        assert_eq!(build(&twice, "p", &text()), Err(Malformed::DuplicateId("f1".to_owned())));
        let mut circle = said();
        circle.nodes[0].parent = Some("e1".to_owned());
        assert!(matches!(build(&circle, "p", &text()), Err(Malformed::Cycle(_))));
        let mut itself = said();
        itself.nodes[1].parent = Some("f1".to_owned());
        assert!(matches!(build(&itself, "p", &text()), Err(Malformed::Cycle(_))));
        assert_eq!(build(&SaidTree { nodes: Vec::new() }, "p", &text()), Err(Malformed::Empty));
    }

    #[test]
    fn the_tree_never_passes_its_bounds() {
        let mut answer = SaidTree { nodes: Vec::new() };
        for claim in 0..(MAX_CLAIMS + 2) {
            answer.nodes.push(node(
                &format!("c{claim}"),
                None,
                SaidKind::Claim,
                "matches dense experts at a fraction of the compute",
            ));
            for finding in 0..(MAX_FINDINGS + 1) {
                let id = format!("f{claim}.{finding}");
                answer.nodes.push(node(
                    &id,
                    Some(&format!("c{claim}")),
                    SaidKind::Finding,
                    "keeps every expert busy during training",
                ));
                for evidence in 0..(MAX_EVIDENCE + 1) {
                    answer.nodes.push(node(
                        &format!("e{claim}.{finding}.{evidence}"),
                        Some(&id),
                        SaidKind::Experiment,
                        "reaches the perplexity of the dense model",
                    ));
                }
            }
        }
        let tree = build(&answer, "p", &text()).unwrap();
        assert_eq!(tree.nodes.len(), MAX_NODES);
        assert!(tree.children(None).count() <= MAX_CLAIMS);
        for node in &tree.nodes {
            assert!(tree.children(Some(&node.id)).count() <= MAX_FINDINGS.max(MAX_EVIDENCE));
        }
        assert_eq!(tree.nodes.len() + tree.left_out.over_bounds, answer.nodes.len());
        // depth: three levels at most, by construction of where a kind may stand
        for node in &tree.nodes {
            let mut depth = 1;
            let mut at = node.parent.clone();
            while let Some(parent) = at {
                depth += 1;
                at = tree.node(&parent).unwrap().parent.clone();
            }
            assert!(depth <= 3);
        }
    }

    #[test]
    fn an_answer_of_ten_thousand_nodes_is_read_to_its_bound_and_every_node_is_shown_or_counted() {
        let mut answer = SaidTree { nodes: Vec::new() };
        let quotes = [
            "matches dense experts at a fraction of the compute",
            "a routing rule that keeps every expert busy during training",
            "reaches the perplexity of the dense model with a third of the compute",
            "a quote that is in no paper anywhere at all",
        ];
        for at in 0..10_000 {
            let id = format!("x{at}");
            let (parent, kind) = match at % 5 {
                0 => (None, SaidKind::Claim),
                1 => (Some(format!("x{}", at - 1)), SaidKind::Finding),
                2 => (Some(format!("x{}", at - 1)), SaidKind::Experiment),
                // a chain three deep under evidence: misplaced, with what is under it
                3 => (Some(format!("x{}", at - 1)), SaidKind::Finding),
                _ => (Some("nowhere".to_owned()), SaidKind::Table),
            };
            answer.nodes.push(node(&id, parent.as_deref(), kind, quotes[at % quotes.len()]));
        }
        let tree = build(&answer, "p", &text()).unwrap();
        assert!(tree.nodes.len() <= MAX_NODES);
        assert_eq!(tree.nodes.len() + tree.left_out.total(), answer.nodes.len(), "{:?}", tree.left_out);
        assert!(tree.left_out.over_bounds >= answer.nodes.len() - MAX_ANSWERED);
        // every node shown stands where its kind may, under a node shown
        for node in &tree.nodes {
            let parent = node.parent.as_deref().map(|id| tree.node(id).unwrap().kind);
            assert!(node.kind.may_stand_under(parent), "{node:?}");
        }
    }

    #[test]
    fn a_chain_of_parents_as_long_as_the_answer_is_refused_only_when_it_closes() {
        let chain = |closed: bool| SaidTree {
            nodes: (0..MAX_ANSWERED)
                .map(|at| {
                    let parent = match at {
                        0 if closed => Some(format!("x{}", MAX_ANSWERED - 1)),
                        0 => None,
                        _ => Some(format!("x{}", at - 1)),
                    };
                    node(&format!("x{at}"), parent.as_deref(), SaidKind::Finding, "keeps every expert busy")
                })
                .collect(),
        };
        assert!(matches!(build(&chain(true), "p", &text()), Err(Malformed::Cycle(_))));
        let open = build(&chain(false), "p", &text()).unwrap();
        assert!(open.nodes.is_empty());
        assert_eq!(open.left_out.total(), MAX_ANSWERED);
    }

    #[test]
    fn the_same_answer_checked_twice_gives_the_same_tree() {
        assert_eq!(build(&said(), "p", &text()), build(&said(), "p", &text()));
    }

    #[test]
    fn the_answers_schema_names_every_field_the_answer_has() {
        let schema = schema();
        let item = &schema["properties"]["nodes"]["items"];
        assert_eq!(item["required"].as_array().unwrap().len(), 6);
        assert_eq!(item["additionalProperties"], false);
        let parsed: SaidTree = serde_json::from_value(serde_json::json!({"nodes": [
            {"id": "c1", "parent": null, "kind": "claim", "statement": "s", "quotes": ["q"], "label": null}]}))
        .unwrap();
        assert_eq!(parsed.nodes[0].kind, SaidKind::Claim);
        assert!(
            serde_json::from_value::<SaidTree>(serde_json::json!({"nodes": [{"id": "c1", "kind": "opinion"}]}))
                .is_err()
        );
    }
}
