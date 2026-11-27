"""The one place that talks to the Claude API.

A caller gives a model, a system prompt and a user prompt, optionally a JSON schema for the answer, and gets the
answer's text back or one of two exceptions:

    Unavailable   the API cannot be used at all (no credentials, unknown model): stop asking
    NoAnswer      this request produced nothing usable (rate limit, server error, refusal): the next one may
"""
import json

from . import usage

# Models offered in the interface: id -> (name as shown, a few words about it).
MODELS = {
    "claude-opus-5-5": ("Claude Opus 5.5", "the default"),
    "claude-sonnet-5-5": ("Claude Sonnet 5.5", "faster, about half the cost"),
    "claude-haiku-5-5": ("Claude Haiku 5.5", "cheapest, least thorough"),
    "claude-fable-5-2": ("Claude Fable 5.2", "most capable, about twice the cost"),
}
# Models for which the API can hand a declined request to another model within the same call.
FALLBACKS = {"claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-2"}


class Unavailable(Exception):
    pass


class NoAnswer(Exception):
    pass


def connect(model: str):
    """A client that has been shown to work with `model`.  Raises Unavailable."""
    try:
        import anthropic
        client = anthropic.Anthropic()
        client.models.retrieve(model)                    # fails fast when there are no usable credentials
        return client
    except Exception as error:                           # noqa: BLE001
        raise Unavailable(f"{type(error).__name__}: {str(error).splitlines()[0][:120]}") from error


def ask(client, model: str, system: str, prompt: str, schema: dict | None = None, effort="low",
        max_tokens=16000, purpose="other") -> str:
    """The text of the model's answer; with `schema`, that text is JSON matching it.  Raises NoAnswer.
    `purpose` says what the call is for in the ledger of what was spent (usage.py)."""
    import anthropic
    request = {"model": model, "max_tokens": max_tokens, "system": system,
               "messages": [{"role": "user", "content": prompt}],
               "output_config": {"effort": effort} | ({"format": {"type": "json_schema", "schema": schema}}
                                                      if schema else {})}
    messages = client.messages
    if model in FALLBACKS:                               # another model answers if this one declines
        messages = client.beta.messages
        request |= {"betas": ["server-side-fallback-2026-07-01"], "fallbacks": "default"}
    try:
        with messages.stream(**request) as stream:       # streamed: prompts here can be very long
            response = stream.get_final_message()
    except anthropic.RateLimitError as error:
        raise NoAnswer(f"rate limited ({error.message})") from error
    except anthropic.APIStatusError as error:
        raise NoAnswer(f"API error {error.status_code} ({error.message})") from error
    except anthropic.APIConnectionError as error:
        raise NoAnswer(f"connection error ({error})") from error
    # paid for whether or not the answer is usable; priced as the model that answered (a fallback may have)
    # A fallback's answer comes with the tokens of each model that worked on it, each billed at its own price.
    parts = [part for part in getattr(response.usage, "iterations", None) or []
             if isinstance(getattr(part, "model", None), str) and isinstance(getattr(part, "input_tokens", None), int)]
    if parts:
        for part in parts:
            usage.record(part.model, purpose, part.input_tokens, getattr(part, "output_tokens", 0) or 0)
    else:
        usage.record(getattr(response, "model", None) or model, purpose, response.usage.input_tokens,
                     response.usage.output_tokens)
    text = "".join(block.text for block in response.content if block.type == "text").strip()
    if response.stop_reason in ("refusal", "max_tokens") or not text:
        raise NoAnswer(f"no usable answer (stop_reason={response.stop_reason})")
    return text


def ask_json(client, model, system, prompt, schema, **options) -> dict:
    try:
        return json.loads(ask(client, model, system, prompt, schema, **options))
    except json.JSONDecodeError as error:
        raise NoAnswer(f"the answer was not valid JSON ({error})") from error
