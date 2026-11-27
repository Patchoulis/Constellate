"""The similarity graph over papers, and what is read off it directly.

adjacency[i, j] = max(0, cosine similarity of papers i and j), zero on the diagonal.

Cutting the graph into communities is in cluster.py; here are the graph itself, its modularity, the Fiedler
vector's bisection of one group, and the few links per paper worth drawing.
"""

import numpy as np
from scipy.sparse.csgraph import connected_components
from scipy.sparse.linalg import LinearOperator, eigsh

from .library import acceptance

DENSE_LIMIT = 30_000  # a dense n x n float32 matrix: 30k papers = 3.6 GB, and it is the only one made


def unit(rows: np.ndarray) -> np.ndarray:
    """Rows scaled to length 1."""
    return rows / (np.linalg.norm(rows, axis=-1, keepdims=True) + 1e-12)


def adjacency_matrix(embeddings: np.ndarray, center=True) -> np.ndarray:
    """max(0, cosine similarity).  With `center`, the mean embedding is removed first: text embeddings all share a
    large common direction, which otherwise makes every pair of papers similar and the clip at 0 do nothing."""
    if len(embeddings) > DENSE_LIMIT:
        raise ValueError(
            f"{len(embeddings)} papers is too many for a dense similarity matrix "
            f"(limit {DENSE_LIMIT}); narrow the date range or categories."
        )
    vectors = embeddings.astype(np.float32)
    if center:
        vectors = vectors - vectors.mean(axis=0, keepdims=True)
    vectors = unit(vectors)
    similarity = vectors @ vectors.T
    np.maximum(similarity, 0.0, out=similarity)
    np.fill_diagonal(similarity, 0.0)
    return similarity


def modularity(matrix: np.ndarray, groups) -> float:
    """Newman modularity of `groups` (index arrays or boolean masks) on the graph `matrix`."""
    degree = matrix.sum(axis=1, dtype=np.float64)
    total = degree.sum()
    return float(
        sum(
            matrix[np.ix_(group, group)].sum(dtype=np.float64) / total - (degree[group].sum() / total) ** 2
            for group in groups
        )
    )


def top_eigenvectors(matrix, k: int) -> tuple:
    """(the k leading eigenvectors of D^-1/2 A D^-1/2 as columns, largest eigenvalue last; D^-1/2 as a vector).

    The normalised matrix is never formed: the solver only needs its product with a vector, which is
    D^-1/2 (A (D^-1/2 v)).  So nothing the size of A is copied, which at 30,000 papers is the difference between
    one 3.6 GB matrix and four.  The start vector is fixed, so the same graph always gives the same vectors."""
    degree = np.asarray(matrix.sum(axis=1), dtype=np.float64).ravel()
    scale = 1.0 / np.sqrt(np.maximum(degree, 1e-12))
    kind = matrix.dtype

    def product(vector):
        return scale * (matrix @ (scale * np.ravel(vector)).astype(kind))

    operator = LinearOperator(matrix.shape, matvec=product, dtype=np.float64)
    start = np.random.default_rng(0).normal(size=matrix.shape[0])
    return eigsh(operator, k=k, which="LA", v0=start, tol=1e-6)[1], scale


def fiedler_side(sub: np.ndarray):
    """Boolean side of every node of one group, or None if the group cannot be split."""
    n = len(sub)
    if n < 2:
        return None
    parts, member = connected_components(sub > 0, directed=False)
    if parts > 1:
        return member == np.bincount(member).argmax()
    if n <= 400:
        scale = 1.0 / np.sqrt(sub.sum(axis=1))
        vector = np.linalg.eigh(scale[:, None] * sub * scale[None, :])[1][:, -2]
    else:
        vectors, scale = top_eigenvectors(sub, 2)
        vector = vectors[:, 0]  # ascending: [second, first]
    fiedler = vector * scale
    side = fiedler > 0
    if side.all() or not side.any():
        side = fiedler > np.median(fiedler)
    return None if side.all() or not side.any() else side


def natural_neighbours(adjacency: np.ndarray, most=8) -> list:
    """The few links worth drawing: [paper, paper, similarity], each pair once.

    A paper is linked to its most similar papers, as many as come before the largest drop in its own ranking:
    among its `most` + 1 best matches, the biggest gap between one similarity and the next is where "close" ends.
    One near-duplicate gives one link; a tight group of five gives five; nothing gets more than `most`.
    Done in blocks of rows, so memory stays at a small fraction of the matrix."""
    count = min(most + 1, len(adjacency) - 1)
    edges = {}
    for start in range(0, len(adjacency), 2048):
        block = adjacency[start : start + 2048]
        best = np.argpartition(-block, count, axis=1)[:, :count]
        weight = np.take_along_axis(block, best, axis=1)
        order = np.argsort(-weight, axis=1)
        best, weight = np.take_along_axis(best, order, axis=1), np.take_along_axis(weight, order, axis=1)
        keep = np.argmax(weight[:, :-1] - weight[:, 1:], axis=1) + 1  # those before the largest drop
        for row, (targets, weights, kept) in enumerate(zip(best, weight, keep)):
            for target, similarity in zip(targets[:kept].tolist(), weights[:kept].tolist()):
                if similarity > 0:
                    edges[(min(start + row, target), max(start + row, target))] = round(similarity, 3)
    return [[a, b, similarity] for (a, b), similarity in edges.items()]


def drawing(papers, embeddings, adjacency, communities: list, flagged, most_links=8) -> dict:
    """What the graph view needs: every paper with its community and a rough position (the two leading
    principal components of the embeddings), which decides where communities sit relative to each other; and
    each paper's few closest papers as edges.  `flagged` marks the papers the AI-text detector flagged."""
    centred = embeddings - embeddings.mean(axis=0, keepdims=True)
    sample = np.random.default_rng(0).choice(len(centred), min(len(centred), 3000), replace=False)
    axes = np.linalg.svd(centred[sample], full_matrices=False)[2]
    position = centred @ axes[:2].T
    position /= np.abs(position).max() + 1e-12
    community = np.empty(len(papers), dtype=int)
    for index, leaf in enumerate(communities):
        community[leaf.nodes] = index
    return {
        "nodes": [
            {
                "id": p["id"],
                "title": p["title"],
                "community": int(c),
                "x": round(float(x), 4),
                "y": round(float(y), 4),
                "ai": bool(ai),
                "acceptance": acceptance(p),
            }
            for p, c, (x, y), ai in zip(papers, community, position, flagged)
        ],
        "edges": natural_neighbours(adjacency, most_links),
    }
