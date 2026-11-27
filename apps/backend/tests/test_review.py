"""One test per defect a code review found, so none of them comes back unnoticed; and the detector's logic."""
import json
import threading

import numpy as np
import pytest

from conftest import paper
from rp import RUNS, cluster, compare, detect, graph, overview, server, settings, store
from rp.library import Papers, from_arxiv
from rp.selection import Selection


# ------------------------------------------------------------------ tree paths
def _paths(leaves):
    for leaf in leaves:
        yield leaf.path
        yield from _paths(leaf.children)


@pytest.mark.parametrize("method", list(cluster.METHODS))
def test_no_two_communities_share_a_path(method):
    """"c1" + "0" used to be the same name as community "c10"."""
    rng = np.random.default_rng(3)
    centres = rng.normal(size=(12, 24)) * 5
    points = (centres[np.repeat(np.arange(12), 30)] + rng.normal(size=(360, 24))).astype(np.float32)
    vectors = graph.unit(points - points.mean(axis=0))
    leaves, _ = cluster.communities(method, graph.adjacency_matrix(points), vectors, 12, min_size=5,
                                    min_modularity=-1.0, sub_levels=2, log=lambda line: None)
    paths = list(_paths(leaves))
    assert len(paths) > 12, "the fixture should produce sub-communities"
    assert len(paths) == len(set(paths))


def test_a_community_is_found_by_its_exact_path_only():
    leaf = lambda path, children=(): {"tree_path": path, "children": list(children)}   # noqa: E731
    first_half, tenth = leaf("c1.0"), leaf("c10", [leaf("c10.0")])
    roots = [leaf("c1", [first_half, leaf("c1.1")]), tenth]
    assert overview._find(roots, "c10") is tenth
    assert overview._find(roots, "c1.0") is first_half
    assert overview._find(roots, "c10.0") is tenth["children"][0]
    for missing in ("c", "c1.", "c100", "c1.0.0"):
        with pytest.raises(LookupError):
            overview._find(roots, missing)


# ------------------------------------------------------------------ files shared between threads
def test_concurrent_additions_to_a_cache_all_survive(tmp_path):
    path = tmp_path / "cache.json"
    workers = [threading.Thread(target=lambda n=n: [store.update_json(path, lambda d, k=f"{n}-{i}": d.__setitem__(k, 1))
                                                    for i in range(25)]) for n in range(8)]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join()
    assert len(store.read_json(path)) == 200
    assert not list(tmp_path.glob("*.tmp"))


def test_a_file_is_replaced_whole(tmp_path):
    path = tmp_path / "file.json"
    store.write_json(path, {"old": True})

    def fail(temporary):
        temporary.write_text("{half")
        raise RuntimeError("interrupted")

    with pytest.raises(RuntimeError):
        store.replace(path, fail)
    assert store.read_json(path) == {"old": True}                    # the old content is still there, whole
    assert store.read_json(tmp_path / "missing.json", "default") == "default"


# ------------------------------------------------------------------ a date range is arXiv submissions
def test_conference_papers_do_not_leak_into_date_ranges():
    library = Papers()
    library.add([paper("2512.01234", published="2025-12-03T10:00:00Z")])
    library.add([paper("neurips-2025-121923", published="2025-12-03T16:30:00-08:00")], venue="NeurIPS 2025")
    chosen = library.select(Selection.parse(start="2025-12-01", end="2025-12-07"))
    assert [p["id"] for p in chosen] == ["2512.01234"]
    assert from_arxiv({"id": "cs/0501001"}) and from_arxiv({"id": "2512.01234"})
    assert not from_arxiv({"id": "icml-2026-60472"})


# ------------------------------------------------------------------ settings are checked
@pytest.mark.parametrize("bad", [
    {"center_embeddings": "false"},            # a string is not a switch position
    {"max_communities": 0}, {"max_communities": "many"}, {"min_community_size": 0},
    {"clustering": "magic"}, {"embedding_model": ""}, {"min_split_modularity": 7},
])
def test_a_bad_setting_changes_nothing(bad):
    before = settings.load()
    with pytest.raises(ValueError):
        settings.save(bad | {"sub_levels": 1})                       # the good change must not go through either
    assert settings.load() == before


