//! What this computer can do, as far as it changes how the app behaves: whether the embedding model, the slowest
//! step of a run, can run on a graphics card here. Shown in the information part of Settings (`Library.machine`
//! in `packages/contracts/src/index.ts`).
//!
//! It is asked of ONNX Runtime, which runs the model ([`crate::model::embed`]), without loading the model and
//! without creating ONNX Runtime's environment: a process may create that environment only once, and it belongs
//! to the encoder, which chooses its execution providers when it is made. What can be asked without it is which
//! execution providers this build of ONNX Runtime has. That is the way to a graphics card the encoder will try
//! (DirectML on Windows, WebGPU on Linux), not a promise that a card is there: the encoder tries the card first
//! and uses the processor when it cannot, so the words say both.
//!
//! The card's own name is not known here. ONNX Runtime lists the devices it found only through an environment
//! (and then only their maker, not their model); the system's own lists (DXGI, Vulkan) would need `unsafe` code or
//! a heavy new dependency.

#[cfg(any(windows, target_os = "linux"))]
use ort::ep::{self, ExecutionProvider};
use serde::Serialize;

/// What the interface is told: `{ gpu: string | null; embedding: string }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Machine {
    /// The graphics card the embedding model can use, in words; `None` when there is none it can use, which the
    /// interface shows as a warning.
    pub gpu: Option<String>,
    /// What that means for embedding, in words for the person reading Settings.
    pub embedding: String,
}

/// Where ONNX Runtime can run the embedding model on this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runner {
    /// DirectML, on Windows: any DirectX 12 graphics card.
    DirectMl,
    /// WebGPU, on Linux: any graphics card with a Vulkan driver.
    WebGpu,
    /// No way to a graphics card is built in for this system.
    Processor,
}

/// What this computer can do for embedding, in the interface's terms.
pub fn summary() -> Machine {
    describe(runner())
}

/// The way to a graphics card this build of ONNX Runtime has, if any. An error from ONNX Runtime while it is asked
/// counts as none: the encoder would then use the processor too.
#[cfg(windows)]
pub fn runner() -> Runner {
    if ep::DirectML::is_available().unwrap_or(false) { Runner::DirectMl } else { Runner::Processor }
}

#[cfg(target_os = "linux")]
pub fn runner() -> Runner {
    if ep::WebGPU::is_available().unwrap_or(false) { Runner::WebGpu } else { Runner::Processor }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn runner() -> Runner {
    Runner::Processor
}

/// The words for `runner`. The speeds are those measured for the default model, `Qwen3-Embedding-0.6B`.
pub fn describe(runner: Runner) -> Machine {
    const ON_THE_PROCESSOR: &str = "allow about a minute per hundred papers";
    const NEVER_AGAIN: &str = "Papers already embedded are never embedded again.";
    let card = |gpu: &str, through: &str| Machine {
        gpu: Some(gpu.to_owned()),
        embedding: format!(
            "Papers are embedded on the graphics card through {through}: about a thousand papers a minute. If no such \
             card is found when embedding starts, the processor is used instead: {ON_THE_PROCESSOR}. {NEVER_AGAIN}"
        ),
    };
    match runner {
        Runner::DirectMl => card(
            "DirectML (any DirectX 12 graphics card)",
            "DirectML, which works with any DirectX 12 card (NVIDIA, AMD or Intel, built-in ones too)",
        ),
        Runner::WebGpu => card(
            "WebGPU (any graphics card with a Vulkan driver)",
            "WebGPU, which works with any card that has a Vulkan driver",
        ),
        Runner::Processor => Machine {
            gpu: None,
            embedding: format!(
                "No graphics card can be used on this system, so papers are embedded on the processor. Everything \
                 works, but embedding is many times slower: {ON_THE_PROCESSOR}, and prefer a smaller embedding \
                 model. {NEVER_AGAIN}"
            ),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_is_told_exactly_the_fields_of_its_contract() {
        for runner in [Runner::DirectMl, Runner::WebGpu, Runner::Processor] {
            let told = serde_json::to_value(describe(runner)).unwrap();
            let mut names: Vec<&str> = told.as_object().unwrap().keys().map(String::as_str).collect();
            names.sort_unstable();
            assert_eq!(names, ["embedding", "gpu"]);
            assert!(told["embedding"].is_string());
        }
        assert_eq!(serde_json::to_value(describe(Runner::Processor)).unwrap()["gpu"], serde_json::Value::Null);
    }

    #[test]
    fn without_a_graphics_card_the_processor_is_named_and_there_is_no_gpu() {
        let machine = describe(Runner::Processor);
        assert_eq!(machine.gpu, None);
        assert!(machine.embedding.contains("processor"), "{}", machine.embedding);
        assert!(machine.embedding.contains("a minute per hundred papers"), "{}", machine.embedding);
    }

    #[test]
    fn with_a_way_to_a_graphics_card_its_name_and_speed_are_said() {
        let machine = describe(Runner::DirectMl);
        assert_eq!(machine.gpu.as_deref(), Some("DirectML (any DirectX 12 graphics card)"));
        assert!(machine.embedding.contains("a thousand papers a minute"), "{}", machine.embedding);
        assert!(describe(Runner::WebGpu).embedding.contains("Vulkan"));
    }

    /// What this computer is told, printed: `cargo test machine -- --nocapture` shows it.
    #[test]
    fn this_computer_is_described_without_loading_the_model() {
        let machine = summary();
        println!("{}", serde_json::to_string_pretty(&machine).unwrap());
        assert_eq!(machine, describe(runner()));
        #[cfg(not(any(windows, target_os = "linux")))]
        assert_eq!(machine.gpu, None);
    }
}
