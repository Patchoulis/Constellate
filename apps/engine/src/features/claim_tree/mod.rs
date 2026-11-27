//! A paper's claims as a tree, with its reviews mapped onto them: reviewers first, the model's own reading of
//! gaps second (survey section 6, after QED Science's claim tree and eLife's pilot of reviews mapped onto it).
//!
//! | module      | what it does                                                                        |
//! |-------------|-------------------------------------------------------------------------------------|
//! | [`tree`]    | the model's tree checked as a tree, every node's quotes found in the paper          |
//! | [`points`]  | a review's sections cut into points, each at its place in the kept page              |
//! | [`mapping`] | each point placed on a node by the model, its justification found in the point      |
//! | [`asking`]  | the two calls (tree, then mapping), kept apart, over one cached prefix              |
//!
//! What this module adds on top ([`view`]):
//!
//! - **At each node**, which reviewers supported, challenged, asked about it, or spoke of its presentation, by
//!   name; a node any reviewer challenged is *contested*. The reviewers' overall scores are given beside their
//!   names on the venue's own scale ([`crate::analysis::reviews`]), never mixed with another scale.
//! - **At the root**, the paper's rating figures, the meta-review and the decision, and the points about the
//!   paper as a whole.
//! - **Gaps**, the model's reading and shown second: claims and findings with no evidence found under them, and
//!   evidence no reviewer's point was placed on. No score, no verdict.
//!
//! Reviews are only what the library kept from pages the user opened in the reader; nothing here fetches.

pub mod asking;
pub mod mapping;
pub mod points;
pub mod routes;
pub mod tree;

use rayon::prelude::*;
use serde::Serialize;

use self::asking::{Mapping as Planned, Questions, mapping_key, paper_prompt, tree_key};
use self::mapping::{Mapping, SaidMapping, Stance};
use self::points::{Point, ReviewPoints, review_points};
use self::tree::{LeftOut, Malformed, Node, NodeKind, SaidTree, Tree};
use crate::analysis::reviews::{PaperFigures, Reviewed, Scale, paper_figures, venue_of};
use crate::evidence::{Evidence, PaperText};
use crate::library::{AnswerKey, Feature, KeptReviews, Library, LibraryError, PaperReading};
use crate::model::batch::BatchEstimate;
use crate::model::chosen::Chosen;
use crate::model::language::Provider;
use crate::text::pdf::passages::Passages;
use crate::text::reviews::Field;
use crate::text::reviews::discussion::{Basis, Discussion, Kind, Reply, Section, Signer};

/// What the paper's text is, for the tree: the JSON's `paper`, read from [`Claims`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PaperState {
    /// Read from its PDF: `sent` of its `passages` go to the model.
    Read { passages: usize, sent: usize },
    /// No PDF is kept: there is no text to find its claims in.
    NoPdf,
    /// Its PDF could not be read.
    Unreadable,
}

/// Where the paper's claim tree stands: the JSON's `tree`, read from [`Claims`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TreeState {
    /// No tree without the paper's text.
    NoText,
    /// Not asked yet (with this model): the estimate says what it would cost.
    NotAsked,
    /// The model's answer is kept but is not a tree.
    Malformed { why: Malformed, kept_at: String },
    Made {
        nodes: Vec<Node>,
        left_out: LeftOut,
        kept_at: String,
        /// The model that wrote it, as the API named it.
        model: String,
    },
    /// A tree was written, but every node of it was left out (`left_out` says why): nothing stands to be shown or
    /// mapped onto. Not `Made` with no nodes, which would read as a paper with no claims.
    AllLeftOut { left_out: LeftOut, kept_at: String, model: String },
}

/// Where the mapping of the paper's reviews onto its tree stands: the JSON's `mapping`, read from the reviews and
/// [`Claims`] ([`ClaimsView::mapping`]), in this order: no reviews, no points, no tree with nodes, then whether
/// the mapping onto that tree is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MappingState {
    /// No reviews are kept: open the paper's Reviews tab in the reader to read them.
    NoReviews,
    /// Reviews are kept, but none of their sections holds points (a form of scores only).
    NoPoints,
    /// There is no tree to map onto yet.
    NoTree,
    NotAsked,
    Made {
        mapping: Mapping,
        kept_at: String,
        model: String,
    },
}

/// A reviewer's overall score, on the venue's scale for that field and year.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rating {
    pub field: Field,
    pub value: u8,
    pub scale: Scale,
}

/// How a reply was attached to the review it is shown under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Attached {
    /// The reply's own title names the reviewer.
    Named,
    /// A guess, and why.
    Guessed { basis: Basis },
}

/// A reply to a review: an author's rebuttal, the reviewer's answer, a chair's comment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplyView {
    /// Its index among the discussion's notes.
    pub note: usize,
    pub signer: Signer,
    pub heading: String,
    pub attached: Attached,
    pub sections: Vec<Section>,
}