def test_good_settings_are_saved_with_their_types():
    saved = settings.save({"max_communities": "12", "center_embeddings": False, "clustering": "kmeans", "junk": 1})
    assert saved["max_communities"] == 12 and saved["center_embeddings"] is False and "junk" not in saved
    settings.save({"max_communities": 24, "center_embeddings": True, "clustering": "spectral"})


# ------------------------------------------------------------------ the API answers only this machine's own app
def test_other_sites_and_other_hosts_are_refused():
    client = server.app.test_client()
    assert client.get("/api/settings").status_code == 200
    assert client.put("/api/settings", json={}, headers={"Origin": "http://localhost:3210"}).status_code == 200
    assert client.put("/api/settings", json={}, headers={"Origin": "https://evil.example"}).status_code == 403
    assert client.post("/api/runs/x/overview/c0", headers={"Origin": "https://evil.example"}).status_code == 403
    assert client.get("/api/settings", headers={"Host": "evil.example"}).status_code == 403    # DNS rebinding


# ------------------------------------------------------------------ comparison
def test_a_score_nobody_has_ranks_nobody():
    rows = [{"k": 8, "method": method, "AMI fine": float("nan"), "AMI coarse": float("nan"),
             "coherence": score, "subsample stability": score}
            for method, score in zip(cluster.METHODS, (0.1, 0.4, 0.3, 0.2))]
    ranking = compare._ranking(rows)
    assert [method for _, method in ranking] == ["spectral-knn", "kmeans", "fiedler", "spectral"]
    assert ranking[0][0] == 1.0 and ranking[-1][0] == 4.0


# ------------------------------------------------------------------ eigenvectors
def test_leading_eigenvectors_match_the_dense_computation_and_repeat():
    rng = np.random.default_rng(5)
    matrix = np.abs(rng.normal(size=(60, 60))).astype(np.float32)
    matrix = (matrix + matrix.T) / 2
    np.fill_diagonal(matrix, 0)
    vectors, scale = graph.top_eigenvectors(matrix, 3)
    dense = np.linalg.eigh(scale[:, None] * matrix.astype(np.float64) * scale[None, :])[1][:, -3:]
    assert np.allclose(np.abs(np.sum(vectors * dense, axis=0)), 1, atol=1e-3)     # same vectors, up to sign
    again, _ = graph.top_eigenvectors(matrix, 3)
    np.testing.assert_array_equal(vectors, again)                                 # no randomness


# ------------------------------------------------------------------ the detector
HUMAN = "we study a model and show results on data with a simple method and report error rates for several tasks "
MACHINE = "this paper delves into an intricate landscape showcasing pivotal insights that underscore robust novel advances "


def _text(base, n):
    return " ".join((base * 6).split() + [f"topic{n}"])


def test_classifier_separates_wording_and_names_the_words():
    model = detect.Classifier([_text(HUMAN, n) for n in range(30)], [_text(MACHINE, n) for n in range(30)])
    human, machine, short = model.score([_text(HUMAN, 99), _text(MACHINE, 99), "too short to judge"])
    assert machine > human and np.isnan(short)
    assert {"delves", "intricate", "pivotal"} & set(model.markers(_text(MACHINE, 99)))
    assert model.markers(_text(HUMAN, 99)) == [] or not {"delves", "pivotal"} & set(model.markers(_text(HUMAN, 99)))
    assert model.version == detect.Classifier([_text(HUMAN, n) for n in range(30)],
                                              [_text(MACHINE, n) for n in range(30)]).version


def test_held_out_papers_are_chosen_by_id_and_are_about_a_fifth():
    ids = [f"19{n:02d}.{n:05d}" for n in range(2000)]
    share = np.mean([detect._held_out(i) for i in ids])
    assert 0.15 < share < 0.25
    assert detect._held_out("1906.00001") == detect._held_out("1906.00001")


