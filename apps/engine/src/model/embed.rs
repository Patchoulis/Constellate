//! Embedding: one unit-length vector per text, from a model run by ONNX Runtime.
//!
//! The model runs on whatever the computer has, with nothing for the user to install and nothing tied to one
//! maker's cards:
//!
//! * **Windows**: DirectML, which uses any DirectX 12 graphics card (NVIDIA, AMD or Intel, built-in ones too).
//! * **Linux**: WebGPU, which reaches any card through Vulkan.
//! * **Anywhere there is no usable card** (or on a system not listed): the processor, about ten times slower.
//!
//! The model is `Qwen3-Embedding-0.6B` in the ONNX form its publisher's community exports
//! (`onnx-community/Qwen3-Embedding-0.6B-ONNX`). Its vectors agree with the ones the PyTorch model gives to four
//! decimals of cosine similarity, so a library embedded before stays valid. Two things about that export shape
//! the code below:
//!
//! * A text's vector is the model's output at the text's **last token**. Texts in a batch have different lengths,
//!   so the shorter ones are padded on the **left**: then every text's last token is the last position.
//! * It was exported as a text generator, so besides the tokens it expects the memory of earlier tokens for each
//!   of its layers. There are none; they are passed empty.
//!
//! # Threads
//!
//! [`Encoder::encode`] takes `&mut self`: ONNX Runtime's sessions are not safe to run from two threads at once
//! (ort's `Session::run` takes `&mut self` for that reason), so the compiler refuses it, and one encoder runs one
//! batch at a time. That is also how it is fastest. On a graphics card the batch is what runs in parallel, so
//! batches go one after another. On the processor ONNX Runtime splits each operator over its own threads, as many
//! as the engine computes on; running batches side by side on the engine's threads as well would put two sets of
//! threads on the same cores. The tokenizer's work is a small part of a batch, and is not spread out either.

use std::path::Path;

use ort::ep;
use ort::session::{OutputSelector, RunOptions, Session};
use ort::value::Tensor;
use tokenizers::Tokenizer;

/// Tokens of a text that are read; the rest is cut off. Titles and abstracts fit.
pub const MAX_TOKENS: usize = 512;
/// Shape of one layer's (empty) memory: [texts, heads, earlier tokens, width per head].
const HEADS: usize = 8;
const HEAD_WIDTH: usize = 128;

/// What runs the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// Whatever graphics card the computer has: through DirectML on Windows, WebGPU (Vulkan) on Linux.
    GraphicsCard,
    Processor,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("the embedding model could not be loaded from {folder}: {reason}")]
    Load { folder: String, reason: String },
    #[error("a text could not be split into tokens: {0}")]
    Tokens(String),
    #[error("the embedding model failed: {0}")]
    Run(#[from] ort::Error),
    #[error("the embedding model answered with {got} numbers where {expected} were expected")]
    Shape { got: usize, expected: usize },
}

/// A batch of token lists as the model wants it: three tables of `texts` rows by `width` columns, row after row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub texts: usize,
    pub width: usize,
    /// The tokens, the shorter texts padded on the left.
    pub ids: Vec<i64>,
    /// 1 where there is a token, 0 where there is padding.
    pub mask: Vec<i64>,
    /// Each token's place in its own text, counted from 0: padding does not shift a text's positions.
    pub positions: Vec<i64>,
}

/// Lay token lists out as one batch, padded on the left to the longest.
pub fn batch(tokens: &[Vec<i64>]) -> Batch {
    let width = tokens.iter().map(Vec::len).max().unwrap_or(0);
    let cells = tokens.len() * width;
    let (mut ids, mut mask, mut positions) = (vec![0; cells], vec![0; cells], vec![0; cells]);
    for (row, text) in tokens.iter().enumerate() {
        let start = row * width + (width - text.len());
        for (place, token) in text.iter().enumerate() {
            ids[start + place] = *token;
            mask[start + place] = 1;
            positions[start + place] = place as i64;
        }
    }
    Batch { texts: tokens.len(), width, ids, mask, positions }
}

