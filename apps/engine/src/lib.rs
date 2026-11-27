//! Constellate's engine: everything the app does that is not drawing it. It runs on the user's own computer as
//! part of the desktop app.
//!
//! The code is laid out by what it is about, one folder per subject:
//!
//! | module      | what lives there                                                              |
//! |-------------|-------------------------------------------------------------------------------|
//! | [`analysis`] | the similarity graph over papers, who cites whom, and what is worked out from them |
//! | [`library`] | what is kept on disk: the file store everything is written through             |
//! | [`jobs`]    | the one background job at a time, as an explicit state machine                  |
//! | [`model`]   | the embedding model, on any graphics card or the processor                     |
//! | [`selection`] | which papers a run is about: conferences, or a date range                     |
//! | [`sources`] | what is asked of outside services: cited works looked up on arXiv              |
//! | [`text`]    | what is read out of text: authors' declarations, reviewers' scores, references |
//!
//! Still to come as the Python backend (`apps/backend`) is ported: `sources` (arXiv, conference lists,
//! proceedings), `analysis` (similarity graph, clustering, reports), the AI-text estimate and `api` (what the interface asks for, answered only on this computer and only
//! to the app itself).

pub mod analysis;
pub mod jobs;
pub mod library;
pub mod model;
pub mod selection;
pub mod sources;
pub mod text;
