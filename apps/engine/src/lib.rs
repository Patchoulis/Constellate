//! Constellate's engine: everything the app does that is not drawing it. It runs on the user's own computer as
//! part of the desktop app.
//!
//! The code is laid out by what it is about, one folder per subject:
//!
//! | module      | what lives there                                                              |
//! |-------------|-------------------------------------------------------------------------------|
//! | [`analysis`] | the similarity graph over papers, who cites whom, and what is worked out from them |
//! | [`api`]     | what the interface asks, over HTTP, answered only to the app itself            |
//! | [`evidence`] | "this statement rests on that place in that paper": quotes checked before they are shown |
//! | [`library`] | what is kept on disk: the SQLite library, and the file store for a run's results |
//! | [`jobs`]    | the one background job at a time, as an explicit state machine                  |
//! | [`model`]   | the embedding model on any graphics card or the processor; Claude over its API  |
//! | [`runs`]    | many runs: the list without loading any, a cache by use, warm-up at start      |
//! | [`usage`]   | what calls to the language model cost, from prices built into the engine       |
//! | [`selection`] | which papers a run is about: conferences, or a date range                     |
//! | [`settings`] | the person's settings and how a change to them is checked                     |
//! | [`sources`] | where papers come from: conference lists, arXiv, proceedings' PDFs             |
//! | [`text`]    | what is read out of text: passages, authors' declarations, reviewers' scores, references |
//!
//! Still to come as the Python backend (`apps/backend`) is ported: the pipeline that runs these in order, reports,
//! the AI-text estimate, the language model's descriptions, and the rest of `api`'s routes.

pub mod analysis;
pub mod api;
pub mod evidence;
pub mod explanation;
pub mod jobs;
pub mod library;
pub mod model;
pub mod runs;
pub mod selection;
pub mod settings;
pub mod sources;
pub mod text;
pub mod usage;