/// Each text's vector from the model's output (`texts` x `width` x `size` numbers, row after row): the output at
/// the last position, scaled to length 1. A vector of zeros stays zeros.
pub fn last_token_vectors(
    hidden: &[f32],
    texts: usize,
    width: usize,
    size: usize,
) -> Result<Vec<Vec<f32>>, EmbedError> {
    if hidden.len() != texts * width * size {
        return Err(EmbedError::Shape { got: hidden.len(), expected: texts * width * size });
    }
    if width == 0 {
        // texts with no tokens at all (empty ones) have no last position: their vectors are zeros
        return Ok(vec![vec![0.0; size]; texts]);
    }
    Ok((0..texts)
        .map(|row| {
            let start = (row * width + width - 1) * size;
            let mut vector = hidden[start..start + size].to_vec();
            let length = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if length > 0.0 {
                vector.iter_mut().for_each(|x| *x /= length);
            }
            vector
        })
        .collect())
}

/// This system's way to run the model on any graphics card; an error where it has none, so that the processor
/// is used.
#[cfg(windows)]
fn graphics_card() -> Result<ep::ExecutionProviderDispatch, ort::Error> {
    Ok(ep::DirectML::default().build().error_on_failure())
}

#[cfg(target_os = "linux")]
fn graphics_card() -> Result<ep::ExecutionProviderDispatch, ort::Error> {
    Ok(ep::WebGPU::default().build().error_on_failure())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn graphics_card() -> Result<ep::ExecutionProviderDispatch, ort::Error> {
    Err(ort::Error::new("no graphics-card support is built in for this system"))
}

pub struct Encoder {
    tokenizer: Tokenizer,
    session: Session,
    /// How many layers the model has: each wants its empty memory.
    layers: usize,
    pub device: Device,
}

impl Encoder {
    /// Load the model from a folder holding `tokenizer.json` and `onnx/model_fp16.onnx` (with its data file).
    /// The graphics card is tried first; if it cannot be used, the processor is.
    pub fn open(folder: &Path) -> Result<Self, EmbedError> {
        let failed = |reason: String| EmbedError::Load { folder: folder.display().to_string(), reason };
        let tokenizer =
            Tokenizer::from_file(folder.join("tokenizer.json")).map_err(|error| failed(error.to_string()))?;
        let model = folder.join("onnx").join("model_fp16.onnx");
        let (session, device) = match Self::session(&model, Device::GraphicsCard) {
            Ok(session) => (session, Device::GraphicsCard),
            Err(_) => (
                Self::session(&model, Device::Processor).map_err(|error| failed(error.to_string()))?,
                Device::Processor,
            ),
        };
        let layers = session.inputs().iter().filter(|input| input.name().ends_with(".key")).count();
        Ok(Self { tokenizer, session, layers, device })
    }

    fn session(model: &Path, device: Device) -> Result<Session, ort::Error> {
        let providers = match device {
            Device::GraphicsCard => vec![graphics_card()?],
            Device::Processor => vec![],
        };
        let environment = ort::init().with_execution_providers(providers).build()?;
        // DirectML needs memory patterns off and one operator at a time; the others do not mind. Within an
        // operator, ONNX Runtime uses as many threads as the engine computes on ([`crate::threads`]), not a count
        // of its own: on the processor those threads are where the model's time goes.
        Session::builder(&environment)?
            .with_memory_pattern(false)?
            .with_parallel_execution(false)?
            .with_intra_threads(crate::threads::computing())?
            .commit_from_file(model)
    }

    /// The tokens of a text, cut to [`MAX_TOKENS`].
    fn tokens(&self, text: &str) -> Result<Vec<i64>, EmbedError> {
        let encoded = self.tokenizer.encode(text, true).map_err(|error| EmbedError::Tokens(error.to_string()))?;
        Ok(encoded.get_ids().iter().take(MAX_TOKENS).map(|id| i64::from(*id)).collect())
    }

    /// One unit-length vector per text. An empty list of texts gives an empty list.
    pub fn encode(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let tokens = texts.iter().map(|text| self.tokens(text)).collect::<Result<Vec<_>, _>>()?;
        let Batch { texts: rows, width, ids, mask, positions } = batch(&tokens);
        let shape = [rows, width];
        let mut inputs = ort::inputs![
            "input_ids" => Tensor::<i64>::from_array((shape, ids))?,
            "attention_mask" => Tensor::<i64>::from_array((shape, mask))?,
            "position_ids" => Tensor::<i64>::from_array((shape, positions))?,
        ];
        for layer in 0..self.layers {
            for part in ["key", "value"] {
                let empty = Tensor::<half::f16>::from_array(([rows, HEADS, 0, HEAD_WIDTH], Vec::<half::f16>::new()))?;
                inputs.push((format!("past_key_values.{layer}.{part}").into(), empty.into()));
            }
        }
        // only the vectors are wanted: the model's memory of this batch would be gigabytes nobody reads
        let only = RunOptions::new()?.with_outputs(OutputSelector::no_default().with("last_hidden_state"));
        let outputs = self.session.run_with_options(inputs, &only)?;
        let (shape, hidden) = outputs["last_hidden_state"].try_extract_tensor::<f32>()?;
        let size = shape.last().copied().unwrap_or(0) as usize;
        let mut vectors = last_token_vectors(hidden, rows, width, size)?;
        // a text with no tokens is all padding: what the model gave at its last position is not its vector
        for (vector, tokens) in vectors.iter_mut().zip(&tokens) {
            if tokens.is_empty() {
                vector.iter_mut().for_each(|x| *x = 0.0);
            }
        }
        Ok(vectors)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn shorter_texts_are_padded_on_the_left_so_every_last_token_is_last() {
        let made = batch(&[vec![11, 12, 13], vec![21], vec![31, 32]]);
        assert_eq!((made.texts, made.width), (3, 3));
        assert_eq!(made.ids, [11, 12, 13, 0, 0, 21, 0, 31, 32]);
        assert_eq!(made.mask, [1, 1, 1, 0, 0, 1, 0, 1, 1]);
        // a text's positions start at 0 wherever its padding puts it
        assert_eq!(made.positions, [0, 1, 2, 0, 0, 0, 0, 0, 1]);
        for row in 0..3 {
            assert_eq!(made.mask[row * 3 + 2], 1, "the last column of row {row} is a real token");
        }
    }

    #[test]
    fn an_empty_batch_and_an_empty_text_are_laid_out_without_trouble() {
        assert_eq!(batch(&[]), Batch { texts: 0, width: 0, ids: vec![], mask: vec![], positions: vec![] });
        let made = batch(&[vec![], vec![5]]);
        assert_eq!((made.ids, made.mask), (vec![0, 5], vec![0, 1]));
    }

    #[test]
    fn a_texts_vector_is_the_output_at_its_last_position_scaled_to_length_one() {
        // 2 texts, 2 positions, 2 numbers each
        let hidden = [9.0, 9.0, 3.0, 4.0, /* second text */ 9.0, 9.0, 0.0, 2.0];
        let vectors = last_token_vectors(&hidden, 2, 2, 2).unwrap();
        assert_eq!(vectors, [vec![0.6, 0.8], vec![0.0, 1.0]]);
        assert_eq!(last_token_vectors(&[0.0, 0.0], 1, 1, 2).unwrap(), [vec![0.0, 0.0]]); // not divided by zero
    }

    #[test]
    fn a_batch_of_texts_with_no_tokens_gives_zero_vectors_and_does_not_panic() {
        assert_eq!(last_token_vectors(&[], 2, 0, 3).unwrap(), [vec![0.0; 3], vec![0.0; 3]]);
    }

    #[test]
    fn an_output_of_the_wrong_size_is_an_error_not_a_wrong_answer() {
        let error = last_token_vectors(&[1.0; 7], 2, 2, 2).unwrap_err();
        assert_eq!(error.to_string(), "the embedding model answered with 7 numbers where 8 were expected");
    }

    /// A tokenizer laid out as the model's `tokenizer.json` is (NFC, the model's split pattern, byte-level BPE, and
    /// `<|endoftext|>` appended to every text), with a vocabulary of a few words: made here, so that no model file
    /// is needed. The ids it gives were worked out by the Python `tokenizers` from the same file.
    const SMALL_TOKENIZER: &str = r#"{
        "version": "1.0", "truncation": null, "padding": null,
        "added_tokens": [{"id": 29, "content": "<|endoftext|>", "single_word": false, "lstrip": false, "rstrip": false,
                          "normalized": false, "special": true}],
        "normalizer": {"type": "NFC"},
        "pre_tokenizer": {"type": "Sequence", "pretokenizers": [
            {"type": "Split", "pattern": {"Regex": "(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\\r\\n\\p{L}\\p{N}]?\\p{L}+|\\p{N}| ?[^\\s\\p{L}\\p{N}]+[\\r\\n]*|\\s*[\\r\\n]+|\\s+(?!\\S)|\\s+"},
             "behavior": "Isolated", "invert": false},
            {"type": "ByteLevel", "add_prefix_space": false, "trim_offsets": false, "use_regex": false}]},
        "post_processor": {"type": "Sequence", "processors": [
            {"type": "ByteLevel", "add_prefix_space": false, "trim_offsets": false, "use_regex": false},
            {"type": "TemplateProcessing",
             "single": [{"Sequence": {"id": "A", "type_id": 0}}, {"SpecialToken": {"id": "<|endoftext|>", "type_id": 0}}],
             "pair": [{"Sequence": {"id": "A", "type_id": 0}}, {"Sequence": {"id": "B", "type_id": 0}},
                      {"SpecialToken": {"id": "<|endoftext|>", "type_id": 0}}],
             "special_tokens": {"<|endoftext|>": {"id": "<|endoftext|>", "ids": [29], "tokens": ["<|endoftext|>"]}}}]},
        "decoder": {"type": "ByteLevel", "add_prefix_space": false, "trim_offsets": false, "use_regex": false},
        "model": {"type": "BPE", "dropout": null, "unk_token": null, "continuing_subword_prefix": "",
                  "end_of_word_suffix": "", "fuse_unk": false, "byte_fallback": false, "ignore_merges": false,
                  "vocab": {"H": 0, "e": 1, "l": 2, "o": 3, "Ġ": 4, "w": 5, "r": 6, "d": 7, "2": 8, "0": 9, "6": 10,
                            "c": 11, "a": 12, "f": 13, "Ã": 14, "©": 15, "He": 16, "ll": 17, "Hell": 18, "Hello": 19,
                            "Ġw": 20, "or": 21, "Ġwor": 22, "Ġworl": 23, "Ġworld": 24, "Ã©": 25, "ca": 26, "caf": 27,
                            "cafÃ©": 28},
                  "merges": ["H e", "l l", "He ll", "Hell o", "Ġ w", "o r", "Ġw or", "Ġwor l", "Ġworl d", "Ã ©", "c a",
                             "ca f", "caf Ã©"]}
    }"#;

    #[test]
    fn a_tokenizer_file_of_the_models_kind_splits_texts_into_the_ids_the_python_gives() {
        // read from a file, as `Encoder::open` reads the model's, and encoded with its special tokens, as
        // `Encoder::tokens` does
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("tokenizer.json"), SMALL_TOKENIZER).unwrap();
        let tokenizer = Tokenizer::from_file(folder.path().join("tokenizer.json")).unwrap();
        let ids = |text: &str| tokenizer.encode(text, true).unwrap().get_ids().to_vec();
        // a word with the space before it is one token; the text ends with <|endoftext|>, whose output is the
        // text's vector
        assert_eq!(ids("Hello world"), [19, 24, 29]);
        // digits one by one, and the space before them a token of its own
        assert_eq!(ids("Hello world 2026"), [19, 24, 4, 8, 9, 8, 10, 29]);
        // a letter and its combining accent are composed first (NFC), then read as their UTF-8 bytes
        assert_eq!(ids("cafe\u{301}"), [28, 29]);
        assert_eq!(ids("caf\u{e9}"), [28, 29]);
        // an empty text still has its end token, so no text is ever without a last position
        assert_eq!(ids(""), [29]);
        // without the special tokens there is no end token
        assert_eq!(tokenizer.encode("Hello world", false).unwrap().get_ids(), [19, 24]);
        assert_eq!(tokenizer.decode(&[19, 24, 4, 8, 9, 8, 10, 29], true).unwrap(), "Hello world 2026");
    }

    #[test]
    fn a_tokenizer_file_that_cannot_be_read_is_an_error_naming_the_folder() {
        let folder = tempfile::tempdir().unwrap();
        let Err(missing) = Encoder::open(folder.path()) else { panic!("an encoder without its files") };
        let said = missing.to_string();
        assert!(said.starts_with("the embedding model could not be loaded from "), "{said}");
        assert!(said.contains(&folder.path().display().to_string()), "{said}");
        std::fs::write(folder.path().join("tokenizer.json"), "{ not json").unwrap();
        assert!(matches!(Encoder::open(folder.path()), Err(EmbedError::Load { .. })));
    }

    /// The model's own tokenizer, when it is on this computer (`CONSTELLATE_MODEL`, as below): the ids the Python
    /// `tokenizers` 0.23 gives with the model's `tokenizer.json`.
    #[test]
    #[ignore = "needs the model's tokenizer.json: set CONSTELLATE_MODEL to its folder"]
    fn the_models_tokenizer_gives_the_ids_the_python_gives() {
        let folder = std::path::PathBuf::from(std::env::var("CONSTELLATE_MODEL").unwrap());
        let tokenizer = Tokenizer::from_file(folder.join("tokenizer.json")).unwrap();
        let ids = |text: &str| tokenizer.encode(text, true).unwrap().get_ids().to_vec();
        assert_eq!(ids("Hello world"), [9707, 1879, 151_643]);
        assert_eq!(ids("Hello world 2026"), [9707, 1879, 220, 17, 15, 17, 21, 151_643]);
        assert_eq!(ids("cafe\u{301} papers"), [924, 58858, 15689, 151_643]);
        assert_eq!(ids(""), [151_643]);
        assert_eq!(
            ids("Graph neural networks for 3D point clouds."),
            [11212, 29728, 14155, 369, 220, 18, 35, 1459, 29514, 13, 151_643]
        );
    }

    /// The real model, when it is on this computer: point `CONSTELLATE_MODEL` at its folder (with the reference
    /// file the Python backend's vectors were exported to) and run `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs the model's files: set CONSTELLATE_MODEL to their folder"]
    fn the_real_model_gives_the_vectors_the_library_already_holds() {
        let folder = std::path::PathBuf::from(std::env::var("CONSTELLATE_MODEL").unwrap());
        let reference: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(folder.join("reference.json")).unwrap()).unwrap();
        let texts: Vec<&str> =
            reference["texts"].as_array().unwrap().iter().map(|text| text.as_str().unwrap()).collect();
        let mut encoder = Encoder::open(&folder).unwrap();
        let mut least = f32::MAX;
        for (chunk, kept) in texts.chunks(8).zip(reference["vectors"].as_array().unwrap().chunks(8)) {
            for (vector, kept) in encoder.encode(chunk).unwrap().iter().zip(kept) {
                let kept: Vec<f32> = kept.as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
                let length = kept.iter().map(|x| x * x).sum::<f32>().sqrt();
                least = least.min(vector.iter().zip(&kept).map(|(a, b)| a * b).sum::<f32>() / length);
            }
        }
        assert!(least > 0.9999, "cosine with the stored vectors fell to {least} on {:?}", encoder.device);
        assert!(encoder.encode(&[]).unwrap().is_empty());
    }
}
