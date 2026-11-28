//! Which providers the language model can be reached through, and what each one can and cannot do.
//!
//! Every provider is one row of [`about`]: its name, the variables its key and address are read from, the API it
//! speaks, a few models it offers, and its [`Capabilities`]. The client reads the capabilities to decide how to ask
//! (a schema the provider enforces, or JSON asked for in the prompt and checked here), and the interface shows them
//! in plain words ([`Capabilities::limits`]), so that what a provider cannot do is said rather than papered over.
//!
//! The API facts of each row (addresses, how JSON and effort are asked for, how cached tokens are reported) were
//! read from the provider's own documentation, named in [`About::docs`], on 2026-11-27. The model ids are those
//! the documentation named that day; a provider whose models change often lets the user type another id
//! ([`ModelIds::ListedOrTyped`]).

use serde::{Deserialize, Serialize};

use super::Effort;

/// A provider of language models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Provider {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "gemini")]
    Gemini,
    /// Any server that speaks OpenAI's Chat Completions: Ollama, LM Studio, llama.cpp, vLLM on this computer, or a
    /// hosted gateway, at the address the user sets.
    #[serde(rename = "compatible")]
    Compatible,
    #[serde(rename = "openrouter")]
    OpenRouter,
    #[serde(rename = "xai")]
    Xai,
    #[serde(rename = "mistral")]
    Mistral,
    #[serde(rename = "deepseek")]
    DeepSeek,
    #[serde(rename = "groq")]
    Groq,
    #[serde(rename = "together")]
    Together,
    #[serde(rename = "fireworks")]
    Fireworks,
    #[serde(rename = "cerebras")]
    Cerebras,
    #[serde(rename = "moonshot")]
    Moonshot,
    #[serde(rename = "minimax")]
    MiniMax,
    #[serde(rename = "zai")]
    Zai,
    #[serde(rename = "nvidia")]
    Nvidia,
    #[serde(rename = "huggingface")]
    HuggingFace,
    #[serde(rename = "vercel")]
    Vercel,
    #[serde(rename = "baseten")]
    Baseten,
    #[serde(rename = "meta")]
    Meta,
}

impl Provider {
    /// Every provider, in the order the interface lists them.
    pub const ALL: [Provider; 20] = [
        Provider::Anthropic,
        Provider::OpenAi,
        Provider::Gemini,
        Provider::Compatible,
        Provider::OpenRouter,
        Provider::Xai,
        Provider::Mistral,
        Provider::DeepSeek,
        Provider::Groq,
        Provider::Together,
        Provider::Fireworks,
        Provider::Cerebras,
        Provider::Moonshot,
        Provider::MiniMax,
        Provider::Zai,
        Provider::Nvidia,
        Provider::HuggingFace,
        Provider::Vercel,
        Provider::Baseten,
        Provider::Meta,
    ];

    /// The id stored in the settings and used in the interface: "openai".
    pub fn id(self) -> &'static str {
        about(self).id
    }

    /// The provider with this id.
    pub fn from_id(id: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|provider| provider.id() == id)
    }

    /// Its row of the table.
    pub fn about(self) -> &'static About {
        about(self)
    }
}

/// The API a provider speaks. Several speak the same one, each with its quirks ([`Chat`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Api {
    /// The Claude API's Messages (`anthropic.rs`).
    Messages,
    /// OpenAI's Responses (`responses.rs`).
    Responses,
    /// Gemini's `generateContent` (`gemini.rs`).
    GenerateContent,
    /// OpenAI's Chat Completions, which most other providers and local servers also speak (`chat.rs`).
    ChatCompletions(Chat),
}

/// What a Chat Completions provider does differently from the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chat {
    pub(crate) strict: Strict,
    pub(crate) effort: EffortField,
}

/// Whether `strict: true` is sent with a JSON schema: some providers refuse the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strict {
    Sent,
    Left,
}

