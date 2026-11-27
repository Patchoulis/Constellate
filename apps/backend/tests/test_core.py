"""The parts of the pipeline that are pure logic: selections, the library's caching rules, the graph, clustering."""

import numpy as np
import pytest
from sklearn.metrics import adjusted_rand_score

from conftest import paper
from rp import cluster, graph
from rp.jobs import Busy, Job, State
from rp.library import Embeddings, Papers, text_hash
from rp.selection import Selection


# ------------------------------------------------------------------ selection
def test_selection_from_conferences():
    selection = Selection.parse(venues=[" NeurIPS 2025 ", "", "ICML 2026"])
    assert selection.venues == ("NeurIPS 2025", "ICML 2026")
    assert selection.name == "neurips2025_icml2026"


def test_selection_from_dates():
    selection = Selection.parse(start="2026-01-01", end="2026-01-07", categories=["cs.LG", " "])
    assert selection.name == "arxiv_2026-01-01_2026-01-07_cs.LG"
    assert selection.categories == ("cs.LG",)


@pytest.mark.parametrize(
    "bad",
    [
        {"venues": ["NeurIPS"]},  # no year
        {"start": "2026-02-01", "end": "2026-01-01"},  # backwards
        {"start": "soon", "end": "later"},
        {},
    ],
)
def test_selection_refuses(bad):
    with pytest.raises(ValueError):
        Selection.parse(**bad)


def test_long_conference_list_gets_a_short_stable_name():
    venues = [f"Conf{i} 2026" for i in range(12)]
    name = Selection.parse(venues=venues).name
    assert len(name) < 80
    assert name == Selection.parse(venues=venues).name
    assert name != Selection.parse(venues=venues[:-1]).name


def test_selection_survives_its_own_json():
    selection = Selection.parse(start="2026-01-01", end="2026-01-07", categories=["cs.LG"])
    assert Selection.parse(**selection.to_json()) == selection


# ------------------------------------------------------------------ library: nothing is done twice
def test_a_known_paper_is_not_stored_again():
    library = Papers()
    assert library.add([paper("a", title="first")]) == (1, 0)
    assert library.add([paper("a", title="second")]) == (0, 1)
    assert library.by_id["a"]["title"] == "first"
    library.add([paper("a", title="second")], overwrite=True)
    assert library.by_id["a"]["title"] == "second"


def test_a_venue_list_is_the_whole_venue():
    library = Papers()
    library.add([paper("a"), paper("b")], venue="X 2026")
    library.add([paper("b"), paper("c")], venue="X 2026")  # the list changed: "a" is no longer in it
    assert library.by_id["a"]["venues"] == []
    assert [p["id"] for p in library.select(Selection.parse(venues=["X 2026"]))] == ["b", "c"]


def test_library_is_read_back_from_disk():
    library = Papers()
    library.add([paper("kept")], venue="Y 2026")
    library.save(finished="slice")
    again = Papers()
    assert "kept" in again.by_id and "slice" in again.done


def test_selecting_by_date_and_category():
    library = Papers()
    library.add(
        [
            paper("in", published="2026-03-02T10:00:00Z"),
            paper("late", published="2026-03-09T10:00:00Z"),
            paper("other", published="2026-03-02T10:00:00Z", categories=["math.PR"]),
        ]
    )
    chosen = library.select(Selection.parse(start="2026-03-01", end="2026-03-03", categories=["cs.LG"]))
    assert [p["id"] for p in chosen] == ["in"]


def test_an_embedding_is_reused_only_for_the_same_text():
    store = Embeddings("test/model")
    first = paper("p", abstract="original")
    store.add([first], np.ones((1, 4), dtype=np.float32))
    assert store.has(first)
    assert not store.has(paper("p", abstract="revised"))  # same id, the text changed
    assert not store.has(paper("q", abstract="original"))
    assert text_hash(first) == text_hash(paper("p", abstract="original"))
    np.testing.assert_array_equal(Embeddings("test/model").get([first]), np.ones((1, 4)))


# ------------------------------------------------------------------ the similarity graph
def test_adjacency_is_clipped_symmetric_and_has_no_self_links(blobs):
    adjacency = graph.adjacency_matrix(blobs[0])
    assert adjacency.min() >= 0
    assert np.allclose(adjacency, adjacency.T, atol=1e-6)
    assert not adjacency.diagonal().any()
    assert (graph.adjacency_matrix(blobs[0], center=False) > 0).mean() >= (adjacency > 0).mean()


