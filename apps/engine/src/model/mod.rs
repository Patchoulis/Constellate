//! The models the engine runs or calls: the embedding model, on any graphics card or the processor, and the
//! language model, over the API of the provider chosen (the Claude API, OpenAI, Gemini, an OpenAI-compatible server
//! and others).

pub mod batch;
pub mod chosen;
pub mod embed;
pub mod language;
