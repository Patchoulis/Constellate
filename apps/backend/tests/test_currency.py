"""What a US dollar is worth in the user's currency: asked once a day, and the dollar itself when it cannot be had."""

import requests

from rp import currency, server, store


class _Answer:
    def __init__(self, body):
        self.body = body

    def json(self):
        return self.body


def test_a_rate_is_asked_for_once_a_day_and_kept(monkeypatch):
    store.write_json(currency.RATES, {})
    monkeypatch.setattr(currency, "_failed", {})
    asked = []

    def get(url, params, timeout):
        asked.append(params)
        return _Answer({"base": "USD", "date": "2026-11-26", "rates": {"CAD": 1.423}})

    monkeypatch.setattr(currency.requests, "get", get)
    assert currency.of("cad") == {"code": "CAD", "rate": 1.423, "date": "2026-11-26"}
    assert currency.of("CAD")["rate"] == 1.423
    assert asked == [{"base": "USD", "symbols": "CAD"}]  # nothing about the user, and only once


def test_the_dollar_and_nonsense_need_no_asking(monkeypatch):
    monkeypatch.setattr(currency.requests, "get", lambda *a, **k: (_ for _ in ()).throw(AssertionError("asked")))
    for code in ("USD", "usd", None, "", "dollars", "C$", "../x"):
        assert currency.of(code) == currency.DOLLAR


def test_without_a_connection_the_last_rate_is_used_or_else_the_dollar(monkeypatch):
    def offline(*a, **k):
        raise requests.ConnectionError("offline")

    monkeypatch.setattr(currency.requests, "get", offline)
    monkeypatch.setattr(currency, "_failed", {})
    store.write_json(currency.RATES, {"EUR": {"rate": 0.91, "date": "2026-11-20", "asked": "2026-11-20"}})
    assert currency.of("EUR") == {"code": "EUR", "rate": 0.91, "date": "2026-11-20"}
    assert currency.of("GBP") == currency.DOLLAR
    monkeypatch.setattr(currency.requests, "get", lambda *a, **k: _Answer({"message": "not found"}))
    assert currency.of("XXX") == currency.DOLLAR  # a currency the bank does not list


def test_the_interface_is_told_the_rate_with_what_was_spent(monkeypatch):
    monkeypatch.setattr(currency, "of", lambda code: {"code": code, "rate": 2.0, "date": "2026-11-26"})
    told = server.app.test_client().get("/api/usage?currency=CAD").get_json()
    assert told["currency"] == {"code": "CAD", "rate": 2.0, "date": "2026-11-26"}


def test_a_rate_that_could_not_be_had_is_not_asked_for_again_at_once(monkeypatch):
    store.write_json(currency.RATES, {})
    monkeypatch.setattr(currency, "_failed", {})
    asked = []

    def offline(*a, **k):
        asked.append(1)
        raise requests.ConnectionError("offline")

    monkeypatch.setattr(currency.requests, "get", offline)
    for _ in range(5):  # the interface asks every few seconds
        assert currency.of("SEK") == currency.DOLLAR
    assert len(asked) == 1
