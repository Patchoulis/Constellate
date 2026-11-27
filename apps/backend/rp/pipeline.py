"""The whole pipeline, as a list of stages run in order over one shared Run record.

    python -m rp.pipeline --venues "NeurIPS 2025" "ICML 2026"
    python -m rp.pipeline --start 2026-11-01 --end 2026-11-07 --categories cs.LG stat.ML

Downloads, embeddings and descriptions already in the library are reused, except for the steps a run is asked
to redo (REDO).
One run writes data/runs/<name>/:
    run.json           what was selected            adjacency.npy     max(0, cosine similarity)
    centroids.npy      mean embedding per community communities.json  the tree of communities and sub-communities
    graph.json         the papers, for the UI       report.md         the readable report
"""

import argparse
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import RUNS, cluster, describe, detect, embed, graph, proceedings, report, store
from . import download as downloads
from . import settings as settings_store
from .library import acceptance
from .selection import Selection


@dataclass
class Run:
    """Everything one run knows.  Each stage reads what earlier stages filled in and adds its own results."""

    selection: Selection
    settings: dict
    redo: frozenset = frozenset()  # steps (of REDO) to do again despite the library
    log: Callable[[str], None] = print
    papers: list = field(default_factory=list)
    embeddings: np.ndarray | None = None
    adjacency: np.ndarray | None = None
    flagged: np.ndarray | None = None  # per paper: flagged by the AI-text detector
    communities: list = field(default_factory=list)  # cluster.Leaf, each with its sub-communities
    splits: list = field(default_factory=list)
    records: list = field(default_factory=list)

    @property
    def folder(self) -> Path:
        return RUNS / self.selection.name


def _enough(run: Run):
    """Refuse to go on with too few papers for communities to mean anything."""
    least = 4 * run.settings["min_community_size"]
    if len(run.papers) < least:
        raise ValueError(f"only {len(run.papers)} papers: at least {least} are needed to look for communities")


def download(run: Run):
    library = downloads.download(run.selection, "download" in run.redo, run.log)
    run.papers = library.select(run.selection)
    run.log(f"  {len(run.papers)} papers selected ({len(library.by_id)} in the library)")
    _enough(run)


def embed_papers(run: Run):
    s = run.settings
    run.embeddings = embed.embed_papers(run.papers, s["embedding_model"], "embeddings" in run.redo, log=run.log)


def detect_ai(run: Run):
    detector = detect.Detector(run.log)
    scores = detector.scores(run.papers)
    run.flagged = np.nan_to_num(scores, nan=-np.inf) > detector.threshold
    run.log(
        f"  {int(run.flagged.sum())} of {len(run.papers)} flagged: wording of the kind language models "
        f"produce (the threshold flags {100 * detect.FALSE_ALARMS:.0f}% of pre-LLM abstracts)"
    )


def filter_papers(run: Run):
    """Leave out what the settings say to leave out.  Each rule only judges papers it knows about: the
    acceptance rule conference papers, the declaration rule papers whose declarations have been read."""
    s = run.settings
    keep = np.ones(len(run.papers), dtype=bool)

    def leave_out(drop, why):
        dropped = int((keep & drop).sum())
        keep[drop] = False
        if dropped:
            run.log(f"  {dropped} left out: {why}")

    if not s["include_ai_papers"]:
        leave_out(run.flagged, "tagged as written the way language models write")
    label, tiers = settings_store.ACCEPTANCE[s["acceptance_filter"]]
    states = [acceptance(paper) for paper in run.papers]
    leave_out(np.array([state is not None and state not in tiers for state in states]), f"not among: {label.lower()}")
    if s["exclude_flags"]:
        read = proceedings.stored()
        raised = [set(read.get(paper["id"], {}).get("flags", [])) & set(s["exclude_flags"]) for paper in run.papers]
        leave_out(np.array([bool(flags) for flags in raised]), "their authors' declarations raise an excluded flag")
        unread = sum(paper["id"] not in read for paper in run.papers)
        if unread:
            run.log(
                f"  NOTE: {unread} of {len(run.papers)} papers' declarations have not been read, so that rule "
                f"could not judge them and they stay in (Settings: Read them all)"
            )
    if not keep.all():
        run.papers = [paper for paper, kept in zip(run.papers, keep) if kept]
        run.embeddings, run.flagged = run.embeddings[keep], run.flagged[keep]
    run.log(f"  {len(run.papers)} papers go on")
    _enough(run)