/// One reviewer: their name, overall score and confidence, and the replies to their review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewerView {
    /// The review's index among the notes.
    pub note: usize,
    pub name: String,
    pub overall: Option<Rating>,
    /// On the venue's confidence scale.
    pub confidence: Option<Rating>,
    pub replies: Vec<ReplyView>,
}

/// A meta-review, as the page has it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MetaReview {
    pub signer: Signer,
    pub heading: String,
    pub sections: Vec<Section>,
}

/// The paper's kept reviews, as this view shows them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReviewsView {
    /// The page the reader showed, and when it was read.
    pub address: String,
    pub read_at: String,
    pub reviewers: Vec<ReviewerView>,
    #[serde(flatten)]
    pub points: ReviewPoints,
    pub figures: PaperFigures,
    pub meta_reviews: Vec<MetaReview>,
    pub decision: Option<String>,
}

/// Reviewers by what their points do to one node, each named once per stance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Stances {
    pub supports: Vec<String>,
    pub challenges: Vec<String>,
    pub asks: Vec<String>,
    pub presentation: Vec<String>,
    pub whole_paper: Vec<String>,
}

/// The reviews at one node of the tree, or at the root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeReviews {
    /// The node; `None` for the root (the paper as a whole).
    pub node: Option<String>,
    /// The points placed here, in the page's order.
    pub points: Vec<String>,
    pub reviewers: Stances,
    /// Challenged by at least one reviewer.
    pub contested: bool,
}

/// What the model's reading of the tree finds missing. Shown apart from, and after, what reviewers said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// A claim or finding with no evidence node found under it.
    NoStatedSupport,
    /// Evidence no reviewer's point was placed on.
    NotExamined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Gap {
    pub kind: GapKind,
    pub node: String,
    /// The model's reading, in its words.
    pub statement: String,
    /// The paper's own words for the node: found in the paper.
    pub evidence: Vec<Evidence>,
}

/// Why there is no text to find a paper's claims in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoText {
    NoPdf,
    Unreadable,
}

/// The mapping of the reviews' points onto a tree, as kept and checked again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingKept {
    pub mapping: Mapping,
    pub kept_at: String,
    pub model: String,
}

/// What is kept of the paper's tree with the chosen model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeKept {
    NotAsked,
    /// The model's answer is kept but is not a tree.
    Malformed {
        why: Malformed,
        kept_at: String,
    },
    /// A tree, and the mapping of the reviews onto it when that is kept: a mapping exists only on a tree.
    Made {
        tree: Tree,
        kept_at: String,
        model: String,
        mapping: Option<MappingKept>,
    },
}

/// The paper's claims, by what its text is: a tree and an estimate only with the text, a mapping only inside a
/// tree. The JSON's `paper`, `tree`, `mapping`, `node_reviews`, `gaps` and `estimate` are all read from this and
/// the reviews ([`ClaimsView`]'s methods), so that no combination of them that cannot be right (a tree made of a
/// paper with no PDF, a mapping made with no tree, an estimate with no text) can be told.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // one per request, never in a collection: boxing would buy nothing
pub enum Claims {
    NoText(NoText),
    Read {
        passages: usize,
        /// Of them, those sent to the model.
        sent: usize,
        tree: TreeKept,
        /// What asking what is not kept would cost.
        estimate: BatchEstimate,
    },
}

/// Everything the Claims view of one paper shows: `ClaimsView` in the contracts, which it is serialised as.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimsView {
    pub paper_id: String,
    /// The model chosen in the settings, and its provider: what a new tree would be asked of.
    pub provider: Provider,
    pub model: String,
    pub reviews: Option<ReviewsView>,
    pub claims: Claims,
}

impl ClaimsView {
    pub fn paper(&self) -> PaperState {
        match &self.claims {
            Claims::NoText(NoText::NoPdf) => PaperState::NoPdf,
            Claims::NoText(NoText::Unreadable) => PaperState::Unreadable,
            Claims::Read { passages, sent, .. } => PaperState::Read { passages: *passages, sent: *sent },
        }
    }

    pub fn tree(&self) -> TreeState {
        match &self.claims {
            Claims::NoText(_) => TreeState::NoText,
            Claims::Read { tree: TreeKept::NotAsked, .. } => TreeState::NotAsked,
            Claims::Read { tree: TreeKept::Malformed { why, kept_at }, .. } => {
                TreeState::Malformed { why: why.clone(), kept_at: kept_at.clone() }
            }
            Claims::Read { tree: TreeKept::Made { tree, kept_at, model, .. }, .. } if tree.nodes.is_empty() => {
                TreeState::AllLeftOut { left_out: tree.left_out, kept_at: kept_at.clone(), model: model.clone() }
            }
            Claims::Read { tree: TreeKept::Made { tree, kept_at, model, .. }, .. } => TreeState::Made {
                nodes: tree.nodes.clone(),
                left_out: tree.left_out,
                kept_at: kept_at.clone(),
                model: model.clone(),
            },
        }
    }

    /// The points of the kept reviews; none without reviews.
    pub fn points(&self) -> &[Point] {
        self.reviews.as_ref().map_or(&[], |reviews| &reviews.points.points)
    }

