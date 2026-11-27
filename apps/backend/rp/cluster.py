"""Communities: a choice of clustering methods behind one interface, and the tree of sub-communities.

Every method answers the same two questions on the same similarity graph:
    partition(...)  split all papers into k communities
    bisect(...)     split one community in two (used to grow sub-communities)

  spectral       classic spectral clustering (Ng, Jordan, Weiss): k-means on the top-k eigenvectors of the
                 normalised graph.  The default: on this kind of data it agreed with authors' own topics about as
                 well as k-means while being far more repeatable (see compare.py).
  spectral-knn   the same on a k-nearest-neighbour graph.  Slightly closer to authors' topics, less even sizes.
  kmeans         k-means on the mean-centred, unit-length embeddings.  Fast; results move most between runs.
  fiedler        recursive bisection by the sign of the Fiedler vector, largest community first.  Deterministic,
                 and a hierarchy by construction, but each cut is final, so early mistakes stay.

Sub-communities come from the same method applied again inside a community.  A split is kept only if both halves
have at least `min_size` papers and the bisection's modularity inside the group reaches `min_modularity`.
(Measured against the whole graph instead, every split below the first few is rejected: the resolution limit of
modularity.)
"""
from dataclasses import dataclass, field

import numpy as np
from scipy.sparse import csr_matrix
from sklearn.cluster import KMeans

from .graph import fiedler_side, modularity, top_eigenvectors, unit

# id -> (name as shown, a few words about it)
METHODS = {
    "spectral": ("Spectral Clustering", "most repeatable; the default"),
    "spectral-knn": ("Spectral on Nearest Neighbours", "closest to authors' topics, uneven sizes"),
    "kmeans": ("K-Means", "fast; changes most between runs"),
    "fiedler": ("Recursive Fiedler Bisection", "deterministic hierarchy; least accurate"),
}
KNN = 15


@dataclass
class Leaf:
    """A community: its place in the tree and its papers.  `children` are its sub-communities.
    `path` names it uniquely in a run: a root name ("c7", or "r" for the bisection tree), then ".0" or ".1" for
    each split below it ("c7.0.1")."""
    path: str
    nodes: np.ndarray
    children: list = field(default_factory=list)


def _kmeans(rows, k, seed):
    return KMeans(n_clusters=k, n_init=1, random_state=seed).fit_predict(rows)


def _spectral(matrix, k, seed):
    """k-means on the row-normalised top-k eigenvectors of D^-1/2 A D^-1/2 (dense or sparse A)."""
    return _kmeans(unit(top_eigenvectors(matrix, k)[0]), k, seed)


def knn_graph(adjacency, neighbours=KNN):
    """Each paper keeps the links to its `neighbours` most similar papers; a link holds if either end lists it."""
    neighbours = min(neighbours, len(adjacency) - 1)
    nearest = np.argpartition(-adjacency, neighbours, axis=1)[:, :neighbours]
    rows = np.repeat(np.arange(len(adjacency)), neighbours)
    weights = np.take_along_axis(adjacency, nearest, axis=1).ravel()
    sparse = csr_matrix((weights, (rows, nearest.ravel())), shape=adjacency.shape)
    return sparse.maximum(sparse.T)


def partition(method: str, adjacency: np.ndarray, vectors: np.ndarray, k: int, seed=0) -> np.ndarray:
    """A community label per paper, for the methods that cut into k at once (all but "fiedler").
    `vectors` are the mean-centred, unit-length embeddings the adjacency was made from."""
    if method == "spectral":
        return _spectral(adjacency, k, seed)
    if method == "spectral-knn":
        return _spectral(knn_graph(adjacency), k, seed)
    if method == "kmeans":
        return _kmeans(vectors, k, seed)
    raise ValueError(f"unknown clustering method {method!r}; the methods are {', '.join(METHODS)}")


def bisect(method: str, adjacency: np.ndarray, vectors: np.ndarray, seed=0):
    """One group cut in two: a boolean side per paper, or None when the method cannot cut it."""
    if len(adjacency) < 4:
        return None
    if method == "fiedler":
        return fiedler_side(adjacency)
    side = partition(method, adjacency, vectors, 2, seed) == 0
    return None if side.all() or not side.any() else side


def split(method, adjacency, vectors, leaf: Leaf, min_size: int, min_modularity: float):
    """(the two halves as leaves, modularity of the cut) if the leaf splits acceptably, else None."""
    if len(leaf.nodes) < 2 * min_size:
        return None
    sub = adjacency[np.ix_(leaf.nodes, leaf.nodes)]
    side = bisect(method, sub, vectors[leaf.nodes])
    if side is None or min(side.sum(), (~side).sum()) < min_size:
        return None
    quality = modularity(sub, (side, ~side))
    if quality < min_modularity:
        return None
    # ".0" / ".1", not "0" / "1": the first half of "c1" must not be named like community "c10"
    return [Leaf(leaf.path + ".0", leaf.nodes[side]), Leaf(leaf.path + ".1", leaf.nodes[~side])], quality


def _by_bisection(method, adjacency, vectors, k, min_size, min_modularity, log):
    """Communities by cutting the largest open group in two until there are k (the "fiedler" method)."""
    leaves, closed, splits = [Leaf("r", np.arange(len(adjacency)))], set(), []
    while len(leaves) < k:
        open_leaves = [leaf for leaf in leaves if leaf.path not in closed]
        if not open_leaves:
            break
        leaf = max(open_leaves, key=lambda one: len(one.nodes))
        result = split(method, adjacency, vectors, leaf, min_size, min_modularity)
        if result is None:
            closed.add(leaf.path)
            continue
        children, quality = result
        leaves.remove(leaf)
        leaves += children
        sizes = [len(child.nodes) for child in children]
        splits.append({"parent": leaf.path, "sizes": sizes, "modularity": quality})
        log(f"    split {leaf.path}: {sizes[0]} | {sizes[1]}  (modularity {quality:.3f}, {len(leaves)} leaves)")
    return leaves, splits


def subdivide(method, adjacency, vectors, leaf: Leaf, levels: int, min_size: int, min_modularity: float):
    """Fill `leaf.children` with its sub-communities, `levels` deep.  A group that does not split acceptably
    simply has none."""
    result = split(method, adjacency, vectors, leaf, min_size, min_modularity) if levels > 0 else None
    if result is not None:
        leaf.children = result[0]
        for child in leaf.children:
            subdivide(method, adjacency, vectors, child, levels - 1, min_size, min_modularity)


def communities(method, adjacency, vectors, k, min_size=15, min_modularity=0.05, sub_levels=0, log=print):
    """(communities largest first, each with its sub-communities; the record of accepted top-level splits)."""
    if method == "fiedler":
        leaves, splits = _by_bisection(method, adjacency, vectors, k, min_size, min_modularity, log)
    else:
        labels = partition(method, adjacency, vectors, k)
        leaves = [Leaf(f"c{label}", np.flatnonzero(labels == label)) for label in np.unique(labels)]
        splits = []
    leaves.sort(key=lambda leaf: -len(leaf.nodes))
    for leaf in leaves:
        subdivide(method, adjacency, vectors, leaf, sub_levels, min_size, min_modularity)
    return leaves, splits


def labels_of(leaves, count: int) -> np.ndarray:
    """A community index per paper."""
    labels = np.empty(count, dtype=int)
    for index, leaf in enumerate(leaves):
        labels[leaf.nodes] = index
    return labels
