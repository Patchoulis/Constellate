"""What the app has spent on the Claude API: every call's tokens, priced, kept in a ledger.

The price of a call is its tokens times the model's price per million (PRICES, the published list prices; a
model not listed is counted in tokens but not in dollars, and the summary says so).  The ledger holds totals per
model and per purpose (naming communities, overviews, training the AI-text detector), not individual calls.
"""

import os
import re
import threading
import time
from urllib.parse import urlparse

from . import LIBRARY, store

LEDGER = LIBRARY / "usage.json"
# model -> (input, output) US dollars per million tokens.  Haiku 5.5 has a second rate for prompts over 100K tokens.
PRICES = {
    "claude-opus-5-5": (4.0, 20.0),
    "claude-sonnet-5-5": (2.0, 10.0),
    "claude-haiku-5-5": (0.10, 0.50),
    "claude-fable-5-2": (8.0, 40.0),
    # what a declined request may fall back to
    "claude-opus-5": (5.0, 25.0),
    "claude-opus-4-8": (5.0, 25.0),
    "claude-sonnet-5": (2.0, 10.0),
}
LONG_PROMPT = {"claude-haiku-5-5": (100_000, (0.50, 2.50))}

_session = {"usd": 0.0, "calls": 0}  # since this server started
_lock = threading.Lock()


def price(model: str, input_tokens: int, output_tokens: int) -> float | None:
    """What a call cost in US dollars, or None for a model whose price is not known here."""
    model = re.sub(r"-\d{8}$", "", model)  # the API may answer with a dated id: claude-opus-5-5-20261101
    if model not in PRICES:
        return None
    per_input, per_output = PRICES[model]
    limit, long_rates = LONG_PROMPT.get(model, (None, None))
    if limit is not None and input_tokens > limit:
        per_input, per_output = long_rates
    return (input_tokens * per_input + output_tokens * per_output) / 1e6


def record(model: str, purpose: str, input_tokens: int, output_tokens: int):
    """Add one call to the ledger.  Never raises: a ledger that cannot be written (the file is held by a sync
    program, say) must not lose an answer that has been paid for."""
    usd = price(model, input_tokens, output_tokens)
    with _lock:
        _session["usd"] += usd or 0.0
        _session["calls"] += 1

    def add(ledger):
        ledger.setdefault("since", time.strftime("%Y-%m-%d"))
        for table, key in (("models", model), ("purposes", purpose)):
            entry = ledger.setdefault(table, {}).setdefault(
                key, {"calls": 0, "input_tokens": 0, "output_tokens": 0, "usd": 0.0, "unpriced_calls": 0}
            )
            entry["calls"] += 1
            entry["input_tokens"] += input_tokens
            entry["output_tokens"] += output_tokens
            entry["usd"] += usd or 0.0  # not rounded here: thousands of tiny calls would drift
            entry["unpriced_calls"] += usd is None

    try:
        store.update_json(LEDGER, add)
    except OSError as error:
        print(f"usage: could not write the ledger ({error}); this call is counted for this session only")


def provider() -> dict:
    """Who answers the app's requests, as far as the environment says: {"name", "ready"}."""
    base = os.environ.get("ANTHROPIC_BASE_URL")
    host = urlparse(base).hostname if base else None
    name = "Anthropic API" if not host or host.endswith("anthropic.com") else host
    return {"name": name, "ready": bool(os.environ.get("ANTHROPIC_API_KEY") or os.environ.get("ANTHROPIC_AUTH_TOKEN"))}


def summary() -> dict:
    ledger = store.read_json(LEDGER) or {}
    models = ledger.get("models", {})
    return {
        "provider": provider(),
        "since": ledger.get("since"),
        "usd": round(sum(entry["usd"] for entry in models.values()), 4),
        "calls": sum(entry["calls"] for entry in models.values()),
        "unpriced_calls": sum(entry["unpriced_calls"] for entry in models.values()),
        "session_usd": round(_session["usd"], 4),
        "session_calls": _session["calls"],
        "models": models,
        "purposes": ledger.get("purposes", {}),
    }