    /// The tree when it is made, with the mapping onto it when that is kept and there are points to show it with.
    fn made(&self) -> Option<(&Tree, Option<&MappingKept>)> {
        let Claims::Read { tree: TreeKept::Made { tree, mapping, .. }, .. } = &self.claims else { return None };
        let mapped = !tree.nodes.is_empty() && !self.points().is_empty();
        Some((tree, mapping.as_ref().filter(|_| mapped)))
    }

    pub fn mapping(&self) -> MappingState {
        match &self.reviews {
            None => return MappingState::NoReviews,
            Some(_) if self.points().is_empty() => return MappingState::NoPoints,
            Some(_) => {}
        }
        match self.made() {
            Some((tree, _)) if tree.nodes.is_empty() => MappingState::NoTree,
            Some((_, Some(kept))) => MappingState::Made {
                mapping: kept.mapping.clone(),
                kept_at: kept.kept_at.clone(),
                model: kept.model.clone(),
            },
            Some((_, None)) => MappingState::NotAsked,
            None => MappingState::NoTree,
        }
    }

    /// The root first, then one per node with points placed on it; nothing without a mapping.
    pub fn node_reviews(&self) -> Vec<NodeReviews> {
        match self.made() {
            Some((tree, Some(kept))) => node_reviews(tree, &kept.mapping, self.points()),
            _ => Vec::new(),
        }
    }

    /// Nodes a reviewer challenged.
    pub fn contested(&self) -> usize {
        contested_of(&self.node_reviews())
    }

    pub fn gaps(&self) -> Vec<Gap> {
        self.made().map(|(tree, kept)| gaps(tree, kept.map(|kept| &kept.mapping))).unwrap_or_default()
    }

    /// What asking what is not kept would cost; `None` when there is nothing that can be asked (no text).
    pub fn estimate(&self) -> Option<&BatchEstimate> {
        match &self.claims {
            Claims::Read { estimate, .. } => Some(estimate),
            Claims::NoText(_) => None,
        }
    }
}

fn contested_of(node_reviews: &[NodeReviews]) -> usize {
    node_reviews.iter().filter(|node| node.node.is_some() && node.contested).count()
}

impl Serialize for ClaimsView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Told<'a> {
            paper_id: &'a str,
            provider: Provider,
            model: &'a str,
            paper: PaperState,
            tree: TreeState,
            reviews: Option<&'a ReviewsView>,
            mapping: MappingState,
            node_reviews: Vec<NodeReviews>,
            contested: usize,
            gaps: Vec<Gap>,
            estimate: Option<&'a BatchEstimate>,
        }
        let node_reviews = self.node_reviews();
        Told {
            paper_id: &self.paper_id,
            provider: self.provider,
            model: &self.model,
            paper: self.paper(),
            tree: self.tree(),
            reviews: self.reviews.as_ref(),
            mapping: self.mapping(),
            contested: contested_of(&node_reviews),
            node_reviews,
            gaps: self.gaps(),
            estimate: self.estimate(),
        }
        .serialize(serializer)
    }
}

fn reply_of(reply: &Reply) -> Option<(usize, Attached)> {
    match reply {
        Reply::Named { note } => Some((*note, Attached::Named)),
        Reply::Guessed { note, basis } => Some((*note, Attached::Guessed { basis: *basis })),
        Reply::Submission | Reply::NotKnown => None,
    }
}

/// The kept reviews as shown: reviewers with their scores and replies, points, figures, meta-review, decision.
pub fn reviews_view(kept: &KeptReviews) -> ReviewsView {
    let discussion: &Discussion = &kept.discussion;
    let venue = venue_of(&kept.paper_id, discussion);
    let figures = paper_figures(&Reviewed::new(&kept.paper_id, venue.as_ref(), discussion));
    let overall = figures.overall().map(|overall| (overall.field, overall.scale));
    let confidence_scale =
        figures.fields.iter().find(|field| field.field == Field::Confidence).map(|field| field.scale);
    let reviewers = points::reviewer_names(discussion)
        .into_iter()
        .map(|(note, name)| {
            let (scores, confidence) = match &discussion.notes[note].kind {
                Kind::OfficialReview { scores, confidence } => (scores.as_slice(), confidence.as_ref()),
                _ => (&[][..], None),
            };
            let overall = overall.and_then(|(field, scale)| {
                scores.iter().find(|score| score.field == field).map(|score| Rating {
                    field,
                    value: score.value,
                    scale,
                })
            });
            let confidence = confidence.zip(confidence_scale).map(|(score, scale)| Rating {
                field: Field::Confidence,
                value: score.value,
                scale,
            });
            let replies = discussion
                .notes
                .iter()
                .enumerate()
                .filter_map(|(at, reply)| {
                    let (answers, attached) = reply_of(&reply.reply)?;
                    (answers == note).then(|| ReplyView {
                        note: at,
                        signer: reply.signer.clone(),
                        heading: reply.heading.clone(),
                        attached,
                        sections: reply.sections.clone(),
                    })
                })
                .collect();
            ReviewerView { note, name, overall, confidence, replies }
        })
        .collect();
    let meta_reviews = discussion
        .notes
        .iter()
        .filter(|note| note.kind == Kind::MetaReview)
        .map(|note| MetaReview {
            signer: note.signer.clone(),
            heading: note.heading.clone(),
            sections: note.sections.clone(),
        })
        .collect();
    let decision = discussion.notes.iter().find_map(|note| match &note.kind {
        Kind::Decision { decision } if !decision.is_empty() => Some(decision.clone()),
        _ => None,
    });
    ReviewsView {
        address: kept.address.clone(),
        read_at: kept.read_at.clone(),
        reviewers,
        points: review_points(discussion),
        figures,
        meta_reviews,
        decision,
    }
}

