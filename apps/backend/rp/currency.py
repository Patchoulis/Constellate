"""What a US dollar is worth in another currency today.

The Claude API bills in US dollars; the interface shows what was spent in the currency of the user's region.
Rates are the European Central Bank's reference rates, from frankfurter.dev (no key, nothing about the user is
sent but the currency's code), asked for once a day per currency and kept.  When a rate cannot be had (no
connection, a currency the bank does not list), the answer is the dollar itself, and the interface shows dollars.
"""

import re
import time

import requests

from . import LIBRARY, store

RATES = LIBRARY / "rates.json"  # {currency code: {"rate", "date"}}
SOURCE = "https://api.frankfurter.dev/v1/latest"
DOLLAR = {"code": "USD", "rate": 1.0, "date": None}
RETRY_SECONDS = 3600
_failed = {}  # currency code -> when asking for it last failed: not asked again for RETRY_SECONDS


def of(code: str | None) -> dict:
    """{"code", "rate": how many of it one US dollar buys, "date": the day the rate is from}."""
    code = (code or "USD").upper()
    if code == "USD" or not re.fullmatch(r"[A-Z]{3}", code):
        return DOLLAR
    today = time.strftime("%Y-%m-%d")
    kept = (store.read_json(RATES) or {}).get(code)
    if kept and kept.get("asked") == today:
        return _shown(code, kept)
    if time.time() - _failed.get(code, 0) < RETRY_SECONDS:
        return _shown(code, kept) if kept else DOLLAR
    try:
        answer = requests.get(SOURCE, params={"base": "USD", "symbols": code}, timeout=8).json()
        found = {"rate": float(answer["rates"][code]), "date": answer["date"], "asked": today}
    except (requests.RequestException, KeyError, TypeError, ValueError):
        _failed[code] = time.time()
        return _shown(code, kept) if kept else DOLLAR  # yesterday's rate is better than none
    try:
        store.update_json(RATES, lambda rates: rates.__setitem__(code, found))
    except OSError:
        pass  # not kept: it is asked for again next time
    return _shown(code, found)


def _shown(code: str, kept: dict) -> dict:
    return {"code": code, "rate": kept["rate"], "date": kept["date"]}