def similarity(run: Run):
    run.adjacency = graph.adjacency_matrix(run.embeddings, run.settings["center_embeddings"])
    count = len(run.papers)
    run.log(
        f"  {count} x {count}, {100 * (run.adjacency > 0).mean():.1f}% of pairs positive, "
        f"mean weight {run.adjacency.mean():.3f}"
    )


def communities(run: Run):
    s = run.settings
    vectors = graph.unit(run.embeddings - run.embeddings.mean(axis=0))
    run.communities, run.splits = cluster.communities(
        s["clustering"],
        run.adjacency,
        vectors,
        s["max_communities"],
        s["min_community_size"],
        s["min_split_modularity"],
        s["sub_levels"],
        run.log,
    )

    def count(leaves):
        return sum(1 + count(leaf.children) for leaf in leaves)

    run.log(f"  {len(run.communities)} communities, {count(run.communities) - len(run.communities)} sub-communities")


def summarise(run: Run):
    s = run.settings
    describer = describe.Describer(s["describe_model"], "descriptions" in run.redo, run.log) if s["describe"] else None
    corpus = report.Corpus(
        run.selection,
        run.papers,
        run.embeddings,
        run.adjacency,
        describe.Vocabulary(run.papers),
        describer,
        run.flagged,
    )
    run.records = report.community_tree(corpus, run.communities)


def write(run: Run):
    s = run.settings
    groups = [community.nodes for community in run.communities]
    result = {
        "name": run.selection.name,
        "papers": len(run.papers),
        "model": s["embedding_model"],
        "centered": s["center_embeddings"],
        "clustering": s["clustering"],
        "modularity": graph.modularity(run.adjacency, groups),
        "ai_flagged": int(run.flagged.sum()),
        "splits": run.splits,
        "communities": run.records,
    }
    files = {
        "run.json": {"selection": run.selection.to_json(), "settings": s, "paper_ids": [p["id"] for p in run.papers]},
        "communities.json": result,
        "graph.json": graph.drawing(
            run.papers, run.embeddings, run.adjacency, run.communities, run.flagged, s["max_links"]
        ),
    }
    run.folder.mkdir(parents=True, exist_ok=True)
    np.save(run.folder / "adjacency.npy", run.adjacency)
    np.save(run.folder / "centroids.npy", describe.centroids(run.embeddings, groups))
    for name, content in files.items():
        store.write_json(run.folder / name, content)
    urls = {p["id"]: p.get("url", "") for p in run.papers}
    store.write_text(run.folder / "report.md", report.markdown(result, urls))
    run.log(f"  modularity {result['modularity']:.3f} -> {run.folder / 'report.md'}")


REDO = ("download", "embeddings", "descriptions")

STAGES = [
    ("download", download),
    ("embed", embed_papers),
    ("detect model-written abstracts", detect_ai),
    ("filter", filter_papers),
    ("similarity -> adjacency", similarity),
    ("communities", communities),
    ("describe communities", summarise),
    ("write results", write),
]


def run(selection: Selection, redo=(), log=print) -> str:
    """Run every stage; returns the run's name.  `redo` names the steps to do again even though their results
    are in the library."""
    unknown = set(redo) - set(REDO)
    if unknown:
        raise ValueError(f"cannot redo {', '.join(sorted(unknown))}; the steps are {', '.join(REDO)}")
    state = Run(selection, settings_store.load(), frozenset(redo), log)
    for number, (title, stage) in enumerate(STAGES, start=1):
        log(f"[{number}/{len(STAGES)}] {title}")
        started = time.perf_counter()
        stage(state)
        log(f"  took {time.perf_counter() - started:.1f}s")
    return selection.name


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--venues", nargs="*", default=[], help='conferences, e.g. "NeurIPS 2025"')
    parser.add_argument(
        "--include-workshops", action="store_true", help="only for conferences taken from arXiv comments"
    )
    parser.add_argument("--start", help="first submission date, YYYY-MM-DD")
    parser.add_argument("--end", help="last submission date, YYYY-MM-DD")
    parser.add_argument("--categories", nargs="*", default=[], help="e.g. cs.LG stat.ML (default: all of arXiv)")
    parser.add_argument(
        "--redo",
        nargs="*",
        default=[],
        choices=REDO,
        help="steps to do again even though their results are in the library",
    )
    args = parser.parse_args()
    try:
        selection = Selection.parse(args.venues, args.start, args.end, args.categories, args.include_workshops)
    except ValueError as error:
        parser.error(str(error))
    run(selection, args.redo)


if __name__ == "__main__":
    main()