fn add(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|known| known == name) {
        names.push(name.to_owned());
    }
}

/// The reviews at the root and at each node points were placed on, in the tree's order.
pub fn node_reviews(tree: &Tree, mapping: &Mapping, points: &[Point]) -> Vec<NodeReviews> {
    let targets = std::iter::once(None).chain(tree.nodes.iter().map(|node| Some(node.id.as_str())));
    targets
        .filter_map(|target| {
            let mut here = NodeReviews {
                node: target.map(str::to_owned),
                points: Vec::new(),
                reviewers: Stances::default(),
                contested: false,
            };
            for placement in mapping.placed.iter().filter(|placement| placement.node.as_deref() == target) {
                let Some(point) = points.iter().find(|point| point.id == placement.point) else { continue };
                here.points.push(point.id.clone());
                let names = match placement.stance {
                    Stance::Supports => &mut here.reviewers.supports,
                    Stance::Challenges => &mut here.reviewers.challenges,
                    Stance::Asks => &mut here.reviewers.asks,
                    Stance::Presentation => &mut here.reviewers.presentation,
                    Stance::WholePaper => &mut here.reviewers.whole_paper,
                };
                add(names, &point.reviewer);
            }
            here.contested = !here.reviewers.challenges.is_empty();
            (target.is_none() || !here.points.is_empty()).then_some(here)
        })
        .collect()
}

/// The model's reading of what is missing: claims and findings with no evidence under them; with reviews mapped,
/// evidence no reviewer's point was placed on.
pub fn gaps(tree: &Tree, mapping: Option<&Mapping>) -> Vec<Gap> {
    tree.nodes
        .iter()
        .filter_map(|node| {
            let kind = match node.kind {
                NodeKind::Claim | NodeKind::Finding if !tree.has_evidence_under(&node.id) => GapKind::NoStatedSupport,
                NodeKind::Evidence(_)
                    if mapping.is_some_and(|mapping| {
                        !mapping.placed.iter().any(|placement| placement.node.as_deref() == Some(node.id.as_str()))
                    }) =>
                {
                    GapKind::NotExamined
                }
                _ => return None,
            };
            Some(Gap {
                kind,
                node: node.id.clone(),
                statement: node.statement.clone(),
                evidence: node.evidence.clone(),
            })
        })
        .collect()
}

/// The kept tree of `paper`, checked against its passages; its mapping is not read here.
fn kept_tree(library: &Library, paper: &str, chosen: &Chosen, passages: &Passages) -> Result<TreeKept, LibraryError> {
    let Some(kept) = library.kept::<SaidTree>(Feature::ClaimTree, &tree_key(chosen, paper))? else {
        return Ok(TreeKept::NotAsked);
    };
    Ok(match tree::build(&kept.answer, paper, &PaperText::Read(passages.clone())) {
        Ok(tree) => TreeKept::Made { tree, kept_at: kept.kept_at, model: kept.model, mapping: None },
        Err(why) => TreeKept::Malformed { why, kept_at: kept.kept_at },
    })
}

/// The mapping's question for `tree` and `points`, when there is a mapping to ask: a tree with nodes, and points.
fn mapping_question(tree: &Tree, points: &[Point]) -> Option<String> {
    (!tree.nodes.is_empty() && !points.is_empty()).then(|| mapping::question(tree, points))
}

/// The claims of a paper whose text was read: the kept tree and mapping, checked again, and the estimate.
fn read_claims(
    library: &Library,
    paper: &str,
    chosen: &Chosen,
    passages: &Passages,
    points: &[Point],
) -> Result<Claims, LibraryError> {
    let prompt = paper_prompt(passages);
    let (tree, question) = match kept_tree(library, paper, chosen, passages)? {
        TreeKept::Made { tree, kept_at, model, .. } => {
            let question = mapping_question(&tree, points);
            let mapping = match &question {
                Some(question) => library
                    .kept::<SaidMapping>(Feature::ClaimTree, &mapping_key(chosen, paper, question))?
                    .map(|kept| MappingKept {
                        mapping: mapping::place(&kept.answer, &tree, points),
                        kept_at: kept.kept_at,
                        model: kept.model,
                    }),
                None => None,
            };
            (TreeKept::Made { tree, kept_at, model, mapping }, question)
        }
        other => (other, None),
    };
    let (tree_schema, mapping_schema) = (tree::schema(), mapping::schema());
    let questions =
        Questions { chosen, paper, shared: &prompt.text, tree_schema: &tree_schema, mapping_schema: &mapping_schema };
    let planned = match (&question, &tree) {
        (Some(question), _) => Planned::Known(question, points.len()),
        (None, TreeKept::NotAsked) if !points.is_empty() => Planned::Guessed(points),
        _ => Planned::Nothing,
    };
    let estimate = asking::estimate(library, &questions, planned)?;
    Ok(Claims::Read { passages: prompt.passages, sent: prompt.sent, tree, estimate })
}