/// Where a Chat Completions provider reads the effort from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffortField {
    /// `reasoning_effort: "low"`, as OpenAI's Chat Completions.
    TopLevel,
    /// `reasoning: {"effort": "low"}` (OpenRouter).
    Object,
}

/// Where the key is read from, and whether one is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "variable", rename_all = "snake_case")]
pub enum Key {
    Required(&'static str),
    /// A server on this computer usually needs none.
    Optional(&'static str),
}

impl Key {
    pub fn variable(self) -> &'static str {
        match self {
            Key::Required(variable) | Key::Optional(variable) => variable,
        }
    }
}

/// Where requests are sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Address {
    /// The provider's own, always.
    Fixed(&'static str),
    /// The provider's own unless `variable` names another (a proxy, a gateway).
    Overridable { variable: &'static str, default: &'static str },
    /// Only what `variable` says: there is no default.
    Required(&'static str),
}

impl Address {
    pub fn variable(self) -> Option<&'static str> {
        match self {
            Address::Fixed(_) => None,
            Address::Overridable { variable, .. } | Address::Required(variable) => Some(variable),
        }
    }
}

/// How an answer in JSON is had from the provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Answers {
    /// The schema is sent and the provider guarantees the answer follows it.
    Enforced,
    /// The schema is sent, but whether the answer follows it depends on the model: it is checked here.
    Requested,
    /// The provider only promises JSON of some shape: the schema is in the prompt and checked here.
    JsonMode,
    /// Nothing of the kind: the schema is in the prompt and the answer checked here.
    Prompted,
}

/// Whether the effort can be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortControl {
    /// For the models listed with efforts; a typed model id gets the provider's default.
    ByModel,
    /// Never sent: the server or the model decides.
    NotSent,
}

/// How the long shared part of a prompt is cached, which decides what a batch is estimated to cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Caching {
    /// Marked in the request; writes and reads each billed at their own price (the Claude API).
    Marked,
    /// The provider caches a repeated prefix by itself and bills writing it at a price of its own.
    AutomaticWritesBilled,
    /// The provider caches a repeated prefix by itself; reads are cheaper, writing costs nothing extra.
    Automatic,
    /// Nothing is reported as cached: every question pays for the whole prefix.
    NotReported,
}

/// How tokens are counted before a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenCount {
    /// The provider has an endpoint that counts a prompt's tokens.
    Endpoint,
    /// Estimated from characters ([`super::Counting::Assumed`]).
    Characters,
}

/// Which model ids can be chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelIds {
    /// Only those listed.
    Listed,
    /// Those listed, or any other id the provider has.
    ListedOrTyped,
    /// Any id the server has: nothing is listed in advance.
    Typed,
}

/// What a provider can do, as one value per question rather than flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub answers: Answers,
    pub effort: EffortControl,
    pub caching: Caching,
    pub counting: TokenCount,
}

impl Capabilities {
    /// What this provider cannot do, in plain words, for the settings: nothing for one that can do everything.
    pub fn limits(&self) -> Vec<&'static str> {
        let mut limits = Vec::new();
        match self.answers {
            Answers::Enforced => {}
            Answers::Requested => limits.push(
                "Does not guarantee answers follow the requested form: each is checked here and asked for once more \
                 if it does not.",
            ),
            Answers::JsonMode | Answers::Prompted => limits.push(
                "Cannot be held to a form: it is asked for in the question, each answer is checked here and asked \
                 for once more if it does not follow it.",
            ),
        }
        if self.effort == EffortControl::NotSent {
            limits.push("How hard the model thinks cannot be set from here.");
        }
        match self.caching {
            Caching::Marked | Caching::Automatic | Caching::AutomaticWritesBilled => {}
            Caching::NotReported => limits
                .push("Reports no prompt caching: estimates count the long shared text in full for every question."),
        }
        if self.counting == TokenCount::Characters {
            limits.push("Cannot count tokens in advance: they are estimated from the number of characters.");
        }
        limits
    }
}

