"""The parts that sit between the pipeline and the outside world, with the outside world replaced."""
import json

import numpy as np
import pytest

from conftest import paper
from rp import RUNS, conferences, describe, detect, overview, server
from rp.library import Papers


# ------------------------------------------------------------------ conference lists
def _entry(event_id, name, kind="Poster", forum=None, **more):
    return {"id": event_id, "name": name, "eventtype": kind, "authors": [{"fullname": "A. Author"}],
            "paper_url": f"https://openreview.net/forum?id={forum}" if forum else None,
            "topic": "Theory->Learning", "decision": "Accept", "starttime": "2025-12-03T10:00:00", **more}


def test_an_oral_is_not_a_second_paper(monkeypatch):
    listing = {"results": [
        _entry(1, "Paper One", forum="abc", abstract="first"),
        _entry(2, "Paper One", kind="Oral"),                         # the same paper's talk: no link at all
        _entry(3, "Paper  one", kind="Oral", forum="made-up"),       # ... or a made-up one, and other spacing
        _entry(4, "Paper Two", forum="xyz"),
    ]}
    monkeypatch.setattr(conferences, "_data", lambda host, slug, year, part, required=True:
                        listing if part == "orals-posters" else {"4": "second, from the abstracts file"})
    found = conferences.accepted_papers("NeurIPS 2025", log=lambda line: None)
    assert [p["id"] for p in found] == ["neurips-2025-1", "neurips-2025-4"]
    assert found[0]["abstract"] == "first"
    assert found[1]["abstract"] == "second, from the abstracts file"
    assert found[0]["pdf_url"] == "https://openreview.net/pdf?id=abc"
    assert found[0]["primary_category"] == "Theory->Learning"


def test_a_conference_without_an_official_list_says_so():
    with pytest.raises(conferences.NoOfficialList):
        conferences.accepted_papers("ACL 2025", log=lambda line: None)


# ------------------------------------------------------------------ descriptions
def test_description_answer_is_parsed():
    assert describe._parse("NAME: Graph Learning\nDESCRIPTION: Papers on graphs.\nMore.") == {
        "name": "Graph Learning", "description": "Papers on graphs.\nMore."}
    with pytest.raises(describe.claude.NoAnswer):
        describe._parse("I would rather not.")


def test_characteristic_terms_are_the_ones_a_group_has_and_others_lack():
    papers = ([paper(f"g{i}", title="graph neural network", abstract="message passing on graphs") for i in range(6)]
              + [paper(f"d{i}", title="diffusion model", abstract="denoising score sampling") for i in range(6)])
    vocabulary = describe.Vocabulary(papers)
    graph_terms = vocabulary.keywords(np.arange(6), top=4)
    assert "graph" in graph_terms or "graphs" in graph_terms
    assert not {"diffusion", "denoising"} & set(graph_terms)
    # against the graph papers alone, "graph" no longer sets a graph paper apart: its weight is zero
    inside = vocabulary._share(vocabulary.per_paper[np.arange(3)])
    around = vocabulary._share(vocabulary.per_paper[np.arange(6)])
    assert np.allclose(inside, around)


def test_a_described_group_is_not_asked_about_again(monkeypatch):
    calls = []
    monkeypatch.setattr(describe.claude, "connect", lambda model: object())
    monkeypatch.setattr(describe.claude, "ask", lambda client, model, system, prompt, **options:
                        (calls.append(prompt), "NAME: A Name\nDESCRIPTION: A description.")[-1])
    papers = [paper(f"p{i}") for i in range(4)]
    embeddings = np.eye(4, dtype=np.float32)
    groups = [np.array([0, 1]), np.array([2, 3])]
    arguments = (papers, embeddings, groups, describe.centroids(embeddings, groups), [["a"], ["b"]])
    first = describe.Describer("test-model", log=lambda line: None).describe(*arguments)
    assert [text["name"] for text in first] == ["A Name", "A Name"] and len(calls) == 2
    describe.Describer("test-model", log=lambda line: None).describe(*arguments)
    assert len(calls) == 2                                           # both came from the library
    describe.Describer("other-model", log=lambda line: None).describe(*arguments)
    assert len(calls) == 4                                           # another model is another question


def test_no_credentials_means_no_descriptions_and_one_notice(monkeypatch):
    def refuse(model):
        raise describe.claude.Unavailable("no key")
    monkeypatch.setattr(describe.claude, "connect", refuse)
    lines = []
    describer = describe.Describer("unavailable-model", log=lines.append)
    papers, embeddings = [paper("x1"), paper("x2")], np.eye(2, dtype=np.float32)
    for _ in range(3):
        assert describer.describe(papers, embeddings, [np.array([0]), np.array([1])], embeddings, [["a"], ["b"]]) == [None, None]
    assert sum("not available" in line for line in lines) == 1


# ------------------------------------------------------------------ overviews: citations are checked
def test_overview_keeps_only_points_with_enough_real_citations():
    papers = [paper(f"p{i}", title=f"Paper {i}") for i in range(10)]
    members = [2, 4, 6, 8]                                           # the community: numbered 1..4 in the prompt
    empty = {key: [] for key in overview.SECTIONS}
    answer = empty | {"title": "T", "summary": "S", "common_methods": [
        {"title": "well supported", "insight": "i", "papers": [1, 2, 3, 3, 4]},     # a repeat counts once
        {"title": "too thin", "insight": "i", "papers": [1, 2]},
        {"title": "invented numbers", "insight": "i", "papers": [1, 2, 99, 0, -3]},
    ]}
    checked = overview._checked(answer, members, papers)
    assert [section["heading"] for section in checked["sections"]] == ["Common methods"]
    (point,) = checked["sections"][0]["points"]
    assert point["title"] == "well supported"
    assert [p["id"] for p in point["papers"]] == ["p2", "p4", "p6", "p8"]


