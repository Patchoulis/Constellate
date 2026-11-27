//! Constellate's engine: everything the app does that is not drawing it. It runs on the user's own computer as
//! part of the desktop app.
//!
//! The code is laid out by what it is about, one folder per subject, and no folder holds more than about eight
//! files: a subject that grows past that is cut into sub-folders of its own. A model feature (critique, stances,
//! extraction, ...) is one subject whole, its logic, routes and storage together, under [`features`].
//!
//! | module      | what lives there                                                              |
//! |-------------|-------------------------------------------------------------------------------|
//! | [`features`] | the model features, one folder each: critique, stances, extraction, explanation, claim tree, citing sentences, cited answers, communities' names and overviews, the AI-text detector |
//! | [`analysis`] | the similarity graph over papers and its communities ([`analysis::graph`]), who cites whom, what reviews come to |
//! | [`api`]     | what the interface asks, over HTTP, answered only to the app itself: the route table, the guard every request passes ([`api::guard`]), a paper's routes ([`api::papers`]) |
//! | [`evidence`] | "this statement rests on that place in that paper": quotes checked before they are shown |
//! | [`library`] | what is kept on disk: the SQLite library ([`library::core`], [`library::papers`], [`library::money`]), and the file store for a run's results |
//! | [`jobs`]    | the one background job at a time, as an explicit state machine                  |
//! | [`machine`] | what this computer can run the embedding model on, in words for Settings       |
//! | [`model`]   | the embedding model on any graphics card or the processor; a language model over its provider's API |
//! | [`pipeline`] | making a run ([`pipeline::steps`]), what it makes ([`pipeline::results`]), keeping it and serving it |
//! | [`papers`]  | one paper's PDF, fetched when it is not kept, and what its authors declare in it |
//! | [`runs`]    | many runs: the list without loading any, a cache by use, warm-up at start      |
//! | [`usage`]   | what calls to the language model cost, from prices built into the engine       |
//! | [`selection`] | which papers a run is about: conferences, or a date range                     |
//! | [`settings`] | the person's settings and how a change to them is checked                     |
//! | [`sources`] | where papers come from: conference lists ([`sources::conferences`]), arXiv ([`sources::arxiv`]), proceedings' PDFs |
//! | [`text`]    | what is read out of text: a PDF's passages and references ([`text::pdf`]), a reviews page ([`text::reviews`]), authors' declarations |
//! | [`threads`] | how many threads the engine computes on, and many papers' work run on them    |
//!
//! The engine is the app's only backend: everything the Python backend (`apps/backend`) did is done here, and its
//! data folder is moved in at the first open of the library ([`library::Library::import_old`]).

pub mod analysis;
pub mod api;
pub mod evidence;
pub mod features;
pub mod jobs;
pub mod library;
pub mod machine;
pub mod model;
pub mod papers;
pub mod pipeline;
pub mod runs;
pub mod selection;
pub mod settings;
pub mod sources;
pub mod text;
pub mod threads;
pub mod usage;
