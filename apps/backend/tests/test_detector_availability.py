"""Whether abstracts get scored: off, on but untrained, or ready; and what a run does in each."""

import numpy as np

from conftest import paper
from rp import detect, machine, pipeline, server, settings
from rp.selection import Selection


def test_exactly_one_state_holds_and_each_says_why(monkeypatch):
    monkeypatch.setattr(detect, "classifier", lambda: None)
    assert detect.availability(False)["state"] is detect.Availability.OFF
    untrained = detect.availability(True)
    assert untrained["state"] is detect.Availability.UNTRAINED and "does not need a graphics card" in untrained["why"]
    monkeypatch.setattr(detect, "classifier", lambda: object())
    assert detect.availability(True)["state"] is detect.Availability.READY
    assert detect.availability(False)["state"] is detect.Availability.OFF  # switched off wins over trained
    assert all(detect.availability(on)["why"] for on in (True, False))


def _run(enabled):
    lines = []
    run = pipeline.Run(Selection.parse(venues=["X 2026"]), settings.load() | {"detect_ai": enabled}, log=lines.append)
    run.papers = [paper(f"x-2026-{n}") for n in range(5)]
    return run, lines


def test_a_run_tags_nothing_when_the_detector_is_off_or_untrained(monkeypatch):
    monkeypatch.setattr(detect, "classifier", lambda: None)
    for enabled, word in ((False, "Switched off"), (True, "Not available yet")):
        run, lines = _run(enabled)
        pipeline.detect_ai(run)
        assert run.flagged.dtype == bool and run.flagged.tolist() == [False] * 5
        assert word in lines[0]


def test_a_run_scores_abstracts_when_the_detector_is_ready(monkeypatch):
    class Scored:
        threshold = 0.5

        def __init__(self, log):
            pass

        def scores(self, papers):
            return np.array([0.9, 0.1, np.nan, 0.6, 0.5])

    monkeypatch.setattr(detect, "classifier", lambda: object())
    monkeypatch.setattr(detect, "Detector", Scored)
    run, _ = _run(True)
    pipeline.detect_ai(run)
    assert run.flagged.tolist() == [True, False, False, True, False]  # no verdict and the threshold itself: not tagged


def test_the_setting_is_a_switch_and_the_interface_is_told_the_state(monkeypatch):
    assert settings.save({"detect_ai": False})["detect_ai"] is False
    monkeypatch.setattr(machine, "_asked", {"gpu": None})
    told = server.app.test_client().get("/api/library").get_json()
    assert told["ai_detector"]["state"] == "off"
    assert told["machine"]["gpu"] is None and "processor" in told["machine"]["embedding"]
    settings.save({"detect_ai": True})
    monkeypatch.setattr(machine, "_asked", {"gpu": "NVIDIA Test Card"})
    assert "NVIDIA Test Card" in server.app.test_client().get("/api/library").get_json()["machine"]["embedding"]
