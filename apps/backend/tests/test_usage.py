"""What the Claude API cost: priced per call, kept, and summed."""
import pytest

from rp import server, store, usage


def test_a_call_is_priced_by_its_model_and_its_length():
    assert usage.price("claude-opus-5-5", 1_000_000, 100_000) == pytest.approx(4.0 + 2.0)
    assert usage.price("claude-haiku-5-5", 50_000, 10_000) == pytest.approx(0.005 + 0.005)
    assert usage.price("claude-haiku-5-5", 200_000, 10_000) == pytest.approx(0.10 + 0.025)     # the long-prompt rate
    assert usage.price("some-other-model", 10, 10) is None


def test_calls_add_up_by_model_and_by_purpose(monkeypatch):
    store.write_json(usage.LEDGER, {})
    monkeypatch.setattr(usage, "_session", {"usd": 0.0, "calls": 0})
    usage.record("claude-opus-5-5", "naming communities", 10_000, 1_000)
    usage.record("claude-opus-5-5", "community overviews", 100_000, 5_000)
    usage.record("some-other-model", "naming communities", 500, 500)
    told = usage.summary()
    assert told["calls"] == 3 and told["session_calls"] == 3 and told["unpriced_calls"] == 1
    assert told["usd"] == pytest.approx(0.06 + 0.5) == told["session_usd"]
    assert told["models"]["claude-opus-5-5"]["input_tokens"] == 110_000
    assert told["purposes"]["naming communities"]["calls"] == 2
    assert told["since"]


def test_the_interface_is_told_who_provides_the_model(monkeypatch):
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    monkeypatch.delenv("ANTHROPIC_AUTH_TOKEN", raising=False)
    monkeypatch.delenv("ANTHROPIC_BASE_URL", raising=False)
    assert usage.provider() == {"name": "Anthropic API", "ready": False}
    monkeypatch.setenv("ANTHROPIC_API_KEY", "test")
    monkeypatch.setenv("ANTHROPIC_BASE_URL", "https://gateway.example.org/v1")
    assert usage.provider() == {"name": "gateway.example.org", "ready": True}
    told = server.app.test_client().get("/api/usage").get_json()
    assert told["model"]["label"].startswith("Claude") and told["provider"]["ready"] is True


def test_the_model_that_answered_finds_its_price_whatever_the_api_calls_it():
    assert usage.price("claude-opus-5-5-20261101", 1_000_000, 0) == pytest.approx(4.0)
    assert usage.price("claude-sonnet-5", 1_000_000, 0) == pytest.approx(2.0)      # what Sonnet 5.5 falls back to
    assert usage.price("claude-opus-5", 0, 1_000_000) == pytest.approx(25.0)


def test_many_tiny_calls_do_not_drift(monkeypatch):
    store.write_json(usage.LEDGER, {})
    monkeypatch.setattr(usage, "_session", {"usd": 0.0, "calls": 0})
    for _ in range(320):
        usage.record("claude-haiku-5-5", "naming communities", 7, 3)
    told = usage.summary()
    assert told["models"]["claude-haiku-5-5"]["usd"] == pytest.approx(320 * (7 * 0.10 + 3 * 0.50) / 1e6)
    assert told["usd"] == told["session_usd"]


def test_a_ledger_that_cannot_be_written_does_not_lose_the_call(monkeypatch, capsys):
    monkeypatch.setattr(usage, "_session", {"usd": 0.0, "calls": 0})

    def held(path, change):
        raise PermissionError("the file is in use")

    monkeypatch.setattr(usage.store, "update_json", held)
    usage.record("claude-opus-5-5", "naming communities", 1_000_000, 0)            # does not raise
    assert usage._session == {"usd": pytest.approx(4.0), "calls": 1}
    assert "could not write the ledger" in capsys.readouterr().out


def test_an_answer_a_fallback_helped_with_is_priced_model_by_model(monkeypatch):
    from types import SimpleNamespace as N

    from rp import claude

    recorded = []
    monkeypatch.setattr(claude.usage, "record", lambda *call: recorded.append(call))
    usage_ = N(input_tokens=1500, output_tokens=300, iterations=[
        N(model="claude-opus-5-5", input_tokens=1000, output_tokens=50),
        N(model="claude-opus-5", input_tokens=1000, output_tokens=250)])
    response = N(model="claude-opus-5", usage=usage_, stop_reason="end_turn", content=[N(type="text", text="An answer")])

    class Stream:
        def __enter__(self):
            return N(get_final_message=lambda: response)

        def __exit__(self, *_):
            return False

    client = N(beta=N(messages=N(stream=lambda **request: Stream())), messages=None)
    assert claude.ask(client, "claude-opus-5-5", "system", "prompt", purpose="community overviews") == "An answer"
    assert recorded == [("claude-opus-5-5", "community overviews", 1000, 50), ("claude-opus-5", "community overviews", 1000, 250)]

    recorded.clear()
    usage_.iterations = None                                                        # an ordinary answer: one entry
    claude.ask(client, "claude-opus-5-5", "system", "prompt")
    assert recorded == [("claude-opus-5", "other", 1500, 300)]
