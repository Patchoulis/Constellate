"""Describe each community from its mean embedding.

The mean embedding (centroid) of a community is a vector, not text.  To turn it back into words it is "decoded"
by retrieval: the papers of the whole selection whose embeddings lie closest to the centroid are looked up, and
Claude is asked what single research topic that point in embedding space stands for.  Without API credentials only
the characteristic terms and the nearest papers are reported.
"""

import hashlib
from concurrent.futures import ThreadPoolExecutor

import numpy as np
from sklearn.feature_extraction.text import CountVectorizer

from . import LIBRARY, claude, store
from .graph import unit

NEAREST = 25
PARALLEL = 8  # description requests in flight at once

SYSTEM = """You name research communities found by clustering paper embeddings.
You are given the papers whose embeddings lie nearest to the mean embedding of one community, nearest first, and \
the terms most characteristic of the community. Reconstruct what that mean embedding represents: the shared topic, \
not a list of the individual papers.
Reply in exactly this form and nothing else:
NAME: <a specific topic name, at most 8 words>
DESCRIPTION: <2-4 sentences: the problem these papers work on, the methods they share, and what sets this \
community apart from neighbouring areas of machine learning>"""


def centroids(embeddings: np.ndarray, groups) -> np.ndarray:
    """The mean embedding of every group (a group = the paper indices of one community)."""
    return np.stack([embeddings[group].mean(axis=0) for group in groups])


def nearest_papers(embeddings: np.ndarray, centroid: np.ndarray, count=NEAREST):
    """Indices (and cosine similarity) of the papers closest to a centroid, over the whole selection."""
    scores = embeddings @ unit(centroid)
    order = np.argsort(-scores)[:count]
    return order, scores[order]


class Vocabulary:
    """Word counts of every paper, made once, to find the terms that set any group of papers apart."""

    def __init__(self, papers):
        counter = CountVectorizer(
            stop_words="english",
            ngram_range=(1, 2),
            min_df=2,
            max_features=200_000,
            token_pattern=r"(?u)\b[a-zA-Z][a-zA-Z\-]{2,}\b",
        )
        self.per_paper = counter.fit_transform(f"{p['title']} {p['abstract']}" for p in papers).tocsr()
        self.terms = np.array(counter.get_feature_names_out())
        self.overall = self._share(self.per_paper)  # how often a term is used anywhere

    @staticmethod
    def _share(rows) -> np.ndarray:
        counts = np.asarray(rows.sum(axis=0), dtype=np.float64).ravel()
        return counts / max(counts.sum(), 1.0)

    def keywords(self, group, within=None, top=12) -> list:
        """The terms that set `group` apart from the papers of `within` (default: from all papers): each term's
        contribution to the divergence between the two word distributions, i.e. frequent here AND rarer there."""
        inside = self._share(self.per_paper[group])
        around = self.overall if within is None else self._share(self.per_paper[within])
        weights = inside * np.log((inside + 1e-12) / (around + 1e-12))
        return self.terms[np.argsort(-weights)[:top]].tolist()


def _parse(text: str) -> dict:
    if "DESCRIPTION:" not in text:
        raise claude.NoAnswer("the answer was not in the NAME / DESCRIPTION form")
    name, description = text.split("DESCRIPTION:", 1)
    return {"name": name.replace("NAME:", "").strip(), "description": description.strip()}


def _prompt(papers, embeddings, size, center, terms) -> str:
    order, scores = nearest_papers(embeddings, center)
    listing = "\n\n".join(
        f"[{rank + 1}] (cosine {score:.3f}) {papers[i]['title']}\n{papers[i]['abstract']}"
        for rank, (i, score) in enumerate(zip(order, scores))
    )
    return (
        f"Community of {size} papers.\nCharacteristic terms: {', '.join(terms)}\n\n"
        f"Papers nearest to the community's mean embedding:\n\n{listing}"
    )


class Describer:
    """Writes community descriptions with Claude, for one run.

    Answers are remembered in the library under the model and the exact set of member papers, so an unchanged
    group is never sent to the API twice (unless `redo`).  The API itself is in one of three states:

        untried --first group that needs it--> available | unavailable

    and once it is unavailable (no credentials, unknown model) nothing more is asked for the rest of the run.
    """

    def __init__(self, model: str, redo=False, log=print):
        self.model, self.redo, self.log = model, redo, log
        self.api, self.client = "untried", None
        self.path = LIBRARY / "descriptions.json"
        self.stored = store.read_json(self.path, {})

    def _key(self, ids) -> str:
        return hashlib.sha1((self.model + "|" + ",".join(sorted(ids))).encode()).hexdigest()

    def _connect(self) -> bool:
        if self.api == "untried":
            try:
                self.client = claude.connect(self.model)
                self.api = "available"
            except claude.Unavailable as error:
                self.api = "unavailable"
                self.log(f"  Claude API not available ({error})")
                self.log("  -> set ANTHROPIC_API_KEY and run again; everything else is reused.")
        return self.api == "available"

    def _write(self, key, papers, embeddings, group, center, terms):
        """Ask for one description and remember it; None when no answer came."""
        try:
            prompt = _prompt(papers, embeddings, len(group), center, terms)
            text = _parse(
                claude.ask(self.client, self.model, SYSTEM, prompt, max_tokens=4000, purpose="naming communities")
            )
        except claude.NoAnswer as error:
            self.log(f"  no description for a group of {len(group)}: {error}")
            return None
        self.stored[key] = text
        store.update_json(self.path, lambda stored: stored.__setitem__(key, text), indent=1)
        self.log(f"  described: {text['name']}")
        return text

    def describe(self, papers, embeddings, groups, centers, terms) -> list:
        """One {"name", "description"} (or None) per group.  Groups not described before are asked for together,
        a few requests at a time."""
        keys = [self._key(papers[i]["id"] for i in group) for group in groups]
        out = [None if self.redo else self.stored.get(key) for key in keys]
        missing = [index for index, text in enumerate(out) if text is None]
        if missing and self._connect():
            with ThreadPoolExecutor(max_workers=PARALLEL) as pool:
                written = pool.map(
                    lambda i: self._write(keys[i], papers, embeddings, groups[i], centers[i], terms[i]), missing
                )
                for index, text in zip(missing, written):
                    out[index] = text
        return out
