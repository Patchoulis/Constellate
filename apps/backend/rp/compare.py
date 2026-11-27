"""Which clustering method gives better communities on a run's papers?

    python -m rp.compare neurips2025_icml2026
    python -m rp.compare neurips2025_icml2026 --k 12 24 48 --model BAAI/bge-base-en-v1.5

Every method (cluster.METHODS) clusters the same papers into the same number of communities, and is scored on
what a reader of the result cares about.  No single score is trusted on its own; each covers another's blind spot:

  agreement with the authors   Each conference paper carries the topic its authors filed it under ("Deep Learning->
    (AMI fine / coarse)        Large Language Models"): an outside reference nobody tuned against.  Adjusted mutual
                               information, corrected for chance, against the fine topic and the coarse one.
                               Conferences use different topic lists, so each is scored on its own papers and the
                               scores are averaged by paper count.  Its weakness: one coarse topic per paper.
  coherence of the wording     For each community, do its ten most characteristic terms actually occur together
    (NPMI)                     in abstracts?  Mean normalised pointwise mutual information over term pairs.  It
                               uses only the text, so it favours neither k-means (which optimises distance in
                               embedding space) nor graph methods (which optimise cuts of the graph), and it
                               needs no reference labels.
  internal quality             Silhouette (cosine) and modularity of the similarity graph.  Kept for reference:
                               each favours the family that optimises something like it.
  stability                    Seeds: clustered again from another random start.  Subsamples: 80% of the papers
                               clustered afresh; agreement (ARI) on the papers two subsamples share.  A method
                               whose communities change when a fifth of the papers is removed describes noise.
  shape                        Smallest and largest community.

The number of communities is swept (--k), because a ranking that holds at one size only is not a ranking, and the
embedding model can be changed (--model), because one model's geometry may suit one method.  Run it on another
run for another dataset.
"""

import argparse
import json
from itertools import combinations

import numpy as np
from sklearn.metrics import adjusted_mutual_info_score, adjusted_rand_score, silhouette_score

from . import RUNS, cluster, describe, embed, graph, store
from .library import Papers

SEEDS = 5
SUBSAMPLES = 3
TOP_TERMS = 10


def _labelings(embeddings, k, settings, seeds=(0,)) -> tuple:
    """({method: one labelling per seed}, adjacency, vectors).  A method with no randomness gives one labelling.
    The graph is rebuilt from the embeddings given, as the pipeline would."""
    vectors = graph.unit(embeddings - embeddings.mean(axis=0))
    adjacency = graph.adjacency_matrix(embeddings, settings["center_embeddings"])
    limits = (settings["min_community_size"], settings["min_split_modularity"])

    def labels(method, seed):
        if method == "fiedler":
            leaves, _ = cluster.communities(method, adjacency, vectors, k, *limits, log=lambda line: None)
            return cluster.labels_of(leaves, len(embeddings))
        return cluster.partition(method, adjacency, vectors, k, seed)

    return (
        {
            method: [labels(method, seed) for seed in (seeds if method != "fiedler" else seeds[:1])]
            for method in cluster.METHODS
        },
        adjacency,
        vectors,
    )


def _against_topics(labels, papers, level) -> float:
    """AMI against the authors' topics, per conference, averaged by paper count."""
    scores, weights = [], []
    for venue in sorted({v for p in papers for v in p.get("venues", [])}):
        rows = [i for i, p in enumerate(papers) if venue in p.get("venues", []) and p["primary_category"]]
        if len(rows) < 100:
            continue
        topics = [
            papers[i]["primary_category"] if level == "fine" else papers[i]["primary_category"].split("->")[0]
            for i in rows
        ]
        scores.append(adjusted_mutual_info_score(topics, labels[rows]))
        weights.append(len(rows))
    return float(np.average(scores, weights=weights)) if scores else float("nan")


def _coherence(labels, vocabulary: describe.Vocabulary) -> float:
    """Mean NPMI between the top terms of each community, from how often terms share an abstract."""
    present = (vocabulary.per_paper > 0).astype(np.float64)
    documents = present.shape[0]
    index = {term: column for column, term in enumerate(vocabulary.terms)}
    scores = []
    for community in np.unique(labels):
        columns = [index[term] for term in vocabulary.keywords(np.flatnonzero(labels == community), top=TOP_TERMS)]
        occurs = present[:, columns]
        single = np.asarray(occurs.sum(axis=0)).ravel() / documents
        joint = (occurs.T @ occurs).toarray() / documents
        for a, b in combinations(range(len(columns)), 2):
            if joint[a, b] > 0:
                scores.append(np.log(joint[a, b] / (single[a] * single[b])) / -np.log(joint[a, b]))
            else:
                scores.append(-1.0)  # the two terms never share an abstract
    return float(np.mean(scores))


def _mean_pairwise_ari(labelings, shared=None) -> float:
    """Mean ARI over all pairs.  `shared[i]` = the papers of labelling i, when they were made on subsamples."""
    scores = []
    for a, b in combinations(range(len(labelings)), 2):
        if shared is None:
            scores.append(adjusted_rand_score(labelings[a], labelings[b]))
            continue
        _, in_a, in_b = np.intersect1d(shared[a], shared[b], return_indices=True)
        scores.append(adjusted_rand_score(labelings[a][in_a], labelings[b][in_b]))
    return float(np.mean(scores)) if scores else 1.0


