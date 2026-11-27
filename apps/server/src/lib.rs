//! Constellate's backend.
//!
//! The code is laid out by what it is about, one folder per subject:
//!
//! | module      | what lives there                                                              |
//! |-------------|-------------------------------------------------------------------------------|
//! | [`library`] | what is kept on disk: the file store everything is written through             |
//! | [`jobs`]    | the one background job at a time, as an explicit state machine                  |
//! | [`model`]   | the embedding model, on any graphics card or the processor                     |
//! | [`selection`] | which papers a run is about: conferences, or a date range                     |
//!
//! Still to come as the Python backend (`apps/backend`) is ported: `sources` (arXiv, conference lists,
//! proceedings), `analysis` (similarity graph, clustering, reports), `text` (authors' declarations, reviewers'
//! ratings, the AI-text estimate) and `api` (the HTTP routes).

pub mod jobs;
pub mod library;
pub mod model;
pub mod selection;