def test_without_a_trained_detector_nothing_is_flagged(monkeypatch, tmp_path):
    monkeypatch.setattr(detect, "TRAINING", tmp_path / "none.json")
    lines = []
    detector = detect.Detector(log=lines.append)
    assert np.isnan(detector.scores([paper("a"), paper("b")])).all()
    assert detector.threshold == float("inf") and detector.verdict(paper("a")) is None
    assert any("no detector" in line for line in lines)


def test_detector_scores_each_abstract_once_per_classifier(monkeypatch, tmp_path):
    calls = []

    class FakeClassifier:
        version, threshold = "v1", 1.0

        def score(self, texts):
            texts = list(texts)
            calls.extend(texts)
            return np.array([3.0 if "delves" in text else (np.nan if not text else -1.0) for text in texts])

        def markers(self, text):
            return ["delves"]

    monkeypatch.setattr(detect, "SCORES", tmp_path / "scores.json")
    monkeypatch.setattr(detect, "classifier", lambda: FakeClassifier())
    papers = [paper("h", abstract="plain words"), paper("m", abstract="it delves"), paper("e", abstract="")]
    scores = detect.Detector(log=lambda line: None).scores(papers)
    assert scores[0] == -1 and scores[1] == 3 and np.isnan(scores[2])
    detector = detect.Detector(log=lambda line: None)
    assert detector.verdict(papers[1]) == {"score": 3.0, "threshold": 1.0, "flagged": True, "false_alarms": 0.01,
                                            "markers": ["delves"]}
    assert not detector.verdict(papers[0])["flagged"] and detector.verdict(papers[2]) is None
    assert detector.verdict(paper("m", abstract="it delves, revised")) is None      # not the text that was scored
    detector.scores(papers)
    assert len(calls) == 3                                                           # nothing scored twice
    FakeClassifier.version = "v2"
    detect.Detector(log=lambda line: None).scores(papers)
    assert len(calls) == 6                                                           # a new classifier scores afresh


def test_mining_adds_the_humans_scored_most_machine_like(monkeypatch, tmp_path):
    monkeypatch.setattr(detect, "TRAINING", tmp_path / "training.json")
    monkeypatch.setattr(detect, "START", 20)
    monkeypatch.setattr(detect, "MINED", 5)
    monkeypatch.setattr(detect, "ROUNDS", 1)
    monkeypatch.setattr(detect, "HELD_OUT_MIRRORS", 10)
    monkeypatch.setattr(detect.claude, "connect", lambda model: object())
    monkeypatch.setattr(detect.claude, "ask", lambda client, model, system, prompt, **options: _text(MACHINE, len(prompt)))
    pool = [{"id": f"1906.{n:05d}", "title": f"Title {n}", "abstract": _text(HUMAN, n)} for n in range(120)]
    store.write_json(detect.TRAINING, {"pool": pool + [pool[0]] * (detect.POOL - len(pool)), "machine": [], "train": [], "rounds": []})
    data = detect.Trainer(log=lambda line: None).build()
    mirrored = {m["id"] for m in data["machine"]}
    assert set(data["train"]) <= mirrored                               # every training human has its mirrors
    assert not any(detect._held_out(paper_id) for paper_id in data["train"])
    assert {m["kind"] for m in data["machine"]} == {"written", "polished"}
    assert data["rounds"][0] == 20 and data["train"][:20] == [p["id"] for p in pool if not detect._held_out(p["id"])][:20]
    report = detect.report(log=lambda line: None)
    assert report["rounds"][0] == {"round": 0, "humans": 20, "written_caught": 1.0, "polished_caught": 1.0}
    assert report["trained_on"] == len(data["train"]) and report["false_alarms"] == 0.01
    again = detect.Trainer(log=lambda line: None)
    made = len(again.data["machine"])
    again._mirror([p for p in pool if p["id"] in mirrored])
    assert len(again.data["machine"]) == made                           # a mirror that exists is not paid for again