/// A model a provider offers, as the interface shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Offered {
    pub id: &'static str,
    /// As shown: "GPT-6.2 Sol".
    pub label: &'static str,
    /// A few words beside it.
    pub note: &'static str,
    /// The efforts the model accepts; none when it has no such setting (or it is not known).
    pub efforts: &'static [Effort],
}

/// One provider's row.
#[derive(Debug)]
pub struct About {
    pub provider: Provider,
    pub id: &'static str,
    pub label: &'static str,
    pub key: Key,
    pub address: Address,
    pub(crate) api: Api,
    pub capabilities: Capabilities,
    pub ids: ModelIds,
    /// The first is the default.
    pub models: &'static [Offered],
    /// Where the facts of this row were read.
    pub docs: &'static str,
}

/// The model that reads papers by default (tables, agree / disagree, questions, claims, weaknesses, explanations,
/// citation kinds) when the user chose none and the Claude API names communities: its newest Haiku, one of the
/// models listed above (with its price). No other provider has a default of its own: one there could point at a
/// model the user has not set up, so its papers are read by the naming model.
pub const CLAUDE_READING_DEFAULT: &str = "claude-haiku-5-5";
/// The effort it reads at by default: the settings' own default effort.
pub const READING_DEFAULT_EFFORT: Effort = Effort::Medium;

impl About {
    /// The model offered under `id`.
    pub fn model(&self, id: &str) -> Option<&'static Offered> {
        self.models.iter().find(|model| model.id == id)
    }

    /// The model that reads papers by default while this provider names communities, if it has one of its own
    /// ([`CLAUDE_READING_DEFAULT`]); none: the naming model reads them.
    pub fn reading_default(&self) -> Option<&'static Offered> {
        match self.provider {
            Provider::Anthropic => self.model(CLAUDE_READING_DEFAULT),
            _ => None,
        }
    }

    /// Whether `id` may be chosen: one listed, or, where ids may be typed, any plain id (letters, digits and
    /// `. _ - / :`, as providers write them, at most 200 characters).
    pub fn accepts(&self, id: &str) -> bool {
        match self.ids {
            ModelIds::Listed => self.model(id).is_some(),
            ModelIds::ListedOrTyped | ModelIds::Typed => self.model(id).is_some() || plain_id(id),
        }
    }
}

/// A model id as providers write them, safe in a URL path (Gemini puts it in one) and in a log line.
pub fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && !id.starts_with(['.', '/'])
        && !id.contains("..")
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':'))
}

const ALL: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh, Effort::Max];
const LOW_TO_HIGH: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High];
const LOW_TO_XHIGH: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh];
const LOW_HIGH_MAX: &[Effort] = &[Effort::Low, Effort::High, Effort::Max];
const NONE: &[Effort] = &[];

const fn offered(id: &'static str, label: &'static str, note: &'static str, efforts: &'static [Effort]) -> Offered {
    Offered { id, label, note, efforts }
}

/// The capabilities most hosted Chat Completions providers share: a schema requested, effort by model, a repeated
/// prefix cached by the provider, no token counting.
const HOSTED_CHAT: Capabilities = Capabilities {
    answers: Answers::Requested,
    effort: EffortControl::ByModel,
    caching: Caching::Automatic,
    counting: TokenCount::Characters,
};

const PLAIN_CHAT: Chat = Chat { strict: Strict::Sent, effort: EffortField::TopLevel };

