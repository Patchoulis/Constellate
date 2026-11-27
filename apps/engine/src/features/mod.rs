//! The model features: what the reader and the community view offer beyond the run itself, most of them asking
//! the language model, each one whole in its own folder.
//!
//! | feature               | what it does                                                                    |
//! |-----------------------|---------------------------------------------------------------------------------|
//! | [`critique`]          | a paper's weaknesses: the reviewers' points, then a model's reading            |
//! | [`stances`]           | where a community's papers stand on a yes/no question                          |
//! | [`extraction`]        | a table of one answer per paper and question, each with the paper's words      |
//! | [`explanation`]       | "explain this": a passage explained, the paper's definitions, related passages |
//! | [`claim_tree`]        | a paper's claims as a tree, with its reviewers' points mapped onto them        |
//! | [`citation_contexts`] | the sentences where one paper of the library cites another, and their kinds    |
//! | [`answers`]           | questions asked of a run or a community, answered with cited statements        |
//! | [`writing`]           | communities' names and descriptions, and a community's overview                |
//! | [`detector`]          | the AI-text estimate: which abstracts read the way language models write       |
//!
//! # One folder per feature
//!
//! A feature's folder holds all of it, and its files are named the same way in every folder:
//!
//! * `mod.rs`: what the feature is, in its doc comment, and the feature's own logic when it fits in one file;
//! * `routes.rs`: its HTTP routes, with a `table()` that [`crate::api::routes::table`] extends with, one line
//!   per feature;
//! * `store.rs`: what it keeps in the library beyond the model's answers (those are kept for every feature by
//!   [`crate::library::money::answers`]), written as `impl Library` blocks;
//! * any other file is a part of its logic, named for what it does (`kinds.rs`, `retrieval.rs`, `tree.rs`).
//!
//! A new feature is one more folder here, one `pub mod` line below, and one `table.extend(...)` line in
//! [`crate::api::routes::table`].

pub mod answers;
pub mod citation_contexts;
pub mod claim_tree;
pub mod critique;
pub mod detector;
pub mod explanation;
pub mod extraction;
pub mod stances;
pub mod writing;
