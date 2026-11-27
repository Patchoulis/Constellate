"""The local library: every downloaded paper and every computed embedding, keyed by arXiv id.

Nothing is fetched or computed twice.  A paper already in the library is not stored again; an embedding is reused
when the same model already embedded the same paper with exactly the same text (id + hash of title and abstract),
so a paper whose abstract changed on arXiv is embedded again and one that did not is not.
"""
import hashlib
import json
import re

import numpy as np

from . import LIBRARY, store


def from_arxiv(paper: dict) -> bool:
    """Was this record downloaded from arXiv (and not from a conference's list)?  Conference records are named
    "<conference>-<year>-<number>"; their "published" is when the paper is presented, not when it was submitted."""
    return re.fullmatch(r"[a-z]+-\d{4}-\d+", paper["id"]) is None


TIERS = ("oral", "spotlight", "poster")          # best first


def acceptance(paper: dict) -> str | None:
    """How a conference paper was accepted, in three tiers that hold across conferences; None for a paper that
    did not come from a conference's list.  Conferences word their decisions differently ("Accept (oral)",
    "Accept (spotlight)", "Accept (regular)", "Highlight", "Award Candidate", plain "Accept"); anything that is
    not singled out counts as an ordinary acceptance."""
    if from_arxiv(paper):
        return None
    decision = paper.get("comment", "").lower()
    if re.search(r"oral|award|best paper", decision):
        return "oral"
    if re.search(r"spotlight|highlight", decision):
        return "spotlight"
    return "poster"


def paper_text(paper: dict) -> str:
    return f"{paper['title']}\n\n{paper['abstract']}"


def text_hash(paper: dict) -> str:
    return hashlib.sha1(paper_text(paper).encode("utf-8")).hexdigest()[:16]


class Papers:
    def __init__(self):
        LIBRARY.mkdir(parents=True, exist_ok=True)
        self.path, self.done_path = LIBRARY / "papers.jsonl", LIBRARY / "download_done.json"
        self.by_id = {}
        if self.path.exists():
            with open(self.path, encoding="utf-8") as handle:
                for line in handle:
                    if line.strip():
                        paper = json.loads(line)
                        self.by_id[paper["id"]] = paper
        self.done = set(json.loads(self.done_path.read_text())) if self.done_path.exists() else set()

    def add(self, papers, venue=None, overwrite=False):
        """Returns (new, already known).  A known paper keeps its record unless `overwrite`.
        With `venue`, `papers` is that venue's whole list: they get the tag and no other paper keeps it."""
        new = known = 0
        listed = set()
        for paper in papers:
            old = self.by_id.get(paper["id"])
            venues = set(old.get("venues", [])) if old else set()
            if venue:
                venues.add(venue)
            if old is None or overwrite:
                self.by_id[paper["id"]] = paper
            new, known = new + (old is None), known + (old is not None)
            self.by_id[paper["id"]]["venues"] = sorted(venues)
            listed.add(paper["id"])
        if venue:
            for paper in self.by_id.values():
                if paper["id"] not in listed and venue in paper.get("venues", []):
                    paper["venues"].remove(venue)
        return new, known

    def save(self, finished=None):
        def write(path):
            with open(path, "w", encoding="utf-8") as handle:
                for paper in self.by_id.values():
                    handle.write(json.dumps(paper, ensure_ascii=False) + "\n")
        store.replace(self.path, write)
        if finished:
            self.done.add(finished)
            store.replace(self.done_path, lambda path: path.write_text(json.dumps(sorted(self.done))))

    def select(self, selection) -> list:
        """The selection's papers, ordered by id."""
        if selection.venues:
            wanted = set(selection.venues)
            chosen = [p for p in self.by_id.values() if wanted & set(p.get("venues", []))]
        else:
            cats, first, last = set(selection.categories), str(selection.start), str(selection.end)
            chosen = [p for p in self.by_id.values()
                      if from_arxiv(p) and first <= p["published"][:10] <= last
                      and (not cats or cats & set(p["categories"]))]
        return sorted(chosen, key=lambda p: p["id"])


class Embeddings:
    """vectors.npy (float32, unit rows) + index.json ({paper id: [row, text hash]}) for one embedding model."""

    def __init__(self, model: str):
        self.folder = LIBRARY / "embeddings" / re.sub(r"[^\w.-]+", "_", model)
        self.folder.mkdir(parents=True, exist_ok=True)
        self.vectors_path, self.index_path = self.folder / "vectors.npy", self.folder / "index.json"
        self.index, self.vectors = {}, None
        if self.vectors_path.exists() and self.index_path.exists():
            self.index = json.loads(self.index_path.read_text())
            self.vectors = np.load(self.vectors_path)

    def has(self, paper: dict) -> bool:
        entry = self.index.get(paper["id"])
        return entry is not None and entry[1] == text_hash(paper)

    def add(self, papers, vectors: np.ndarray):
        vectors = np.asarray(vectors, dtype=np.float32)
        start = 0 if self.vectors is None else len(self.vectors)
        self.vectors = vectors if self.vectors is None else np.concatenate([self.vectors, vectors])
        for offset, paper in enumerate(papers):
            self.index[paper["id"]] = [start + offset, text_hash(paper)]     # an older row of the id is orphaned
        store.replace(self.vectors_path, lambda path: np.save(open(path, "wb"), self.vectors))
        store.replace(self.index_path, lambda path: path.write_text(json.dumps(self.index)))

    def get(self, papers) -> np.ndarray:
        return self.vectors[[self.index[p["id"]][0] for p in papers]]