static TABLE: [About; 20] = [
    About {
        provider: Provider::Anthropic,
        id: "anthropic",
        label: "Anthropic",
        key: Key::Required("ANTHROPIC_API_KEY"),
        address: Address::Overridable { variable: "ANTHROPIC_BASE_URL", default: "https://api.anthropic.com" },
        api: Api::Messages,
        capabilities: Capabilities {
            answers: Answers::Enforced,
            effort: EffortControl::ByModel,
            caching: Caching::Marked,
            counting: TokenCount::Endpoint,
        },
        ids: ModelIds::Listed,
        models: &[
            offered("claude-opus-5-5", "Claude Opus 5.5", "the default", ALL),
            offered("claude-sonnet-5-5", "Claude Sonnet 5.5", "faster, about half the cost", ALL),
            offered("claude-haiku-5-5", "Claude Haiku 5.5", "cheapest, least thorough", ALL),
            offered("claude-fable-5-2", "Claude Fable 5.2", "most capable, about twice the cost", ALL),
        ],
        docs: "https://platform.claude.com/docs/en/api/overview",
    },
    About {
        provider: Provider::OpenAi,
        id: "openai",
        label: "OpenAI",
        key: Key::Required("OPENAI_API_KEY"),
        address: Address::Overridable { variable: "OPENAI_BASE_URL", default: "https://api.openai.com/v1" },
        api: Api::Responses,
        capabilities: Capabilities {
            answers: Answers::Enforced,
            effort: EffortControl::ByModel,
            caching: Caching::AutomaticWritesBilled,
            counting: TokenCount::Endpoint,
        },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("gpt-6.2-sol", "GPT-6.2 Sol", "OpenAI's everyday model", ALL),
            offered("gpt-6.2-astra", "GPT-6.2 Astra", "most capable, four times the cost", ALL),
            offered("gpt-6.1-terra", "GPT-6.1 Terra", "fast, half the cost", ALL),
            offered("gpt-6-luna", "GPT-6 Luna", "cheapest", ALL),
        ],
        docs: "https://developers.openai.com/api/reference/resources/responses/methods/create",
    },
    About {
        provider: Provider::Gemini,
        id: "gemini",
        label: "Google Gemini",
        key: Key::Required("GEMINI_API_KEY"),
        address: Address::Overridable {
            variable: "GEMINI_BASE_URL",
            default: "https://generativelanguage.googleapis.com",
        },
        api: Api::GenerateContent,
        capabilities: Capabilities {
            answers: Answers::Enforced,
            effort: EffortControl::ByModel,
            caching: Caching::Automatic,
            counting: TokenCount::Endpoint,
        },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("gemini-4-carbon", "Gemini 4 Carbon", "Google's most capable", LOW_TO_HIGH),
            offered("gemini-4-argon", "Gemini 4 Argon", "faster", LOW_TO_HIGH),
            offered("gemini-4-neon", "Gemini 4 Neon", "", LOW_TO_XHIGH),
            offered("gemini-4-hydrogen", "Gemini 4 Hydrogen", "cheaper", LOW_TO_XHIGH),
            offered("gemini-3.8-flash", "Gemini 3.8 Flash", "fast", LOW_TO_HIGH),
            offered("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite", "cheapest", LOW_TO_HIGH),
        ],
        docs: "https://aistudio.google.com/api-reference/generate-content/generate-content.md",
    },
    About {
        provider: Provider::Compatible,
        id: "compatible",
        label: "OpenAI-compatible endpoint",
        key: Key::Optional("CONSTELLATE_MODEL_API_KEY"),
        address: Address::Required("CONSTELLATE_MODEL_BASE_URL"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities {
            answers: Answers::Requested,
            effort: EffortControl::NotSent,
            caching: Caching::NotReported,
            counting: TokenCount::Characters,
        },
        ids: ModelIds::Typed,
        models: &[],
        docs: "https://docs.ollama.com/api/openai-compatibility",
    },
    About {
        provider: Provider::OpenRouter,
        id: "openrouter",
        label: "OpenRouter",
        key: Key::Required("OPENROUTER_API_KEY"),
        address: Address::Fixed("https://openrouter.ai/api/v1"),
        api: Api::ChatCompletions(Chat { strict: Strict::Sent, effort: EffortField::Object }),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("x-ai/grok-4.7", "Grok 4.7", "through OpenRouter", ALL),
            offered("deepseek/deepseek-v4-pro", "DeepSeek V4 Pro", "through OpenRouter", ALL),
        ],
        docs: "https://openrouter.ai/docs/api-reference/overview",
    },
    About {
        provider: Provider::Xai,
        id: "xai",
        label: "xAI",
        key: Key::Required("XAI_API_KEY"),
        address: Address::Fixed("https://api.x.ai/v1"),
        api: Api::Responses,
        capabilities: Capabilities {
            answers: Answers::Requested,
            effort: EffortControl::ByModel,
            caching: Caching::Automatic,
            counting: TokenCount::Characters,
        },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("grok-4.7", "Grok 4.7", "xAI's flagship", LOW_TO_XHIGH),
            offered("grok-4.3", "Grok 4.3", "cheaper", &[Effort::Low, Effort::Medium]),
        ],
        docs: "https://docs.x.ai/developers/rest-api-reference/inference/chat-completions",
    },
    About {
        provider: Provider::Mistral,
        id: "mistral",
        label: "Mistral",
        key: Key::Required("MISTRAL_API_KEY"),
        address: Address::Fixed("https://api.mistral.ai/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("mistral-large-4-0", "Mistral Large 4", "", NONE),
            offered("mistral-medium-latest", "Mistral Medium", "", NONE),
            offered("mistral-small-latest", "Mistral Small", "", NONE),
        ],
        docs: "https://docs.mistral.ai/api/endpoint/chat",
    },
    About {
        provider: Provider::DeepSeek,
        id: "deepseek",
        label: "DeepSeek",
        key: Key::Required("DEEPSEEK_API_KEY"),
        address: Address::Fixed("https://api.deepseek.com"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities { answers: Answers::JsonMode, ..HOSTED_CHAT },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("deepseek-flash", "DeepSeek Flash", "half price off-peak", LOW_HIGH_MAX),
            offered("deepseek-v4-pro", "DeepSeek V4 Pro", "half price off-peak", LOW_HIGH_MAX),
        ],
        docs: "https://api-docs.deepseek.com/",
    },
    About {
        provider: Provider::Groq,
        id: "groq",
        label: "Groq",
        key: Key::Required("GROQ_API_KEY"),
        address: Address::Fixed("https://api.groq.com/openai/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("openai/gpt-oss-120b", "GPT-OSS 120B", "on Groq", LOW_TO_HIGH),
            offered("openai/gpt-oss-20b", "GPT-OSS 20B", "on Groq, cheaper", LOW_TO_HIGH),
            offered("llama-3.3-70b-versatile", "Llama 3.3 70B", "on Groq", NONE),
        ],
        docs: "https://console.groq.com/docs/openai",
    },
    About {
        provider: Provider::Together,
        id: "together",
        label: "Together AI",
        key: Key::Required("TOGETHER_API_KEY"),
        address: Address::Fixed("https://api.together.ai/v1"),
        api: Api::ChatCompletions(Chat { strict: Strict::Left, effort: EffortField::TopLevel }),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("deepseek-ai/DeepSeek-V4.1-Flash", "DeepSeek V4.1 Flash", "on Together", NONE),
            offered("moonshotai/Kimi-K3.1-Pro", "Kimi K3.1 Pro", "on Together", NONE),
            offered("zai-org/GLM-5.3", "GLM-5.3", "on Together", NONE),
            offered("Qwen/Qwen4-Max", "Qwen4 Max", "on Together", NONE),
        ],
        docs: "https://docs.together.ai/docs/openai-api-compatibility",
    },
    About {
        provider: Provider::Fireworks,
        id: "fireworks",
        label: "Fireworks AI",
        key: Key::Required("FIREWORKS_API_KEY"),
        address: Address::Fixed("https://api.fireworks.ai/inference/v1"),
        api: Api::ChatCompletions(Chat { strict: Strict::Left, effort: EffortField::TopLevel }),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("accounts/fireworks/models/glm-5p3", "GLM 5.3", "on Fireworks", ALL),
            offered("accounts/fireworks/models/glm-5p3-flash", "GLM 5.3 Flash", "on Fireworks, cheaper", ALL),
            offered("accounts/fireworks/models/kimi-k3", "Kimi K3", "on Fireworks", ALL),
            offered("accounts/fireworks/models/gpt-oss-120b", "GPT-OSS 120B", "on Fireworks", LOW_TO_HIGH),
        ],
        docs: "https://docs.fireworks.ai/api-reference/post-chatcompletions",
    },
    About {
        provider: Provider::Cerebras,
        id: "cerebras",
        label: "Cerebras",
        key: Key::Required("CEREBRAS_API_KEY"),
        address: Address::Fixed("https://api.cerebras.ai/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("gpt-oss-120b", "GPT-OSS 120B", "on Cerebras", NONE),
            offered("qwen-3.8-27b", "Qwen 3.8 27B", "on Cerebras", LOW_TO_HIGH),
        ],
        docs: "https://inference-docs.cerebras.ai/resources/openai",
    },
    About {
        provider: Provider::Moonshot,
        id: "moonshot",
        label: "Moonshot AI (Kimi)",
        key: Key::Required("MOONSHOT_API_KEY"),
        address: Address::Fixed("https://api.moonshot.ai/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("kimi-k3.1-pro", "Kimi K3.1 Pro", "", LOW_HIGH_MAX),
            offered("kimi-k3", "Kimi K3", "", LOW_HIGH_MAX),
        ],
        docs: "https://platform.kimi.ai/docs/api/chat",
    },
    About {
        provider: Provider::MiniMax,
        id: "minimax",
        label: "MiniMax",
        key: Key::Required("MINIMAX_API_KEY"),
        address: Address::Fixed("https://api.minimax.io/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities { answers: Answers::Prompted, ..HOSTED_CHAT },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("MiniMax-M3", "MiniMax M3", "", NONE),
            offered("MiniMax-M3.1-Flash", "MiniMax M3.1 Flash", "cheaper", ALL),
        ],
        docs: "https://platform.minimax.io/docs/api-reference/text-openai-api",
    },
    About {
        provider: Provider::Zai,
        id: "zai",
        label: "Z.AI (GLM)",
        key: Key::Required("ZAI_API_KEY"),
        address: Address::Fixed("https://api.z.ai/api/paas/v4"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities { answers: Answers::JsonMode, ..HOSTED_CHAT },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("glm-5.3", "GLM-5.3", "", LOW_HIGH_MAX),
            offered("glm-5.3-flash", "GLM-5.3 Flash", "cheaper", LOW_HIGH_MAX),
        ],
        docs: "https://docs.z.ai/api-reference/llm/chat-completion",
    },
    About {
        provider: Provider::Nvidia,
        id: "nvidia",
        label: "NVIDIA API catalog",
        key: Key::Required("NVIDIA_API_KEY"),
        address: Address::Fixed("https://integrate.api.nvidia.com/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities {
            answers: Answers::Prompted,
            effort: EffortControl::NotSent,
            caching: Caching::NotReported,
            counting: TokenCount::Characters,
        },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("nvidia/nemotron-3-super-120b-a12b", "Nemotron 3 Super", "free trial tier", NONE),
            offered("openai/gpt-oss-120b", "GPT-OSS 120B", "free trial tier", NONE),
            offered("meta/llama-3.1-70b-instruct", "Llama 3.1 70B", "free trial tier", NONE),
        ],
        docs: "https://docs.api.nvidia.com/nim/reference/llm-apis",
    },
    About {
        provider: Provider::HuggingFace,
        id: "huggingface",
        label: "Hugging Face Inference Providers",
        key: Key::Required("HF_TOKEN"),
        address: Address::Fixed("https://router.huggingface.co/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities { effort: EffortControl::NotSent, caching: Caching::NotReported, ..HOSTED_CHAT },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("deepseek-ai/DeepSeek-V4.1-Flash", "DeepSeek V4.1 Flash", "routed by Hugging Face", NONE),
            offered("Qwen/Qwen3-32B", "Qwen3 32B", "routed by Hugging Face", NONE),
        ],
        docs: "https://huggingface.co/docs/inference-providers/index",
    },
    About {
        provider: Provider::Vercel,
        id: "vercel",
        label: "Vercel AI Gateway",
        key: Key::Required("AI_GATEWAY_API_KEY"),
        address: Address::Fixed("https://ai-gateway.vercel.sh/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities { caching: Caching::NotReported, ..HOSTED_CHAT },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("anthropic/claude-opus-5", "Claude Opus 5", "through Vercel", ALL),
            offered("openai/gpt-6-astra", "GPT-6 Astra", "through Vercel", ALL),
        ],
        docs: "https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions",
    },
    About {
        provider: Provider::Baseten,
        id: "baseten",
        label: "Baseten",
        key: Key::Required("BASETEN_API_KEY"),
        address: Address::Fixed("https://inference.baseten.co/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: HOSTED_CHAT,
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("openai/gpt-oss-120b", "GPT-OSS 120B", "on Baseten", ALL),
            offered("deepseek-ai/DeepSeek-V4.1-Flash", "DeepSeek V4.1 Flash", "on Baseten", ALL),
            offered("moonshotai/Kimi-K3.1-Pro", "Kimi K3.1 Pro", "on Baseten", ALL),
            offered("zai-org/GLM-5.3", "GLM-5.3", "on Baseten", ALL),
        ],
        docs: "https://docs.baseten.co/development/model-apis/overview",
    },
    About {
        provider: Provider::Meta,
        id: "meta",
        label: "Meta Model API",
        key: Key::Required("META_API_KEY"),
        address: Address::Fixed("https://api.meta.ai/v1"),
        api: Api::ChatCompletions(PLAIN_CHAT),
        capabilities: Capabilities { answers: Answers::Prompted, effort: EffortControl::NotSent, ..HOSTED_CHAT },
        ids: ModelIds::ListedOrTyped,
        models: &[
            offered("muse-spark-1.3", "Muse Spark 1.3", "", NONE),
            offered("muse-wonder-1.0", "Muse Wonder 1.0", "in preview, most capable", NONE),
            offered("muse-ember-1.0", "Muse Ember 1.0", "cheapest", NONE),
        ],
        docs: "https://dev.meta.ai/docs/pricing-rate-limits",
    },
];