def _rows(papers, embeddings, vocabulary, k, settings, log) -> list:
    """One row of scores per method, at k communities."""
    log(f"k = {k}: {SEEDS} random starts ...")
    runs, adjacency, vectors = _labelings(embeddings, k, settings, tuple(range(SEEDS)))
    log(f"k = {k}: {SUBSAMPLES} subsamples of 80% ...")
    rng = np.random.default_rng(0)
    kept = [np.sort(rng.choice(len(papers), int(0.8 * len(papers)), replace=False)) for _ in range(SUBSAMPLES)]
    subsampled = [_labelings(embeddings[rows], k, settings)[0] for rows in kept]
    sample = np.random.default_rng(1).choice(len(papers), min(4000, len(papers)), replace=False)

    def score(labels) -> dict:
        groups = [np.flatnonzero(labels == c) for c in np.unique(labels)]
        return {
            "AMI fine": _against_topics(labels, papers, "fine"),
            "AMI coarse": _against_topics(labels, papers, "coarse"),
            "coherence": _coherence(labels, vocabulary),
            "silhouette": silhouette_score(vectors[sample], labels[sample], metric="cosine"),
            "modularity": graph.modularity(adjacency, groups),
            "smallest": float(min(map(len, groups))),
            "largest": float(max(map(len, groups))),
        }

    rows = []
    for method, labelings in runs.items():
        scores = [score(labels) for labels in labelings]
        mean = {name: float(np.mean([one[name] for one in scores])) for name in scores[0]}
        rows.append(
            {
                "k": k,
                "method": method,
                "AMI fine": mean["AMI fine"],
                # how far one random start lands from another on that score: a smaller difference between two
                # methods means nothing
                "spread": float(np.std([one["AMI fine"] for one in scores])),
                "AMI coarse": mean["AMI coarse"],
                "coherence": mean["coherence"],
                "silhouette": mean["silhouette"],
                "modularity": mean["modularity"],
                "seed stability": _mean_pairwise_ari(labelings),
                "subsample stability": _mean_pairwise_ari([sub[method][0] for sub in subsampled], kept),
                "smallest": round(mean["smallest"]),
                "largest": round(mean["largest"]),
            }
        )
    return rows


def _ranking(rows: list) -> list:
    """Mean rank of each method over every k, on the scores that are fair to all methods."""
    fair = ("AMI fine", "AMI coarse", "coherence", "subsample stability")
    ranks = {method: [] for method in cluster.METHODS}
    for k in sorted({row["k"] for row in rows}):
        at_k = [row for row in rows if row["k"] == k]
        for name in fair:
            if any(np.isnan(row[name]) for row in at_k):  # e.g. no authors' topics: the score ranks nobody
                continue
            ordered = sorted(at_k, key=lambda row: -row[name])
            for place, row in enumerate(ordered, start=1):
                ranks[row["method"]].append(place)
    return sorted(((float(np.mean(places)), method) for method, places in ranks.items()))


def compare(name: str, ks=(), model=None, log=print) -> str:
    folder = RUNS / name
    run = json.loads((folder / "run.json").read_text(encoding="utf-8"))
    settings = run["settings"]
    model = model or settings["embedding_model"]
    ks = list(ks) or [len(json.loads((folder / "communities.json").read_text(encoding="utf-8"))["communities"])]
    library = Papers().by_id
    papers = [library[paper_id] for paper_id in run["paper_ids"]]
    embeddings = embed.embed_papers(papers, model, log=log)
    vocabulary = describe.Vocabulary(papers)
    log(f"{len(papers)} papers, {sum(bool(p['primary_category']) for p in papers)} with a topic, embeddings: {model}")
    rows = [row for k in ks for row in _rows(papers, embeddings, vocabulary, k, settings, log)]

    columns = list(rows[0])
    cell = lambda value: f"{value:.3f}" if isinstance(value, float) else str(value)  # noqa: E731
    text = "\n".join(
        [
            f"# Clustering comparison: {name}",
            "",
            f'{len(papers)} papers, embeddings from `{model}`. Scores are means over {SEEDS} random starts; "spread" '
            f"is the standard deviation of AMI fine between starts. Stability is the mean adjusted Rand index between "
            f"those starts, and between {SUBSAMPLES} subsamples of 80% of the papers (on the papers two subsamples "
            "share). What each score means and what it cannot tell is in rp/compare.py.",
            "",
            "| " + " | ".join(columns) + " |",
            "|" + "---|" * len(columns),
            *("| " + " | ".join(cell(row[c]) for c in columns) + " |" for row in rows),
            "",
            "Mean rank over every k on the scores that are fair to all methods (AMI fine, AMI coarse, coherence, "
            "subsample stability); 1 = best:",
            "",
            *(f"- {method}: {rank:.2f}" for rank, method in _ranking(rows)),
            "",
        ]
    )
    suffix = "" if model == settings["embedding_model"] else "_" + model.split("/")[-1]
    store.write_text(folder / f"clustering_comparison{suffix}.md", text)
    log(text)
    return text


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("run")
    parser.add_argument("--k", nargs="*", type=int, default=[], help="numbers of communities (default: the run's)")
    parser.add_argument("--model", help="embedding model (default: the run's); papers are embedded if needed")
    args = parser.parse_args()
    compare(args.run, args.k, args.model)
