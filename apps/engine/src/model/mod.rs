//! The models the engine runs or calls: the embedding model, on any graphics card or the processor, and the
//! language model, Claude, over its API, asked one question at a time or about many papers at once ([`batch`]).

pub mod answers;
pub mod batch;
pub mod embed;
pub mod language;