/// The Claims view of `paper` with `model`: what is kept, checked again against the paper and the reviews, and
/// what asking the rest would cost. Asks nothing. `None` when the paper is not in the library.
pub fn view(library: &Library, paper: &str, chosen: &Chosen) -> Result<Option<ClaimsView>, LibraryError> {
    let Some(reading) = library.paper_texts(&[paper])?.pop() else { return Ok(None) };
    let text = match reading {
        PaperReading::NotInLibrary => return Ok(None),
        PaperReading::Read(passages) => Ok(passages),
        PaperReading::NoPdf { .. } => Err(NoText::NoPdf),
        PaperReading::Unreadable { .. } => Err(NoText::Unreadable),
    };
    let reviews = library.reviews(paper)?.map(|kept| reviews_view(&kept));
    let points: &[Point] = reviews.as_ref().map_or(&[], |reviews| &reviews.points.points);
    let claims = match text {
        Ok(passages) => read_claims(library, paper, chosen, &passages, points)?,
        Err(why) => Claims::NoText(why),
    };
    Ok(Some(ClaimsView {
        paper_id: paper.to_owned(),
        provider: chosen.provider(),
        model: chosen.model().to_owned(),
        reviews,
        claims,
    }))
}

/// Forget what the Claims view of `paper` shows with `chosen` (a redo): its tree, and the mapping of the reviews
/// onto that tree. What other models wrote about the paper stays: the view does not show it, and it was paid for.
/// The mapping goes first, so that a stop between the two never leaves a mapping that a tree asked again (and
/// answered the same) would find. Returns how many answers were forgotten.
pub fn forget(library: &Library, paper: &str, chosen: &Chosen) -> Result<usize, LibraryError> {
    let mut forgotten = 0;
    if let Some(key) = shown_mapping_key(library, paper, chosen)? {
        forgotten += usize::from(library.forget_key(Feature::ClaimTree, &key)?);
    }
    forgotten += usize::from(library.forget_key(Feature::ClaimTree, &tree_key(chosen, paper))?);
    Ok(forgotten)
}

/// The key of the mapping the view would show: of the kept tree, checked again, and the kept reviews' points.
fn shown_mapping_key(library: &Library, paper: &str, chosen: &Chosen) -> Result<Option<AnswerKey>, LibraryError> {
    let Some(PaperReading::Read(passages)) = library.paper_texts(&[paper])?.pop() else { return Ok(None) };
    let Some(kept) = library.kept::<SaidTree>(Feature::ClaimTree, &tree_key(chosen, paper))? else { return Ok(None) };
    let Ok(tree) = tree::build(&kept.answer, paper, &PaperText::Read(passages)) else { return Ok(None) };
    let Some(reviews) = library.reviews(paper)? else { return Ok(None) };
    let points = review_points(&reviews.discussion).points;
    Ok(mapping_question(&tree, &points).map(|question| mapping_key(chosen, paper, &question)))
}

/// How many of one paper's claims its reviewers contested, from what is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PaperContested {
    pub paper_id: String,
    /// Nodes of its tree a reviewer challenged.
    pub contested: usize,
    pub nodes: usize,
    pub reviewers: usize,
}

/// For a community's reviews section: per paper whose tree and mapping are kept, its contested claims.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Contested {
    pub papers: usize,
    pub papers_with_reviews: usize,
    /// Of them, whose reviews were mapped onto a kept tree with this model.
    pub papers_mapped: usize,
    /// By paper id.
    pub counts: Vec<PaperContested>,
}

/// A paper's kept reviews, its checked tree, its points and the mapping's question.
type Built<'a> = (&'a KeptReviews, Tree, Vec<Point>, String);

/// [`Contested`] for `papers`, from kept answers of `model` only: nothing is asked. The trees are checked on every
/// core; papers come out ordered by id whatever the number of threads.
pub fn contested(library: &Library, papers: &[String], chosen: &Chosen) -> Result<Contested, LibraryError> {
    contested_on(library, papers, chosen, None)
}