def test_overview_prompt_numbers_every_member_and_reports_structure():
    papers = [paper(f"p{i}", title=f"Title {i}", abstract=f"Abstract {i}") for i in range(6)]
    child = {"name": "Half", "keywords": ["k"], "size": 2, "members": [0, 1], "children": [], "tree_path": "c00"}
    community = {"name": "Mine", "keywords": ["a", "b"], "size": 3, "members": [0, 1, 2], "children": [child],
                 "tree_path": "c0", "nearest_to_centroid": [{"id": "p1"}]}
    other = {"name": "Theirs", "keywords": ["z"], "size": 3, "members": [3, 4, 5], "children": [], "tree_path": "c1",
             "nearest_to_centroid": []}
    prompt = overview._prompt(community, [community, other], {"edges": [[0, 1, 0.9], [2, 3, 0.5]]}, papers)
    for number in (1, 2, 3):
        assert f"[{number}] Title {number - 1}" in prompt
    assert "Title 4" not in prompt                                   # not a member
    assert "Theirs (1 links)" in prompt and 'Sub-community "Half"' in prompt
    assert overview._find([community, other], "c00") is child
    with pytest.raises(LookupError):
        overview._find([community, other], "c9")


# ------------------------------------------------------------------ the HTTP API
@pytest.fixture
def client():
    folder = RUNS / "demo"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "run.json").write_text(json.dumps({"selection": {"venues": ["X 2026"]}, "settings": {}, "paper_ids": []}))
    (folder / "communities.json").write_text(json.dumps({"name": "demo", "papers": 0, "modularity": 0.1,
                                                         "communities": []}))
    (folder / "graph.json").write_text(json.dumps({"nodes": [], "edges": []}))
    return server.app.test_client()


def test_runs_are_listed_renamed_and_deleted(client):
    assert client.get("/api/runs").get_json()[0] == {"name": "demo", "title": "demo", "papers": 0, "communities": 0,
                                                     "modularity": 0.1}
    assert client.patch("/api/runs/demo", json={"title": "  My   run "}).get_json()["title"] == "My run"
    assert client.get("/api/runs").get_json()[0]["title"] == "My run"
    assert client.patch("/api/runs/demo", json={"title": ""}).get_json()["title"] == "demo"
    assert client.delete("/api/runs/demo").status_code == 200
    assert client.get("/api/runs/demo").status_code == 404


@pytest.mark.parametrize("name", ["..", ".", "nope", "demo/..", "../demo", "demo/../demo"])
def test_only_real_runs_can_be_touched(name):
    """Asked directly, without the router's own filtering of odd paths in front of it."""
    (RUNS / "demo").mkdir(parents=True, exist_ok=True)
    (RUNS / "demo" / "run.json").write_text("{}")
    (RUNS.parent / "run.json").write_text("{}")                      # a decoy one level up
    with pytest.raises(LookupError):
        server._run_folder(name)
    assert server._run_folder("demo") == (RUNS / "demo").resolve()


def test_bad_requests_are_explained_not_crashed(client):
    answer = client.post("/api/runs", json={"start": "2026-02-01", "end": "2026-01-01"})
    assert answer.status_code == 400 and "after" in answer.get_json()["error"]
    assert client.post("/api/runs", json={"rerun": "nope"}).status_code == 404
    assert client.get("/api/papers/no-such-paper").status_code == 404
    assert client.get("/api/pdf/no-such-paper").status_code == 404


def test_settings_keep_their_types_and_ignore_unknown_keys(client):
    saved = client.put("/api/settings", json={"max_communities": "12", "clustering": "kmeans", "nonsense": 1}).get_json()
    assert saved["max_communities"] == 12 and saved["clustering"] == "kmeans" and "nonsense" not in saved
    assert client.get("/api/settings").get_json() == saved


def test_catalog_offers_what_the_backend_can_do(client):
    catalog = client.get("/api/catalog").get_json()
    assert {item["name"] for item in catalog["clustering"]} == set(server.pipeline.cluster.METHODS)
    assert any(item["name"] == "NeurIPS" and item["note"] == "official list"
               for group in catalog["conferences"] for item in group["items"])
    assert catalog["embedding_models"] and catalog["describe_models"]


def test_a_paper_comes_with_its_detector_verdict(client):
    library = Papers()
    library.add([paper("known")])
    library.save()
    answer = client.get("/api/papers/known").get_json()
    assert answer["title"] == "A title" and answer["ai"] is None      # never scored: no verdict, not "not AI"


def test_an_overview_is_looked_up_by_exact_path(client):
    folder = RUNS / "demo"
    community = lambda path, members, children=(): {                  # noqa: E731
        "tree_path": path, "name": path, "keywords": [], "size": len(members), "members": members,
        "children": list(children), "nearest_to_centroid": []}
    (folder / "communities.json").write_text(json.dumps({"name": "demo", "papers": 4, "modularity": 0, "communities": [
        community("c1", [0, 1], [community("c1.0", [0]), community("c1.1", [1])]), community("c10", [2, 3])]}))
    (folder / "run.json").write_text(json.dumps({"selection": {}, "settings": {}, "paper_ids": ["a", "b", "c", "d"]}))
    library = Papers()
    library.add([paper(name) for name in "abcd"])
    library.save()
    assert client.get("/api/runs/demo/overview/c10").get_json()["state"] == "idle"
    assert client.get("/api/runs/demo/overview/c1.0").get_json()["state"] == "idle"
    assert client.get("/api/runs/demo/overview/c100").status_code == 404