def test_modularity_prefers_the_real_groups(blobs):
    points, truth = blobs
    adjacency = graph.adjacency_matrix(points)
    real = [np.flatnonzero(truth == c) for c in range(3)]
    shuffled = np.random.default_rng(1).permutation(truth)
    assert graph.modularity(adjacency, real) > 0.3
    assert graph.modularity(adjacency, real) > graph.modularity(
        adjacency, [np.flatnonzero(shuffled == c) for c in range(3)]
    )


def test_fiedler_vector_separates_two_cliques():
    block = np.ones((6, 6)) - np.eye(6)
    two = np.block([[block, np.full((6, 6), 0.01)], [np.full((6, 6), 0.01), block]])
    side = graph.fiedler_side(two)
    assert side[:6].all() != side[6:].all()
    assert len(set(side[:6])) == 1 and len(set(side[6:])) == 1


def test_links_stop_at_the_gap_in_similarity():
    # papers 0, 1, 2 are close to each other and far from the rest; so are 3, 4 and 5, 6; 7 leans toward 6
    adjacency = np.full((8, 8), 0.1)
    for a, b, similarity in [(0, 1, 0.9), (0, 2, 0.9), (1, 2, 0.9), (3, 4, 0.9), (5, 6, 0.9), (6, 7, 0.5)]:
        adjacency[a, b] = adjacency[b, a] = similarity
    np.fill_diagonal(adjacency, 0)
    links = {(a, b) for a, b, _ in graph.natural_neighbours(adjacency, most=5)}
    # every paper keeps the neighbours before the largest drop in its own ranking, and nothing after it
    assert links == {(0, 1), (0, 2), (1, 2), (3, 4), (5, 6), (6, 7)}


# ------------------------------------------------------------------ clustering
@pytest.mark.parametrize("method", list(cluster.METHODS))
def test_every_method_finds_planted_groups(blobs, method):
    points, truth = blobs
    vectors = graph.unit(points - points.mean(axis=0))
    leaves, _ = cluster.communities(
        method, graph.adjacency_matrix(points), vectors, 3, min_size=5, min_modularity=0.0, log=lambda line: None
    )
    assert adjusted_rand_score(truth, cluster.labels_of(leaves, len(points))) > 0.95
    assert sorted(len(leaf.nodes) for leaf in leaves) == [40, 40, 40]
    assert [len(leaf.nodes) for leaf in leaves] == sorted((len(leaf.nodes) for leaf in leaves), reverse=True)


def test_sub_communities_partition_their_parent(blobs):
    points, _ = blobs
    vectors = graph.unit(points - points.mean(axis=0))
    leaves, _ = cluster.communities(
        "spectral",
        graph.adjacency_matrix(points),
        vectors,
        2,
        min_size=5,
        min_modularity=0.0,
        sub_levels=1,
        log=lambda line: None,
    )
    split = [leaf for leaf in leaves if leaf.children]
    assert split, "one of two communities holds two planted groups and should split"
    for leaf in split:
        assert sorted(np.concatenate([child.nodes for child in leaf.children])) == sorted(leaf.nodes)


def test_a_group_too_small_to_split_is_left_whole(blobs):
    points, _ = blobs
    vectors = graph.unit(points - points.mean(axis=0))
    leaf = cluster.Leaf("c0", np.arange(30))
    assert (
        cluster.split("spectral", graph.adjacency_matrix(points), vectors, leaf, min_size=20, min_modularity=0) is None
    )


def test_unknown_method_is_refused(blobs):
    with pytest.raises(ValueError):
        cluster.partition("magic", graph.adjacency_matrix(blobs[0]), blobs[0], 3)


# ------------------------------------------------------------------ the job state machine
def test_job_runs_to_done_and_refuses_a_second_start():
    import threading

    release = threading.Event()
    job = Job()
    assert job.state is State.IDLE
    job.start(lambda log: (log("working"), release.wait(5), "result")[-1])
    assert job.state is State.RUNNING
    with pytest.raises(Busy):
        job.start(lambda log: None)
    release.set()
    for _ in range(200):
        if job.state is not State.RUNNING:
            break
        threading.Event().wait(0.01)
    assert job.snapshot() == {"state": "done", "result": "result", "error": None, "log": ["working"], "total": 1}
    assert job.snapshot(since=1)["log"] == []


def test_job_that_raises_is_failed_and_can_start_again():
    import threading

    def boom(log):
        raise RuntimeError("no good")

    job = Job()
    job.start(boom)
    for _ in range(200):
        if job.state is not State.RUNNING:
            break
        threading.Event().wait(0.01)
    assert job.state is State.FAILED and job.error == "no good"
    job.start(lambda log: "fine")  # failed -> running is allowed