fn about(provider: Provider) -> &'static About {
    // the table has one row per provider, in the enum's order of declaration (and of `ALL`): checked by a test
    &TABLE[provider as usize]
}

/// The variable that switches the compatible endpoint from a JSON schema to JSON asked for in the prompt, for a
/// server that refuses `response_format`: `prompt`, or `schema` (the default).
pub const ANSWERS_VARIABLE: &str = "CONSTELLATE_MODEL_JSON";

/// Every variable the language model's providers are configured with: their keys, their addresses, and
/// [`ANSWERS_VARIABLE`].
pub fn variables() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for provider in Provider::ALL {
        let about = provider.about();
        names.push(about.key.variable());
        names.extend(about.address.variable());
    }
    names.push(ANSWERS_VARIABLE);
    names
}

/// The effort sent for `asked` to a model that accepts `efforts`: the one asked for when the model has it, else the
/// nearest it has (the lower of two as near), else none at all.
pub fn effort_for(asked: Effort, efforts: &[Effort]) -> Option<Effort> {
    let rank = |effort: Effort| ALL.iter().position(|one| *one == effort).unwrap_or(0);
    efforts.iter().copied().min_by_key(|effort| (rank(*effort).abs_diff(rank(asked)), rank(*effort)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_claude_apis_newest_haiku_reads_papers_by_default_and_no_other_provider_has_a_default() {
        let anthropic = Provider::Anthropic.about();
        let haikus: Vec<&str> =
            anthropic.models.iter().map(|model| model.id).filter(|id| id.contains("haiku")).collect();
        assert_eq!(haikus.first(), Some(&CLAUDE_READING_DEFAULT), "the first Haiku listed, the newest");
        assert_eq!(anthropic.reading_default().map(|model| model.label), Some("Claude Haiku 5.5"));
        for provider in Provider::ALL.iter().filter(|provider| **provider != Provider::Anthropic) {
            assert!(provider.about().reading_default().is_none(), "{provider:?}");
        }
    }

    #[test]
    fn every_provider_has_its_own_row_found_by_its_id() {
        for provider in Provider::ALL {
            assert_eq!(provider.about().provider, provider);
            assert_eq!(Provider::from_id(provider.id()), Some(provider));
            let json = serde_json::to_value(provider).unwrap();
            assert_eq!(json, provider.id(), "the id stored is the one serialised");
        }
        assert_eq!(Provider::from_id("claude"), None);
        let mut ids: Vec<_> = Provider::ALL.iter().map(|provider| provider.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Provider::ALL.len());
    }

    #[test]
    fn every_address_is_https_and_every_listed_model_id_is_plain() {
        for provider in Provider::ALL {
            let about = provider.about();
            match about.address {
                Address::Fixed(default) | Address::Overridable { default, .. } => {
                    assert!(default.starts_with("https://"), "{default}")
                }
                Address::Required(_) => assert_eq!(provider, Provider::Compatible),
            }
            for model in about.models {
                assert!(plain_id(model.id), "{}", model.id);
                assert!(about.accepts(model.id));
            }
            assert_eq!(about.models.is_empty(), about.ids == ModelIds::Typed, "{}", about.id);
        }
    }

    #[test]
    fn a_typed_id_is_taken_only_where_ids_may_be_typed_and_only_when_plain() {
        assert!(!Provider::Anthropic.about().accepts("claude-opus-9"));
        assert!(Provider::OpenAi.about().accepts("gpt-7-nova"));
        assert!(Provider::Compatible.about().accepts("qwen3:32b"));
        for odd in ["", "../models", "a b", "x?key=1", "/v1", "model\n"] {
            assert!(!Provider::Compatible.about().accepts(odd), "{odd:?}");
        }
    }

    #[test]
    fn an_effort_a_model_lacks_becomes_the_nearest_it_has() {
        assert_eq!(effort_for(Effort::Medium, ALL), Some(Effort::Medium));
        assert_eq!(effort_for(Effort::Max, LOW_TO_HIGH), Some(Effort::High));
        assert_eq!(effort_for(Effort::Medium, LOW_HIGH_MAX), Some(Effort::Low));
        assert_eq!(effort_for(Effort::Xhigh, LOW_HIGH_MAX), Some(Effort::High), "of two as near, the lower");
        assert_eq!(effort_for(Effort::Max, LOW_HIGH_MAX), Some(Effort::Max));
        assert_eq!(effort_for(Effort::High, NONE), None);
    }

    #[test]
    fn the_variables_are_the_keys_and_addresses_of_every_provider() {
        let names = variables();
        for name in ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "OPENAI_API_KEY", "OPENAI_BASE_URL", "GEMINI_API_KEY"] {
            assert!(names.contains(&name), "{name}");
        }
        for name in ["CONSTELLATE_MODEL_BASE_URL", "CONSTELLATE_MODEL_API_KEY", ANSWERS_VARIABLE] {
            assert!(names.contains(&name), "{name}");
        }
    }

    #[test]
    fn what_a_provider_cannot_do_is_said_in_plain_words() {
        assert!(Provider::Anthropic.about().capabilities.limits().is_empty());
        let compatible = Provider::Compatible.about().capabilities.limits().join(" ");
        assert!(compatible.contains("checked here"));
        assert!(compatible.contains("estimated from the number of characters"));
        assert!(compatible.contains("in full for every question"));
    }
}