/// [`contested`], the trees checked on `pool` when one is given (the tests' way of trying thread counts).
fn contested_on(
    library: &Library,
    papers: &[String],
    chosen: &Chosen,
    pool: Option<&rayon::ThreadPool>,
) -> Result<Contested, LibraryError> {
    let ids: Vec<&str> = papers.iter().map(String::as_str).collect();
    let reviewed = library.reviews_of(&ids)?;
    let mut kept_trees: Vec<(&KeptReviews, SaidTree)> = Vec::new();
    for kept in &reviewed {
        if let Some(tree) = library.kept::<SaidTree>(Feature::ClaimTree, &tree_key(chosen, &kept.paper_id))? {
            kept_trees.push((kept, tree.answer));
        }
    }
    // read outside the pool: the library reads PDFs on every core itself
    let with_trees: Vec<&str> = kept_trees.iter().map(|(kept, _)| kept.paper_id.as_str()).collect();
    let readings = library.paper_texts(&with_trees)?;
    let build = || -> Vec<Option<Built<'_>>> {
        kept_trees
            .par_iter()
            .zip(readings.par_iter())
            .map(|((kept, said), reading)| {
                let passages = reading.passages()?;
                let tree = tree::build(said, &kept.paper_id, &PaperText::Read(passages.clone())).ok()?;
                let points = review_points(&kept.discussion).points;
                let question = mapping_question(&tree, &points)?;
                Some((*kept, tree, points, question))
            })
            .collect()
    };
    let built = match pool {
        Some(pool) => pool.install(build),
        None => build(),
    };
    let mut counts = Vec::new();
    for (kept, tree, points, question) in built.into_iter().flatten() {
        let Some(said) =
            library.kept::<SaidMapping>(Feature::ClaimTree, &mapping_key(chosen, &kept.paper_id, &question))?
        else {
            continue;
        };
        let mapping = mapping::place(&said.answer, &tree, &points);
        let contested = contested_of(&node_reviews(&tree, &mapping, &points));
        counts.push(PaperContested {
            paper_id: kept.paper_id.clone(),
            contested,
            nodes: tree.nodes.len(),
            reviewers: kept.discussion.review_count(),
        });
    }
    Ok(Contested { papers: papers.len(), papers_with_reviews: reviewed.len(), papers_mapped: counts.len(), counts })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use serde_json::{Value, json};

    use super::asking::tests::{MODEL, PAPER, library_with, mapping_answer, reply, tree_answer};
    use super::asking::{Askers, ask};
    use super::*;
    use crate::jobs::{Cancel, Progress, Tracker};
    use crate::model::language::fake::Fake;
    use crate::model::language::{Budget, Endpoint, KEY_VARIABLE, Limit};
    use crate::usage::{Call, Usd};

    fn asked(library: &Library, answers: &[serde_json::Value]) {
        let fake = Fake::new(answers.iter().map(reply).collect());
        let calls: Mutex<Vec<Call>> = Mutex::new(Vec::new());
        let cancel = Cancel::new();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});
        let tracker = Tracker::new(&log, &progress, &cancel);
        let endpoint =
            Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap();
        let askers = Askers { library, transport: &fake, endpoint, recorder: &calls };
        let budget = Budget::new(Limit::Dollars(Usd::from_picodollars(5_000_000_000_000)));
        ask(&askers, PAPER, &Chosen::claude(MODEL), &budget, &tracker).unwrap();
    }

    fn made(view: &ClaimsView) -> (Vec<Node>, LeftOut) {
        match view.tree() {
            TreeState::Made { nodes, left_out, .. } => (nodes, left_out),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reviewers_are_mapped_onto_the_tree_with_who_challenged_and_supported_each_node() {
        let (_folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        let view = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        let (nodes, left_out) = made(&view);
        assert_eq!(nodes.len(), 4);
        assert_eq!(left_out.unquoted, 1, "the theorem's made-up quote was not found: left out and counted");
        let MappingState::Made { mapping, .. } = view.mapping() else { panic!("{:?}", view.mapping()) };
        assert_eq!(mapping.placed.len(), 4);
        // p5's words are not in it: unplaced, and listed
        assert_eq!(mapping.unplaced.len(), 1);
        assert_eq!(mapping.unplaced[0].point, "p5");
        let claim = view.node_reviews().into_iter().find(|node| node.node.as_deref() == Some("n1")).unwrap();
        assert_eq!(claim.reviewers.challenges, ["Reviewer Ab12", "Reviewer Cd34"]);
        assert!(claim.contested);
        let ablation = view.node_reviews().into_iter().find(|node| node.node.as_deref() == Some("n4")).unwrap();
        assert_eq!(
            (ablation.reviewers.supports.as_slice(), ablation.contested),
            (&["Reviewer Ab12".to_owned()][..], false)
        );
        let root = &view.node_reviews()[0];
        assert_eq!(
            (root.node.as_deref(), root.reviewers.presentation.as_slice()),
            (None, &["Reviewer Cd34".to_owned()][..])
        );
        assert_eq!(view.contested(), 1);
        // the reviewers' scores, on the venue's own scale
        let reviews = view.reviews.as_ref().unwrap();
        let first = &reviews.reviewers[0];
        assert_eq!(first.name, "Reviewer Ab12");
        let rating = first.overall.as_ref().unwrap();
        assert_eq!((rating.field, rating.value, rating.scale.minimum, rating.scale.maximum), (Field::Rating, 6, 1, 10));
        assert_eq!(reviews.figures.reviews, 2);
        assert_eq!(reviews.decision.as_deref(), Some("Accept (poster)"));
        assert_eq!(reviews.meta_reviews.len(), 1);
        // the authors' reply, named by its title
        assert_eq!(first.replies.len(), 1);
        assert_eq!(first.replies[0].attached, Attached::Named);
        assert_eq!(view.estimate().unwrap().to_ask, 0);
    }

    #[test]
    fn gaps_are_the_models_reading_of_what_has_no_support_or_was_not_examined() {
        let (_folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        let view = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        let gaps: Vec<(GapKind, String)> = view.gaps().into_iter().map(|gap| (gap.kind, gap.node)).collect();
        // the experiment (n3) was examined by no reviewer; every claim and finding has evidence under it
        assert_eq!(gaps, [(GapKind::NotExamined, "n3".to_owned())]);
        assert!(view.gaps()[0].evidence[0].address().is_some());
    }

    #[test]
    fn a_paper_with_reviews_and_no_pdf_has_no_tree_and_its_reviews_as_stored() {
        let (_folder, library) = library_with(false, true);
        let view = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        assert_eq!((view.paper(), view.tree()), (PaperState::NoPdf, TreeState::NoText));
        assert_eq!(view.mapping(), MappingState::NoTree);
        assert_eq!(view.reviews.as_ref().unwrap().points.points.len(), 5);
        assert_eq!(view.estimate(), None);
    }

    #[test]
    fn a_paper_with_a_pdf_and_no_reviews_has_the_tree_alone() {
        let (_folder, library) = library_with(true, false);
        let before = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        assert_eq!((before.tree(), before.mapping()), (TreeState::NotAsked, MappingState::NoReviews));
        assert_eq!(before.estimate().unwrap().to_ask, 1);
        asked(&library, &[tree_answer()]);
        let after = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        assert_eq!(made(&after).0.len(), 4);
        assert_eq!(after.mapping(), MappingState::NoReviews);
        // with no reviews, nothing is "not examined": only claims without support would be gaps
        assert!(after.gaps().is_empty());
    }

    /// The view of the paper with its tree and mapping asked, as kept.
    fn mapped() -> (tempfile::TempDir, ClaimsView) {
        let (folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        let view = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        (folder, view)
    }

    #[test]
    fn a_kept_mapping_is_not_told_when_the_reviews_are_gone_or_hold_no_points() {
        let (_folder, mut view) = mapped();
        assert!(matches!(view.mapping(), MappingState::Made { .. }));
        let reviews = view.reviews.take();
        assert_eq!(view.mapping(), MappingState::NoReviews);
        assert!(view.node_reviews().is_empty() && view.contested() == 0);
        assert!(view.gaps().iter().all(|gap| gap.kind != GapKind::NotExamined), "nothing was examined by no one");
        let mut pointless = reviews.unwrap();
        pointless.points.points.clear();
        view.reviews = Some(pointless);
        assert_eq!(view.mapping(), MappingState::NoPoints);
        assert!(view.node_reviews().is_empty());
    }

    #[test]
    fn a_tree_left_with_no_nodes_has_no_tree_to_map_onto_whatever_is_kept() {
        let (_folder, mut view) = mapped();
        let Claims::Read { tree: TreeKept::Made { tree, .. }, .. } = &mut view.claims else { panic!() };
        tree.nodes.clear();
        assert!(matches!(view.tree(), TreeState::AllLeftOut { .. }), "its own state, not a tree of no claims");
        assert_eq!(view.mapping(), MappingState::NoTree);
        assert!(view.node_reviews().is_empty() && view.gaps().is_empty());
    }

    #[test]
    fn a_paper_with_no_text_tells_no_tree_no_estimate_and_its_reviews_points_state() {
        let (_folder, mut view) = mapped();
        view.claims = Claims::NoText(NoText::Unreadable);
        assert_eq!((view.paper(), view.tree(), view.estimate()), (PaperState::Unreadable, TreeState::NoText, None));
        assert_eq!(view.mapping(), MappingState::NoTree);
        view.reviews.as_mut().unwrap().points.points.clear();
        assert_eq!(view.mapping(), MappingState::NoPoints, "as with a PDF: no points comes before no tree");
        let told = serde_json::to_value(&view).unwrap();
        assert_eq!((told["tree"]["state"].clone(), told["estimate"].clone()), (json!("no_text"), Value::Null));
    }

    #[test]
    fn a_tree_not_asked_or_malformed_has_no_mapping_and_no_gaps() {
        let (_folder, mut view) = mapped();
        let estimate = view.estimate().unwrap().clone();
        let malformed = TreeKept::Malformed { why: Malformed::Empty, kept_at: "then".to_owned() };
        for tree in [TreeKept::NotAsked, malformed] {
            view.claims = Claims::Read { passages: 9, sent: 8, tree, estimate: estimate.clone() };
            assert_eq!(view.mapping(), MappingState::NoTree);
            assert!(view.node_reviews().is_empty() && view.gaps().is_empty());
            assert_eq!(view.paper(), PaperState::Read { passages: 9, sent: 8 });
        }
    }

    #[test]
    fn a_redo_forgets_the_shown_models_tree_and_mapping_and_keeps_another_models() {
        let (_folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        let other = Chosen::claude("claude-opus-5-5");
        let fake = Fake::new(vec![reply(&tree_answer()), reply(&mapping_answer())]);
        let calls: Mutex<Vec<Call>> = Mutex::new(Vec::new());
        let cancel = Cancel::new();
        let (log, progress) = (|_: &str| {}, |_: Progress| {});
        let tracker = Tracker::new(&log, &progress, &cancel);
        let endpoint =
            Endpoint::from_lookup(|name| (name == KEY_VARIABLE).then(|| "test-key-not-real".to_owned())).unwrap();
        let askers = Askers { library: &library, transport: &fake, endpoint, recorder: &calls };
        let budget = Budget::new(Limit::Dollars(Usd::from_picodollars(5_000_000_000_000)));
        ask(&askers, PAPER, &other, &budget, &tracker).unwrap();

        assert_eq!(forget(&library, PAPER, &Chosen::claude(MODEL)).unwrap(), 2, "its tree and its mapping");
        let shown = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        assert_eq!((shown.tree(), shown.mapping()), (TreeState::NotAsked, MappingState::NoTree));
        let kept = view(&library, PAPER, &other).unwrap().unwrap();
        assert!(matches!(kept.mapping(), MappingState::Made { .. }), "the other model's answers were paid for");
        assert_eq!(forget(&library, PAPER, &Chosen::claude(MODEL)).unwrap(), 0, "nothing left to forget");
    }

    #[test]
    fn a_redone_tree_answered_the_same_does_not_find_the_forgotten_mapping() {
        let (_folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        forget(&library, PAPER, &Chosen::claude(MODEL)).unwrap();
        // the tree asked again, and answered the same; the mapping's call gets no answer
        asked(&library, &[tree_answer()]);
        let view = view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap();
        assert!(matches!(view.tree(), TreeState::Made { .. }));
        assert_eq!(view.mapping(), MappingState::NotAsked, "the forgotten placements are not shown again");
    }

    #[test]
    fn another_model_has_its_own_tree_and_the_paper_must_be_in_the_library() {
        let (_folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        assert_eq!(
            view(&library, PAPER, &Chosen::claude("claude-opus-5-5")).unwrap().unwrap().tree(),
            TreeState::NotAsked
        );
        assert_eq!(view(&library, "nowhere", &Chosen::claude(MODEL)).unwrap(), None);
    }

    #[test]
    fn contested_claims_are_counted_per_paper_the_same_on_one_thread_or_many() {
        let (_folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        let papers = vec![PAPER.to_owned(), "not-reviewed".to_owned()];
        let on = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            contested_on(&library, &papers, &Chosen::claude(MODEL), Some(&pool)).unwrap()
        };
        let one = on(1);
        assert_eq!(one, on(4));
        assert_eq!((one.papers, one.papers_with_reviews, one.papers_mapped), (2, 1, 1));
        assert_eq!(one.counts, [PaperContested { paper_id: PAPER.to_owned(), contested: 1, nodes: 4, reviewers: 2 }]);
    }

    #[test]
    fn the_view_is_told_to_the_interface_in_the_contracts_shape() {
        let (_folder, library) = library_with(true, true);
        asked(&library, &[tree_answer(), mapping_answer()]);
        let told = serde_json::to_value(view(&library, PAPER, &Chosen::claude(MODEL)).unwrap().unwrap()).unwrap();
        assert_eq!(told["tree"]["state"], "made");
        assert_eq!(told["tree"]["nodes"][0]["kind"], "claim");
        assert_eq!(told["tree"]["nodes"][2]["kind"], "evidence");
        assert_eq!(told["tree"]["nodes"][2]["evidence_kind"], "experiment");
        assert_eq!(told["tree"]["nodes"][0]["evidence"][0]["established"]["kind"], "verified");
        assert_eq!(told["mapping"]["state"], "made");
        assert_eq!(told["mapping"]["mapping"]["placed"][0]["stance"], "supports");
        assert_eq!(told["reviews"]["points"][0]["kind"], "strength");
        assert_eq!(told["reviews"]["reviewers"][0]["overall"]["scale"]["maximum"], 10);
        assert_eq!(told["reviews"]["reviewers"][0]["replies"][0]["attached"], serde_json::json!({"by": "named"}));
        assert_eq!(told["paper"]["state"], "read");
        assert_eq!(
            (told["estimate"]["provider"].clone(), told["provider"].clone()),
            (serde_json::json!("anthropic"), serde_json::json!("anthropic"))
        );
        assert!(
            crate::model::language::strict_ready(&tree::schema())
                && crate::model::language::strict_ready(&mapping::schema())
        );
    }
}
