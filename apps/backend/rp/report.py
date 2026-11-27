"""Turn the tree of communities into what is stored and shown: one record per (sub-)community, and the report."""

from collections import Counter
from dataclasses import dataclass

import numpy as np

from . import describe
from .graph import unit
from .library import acceptance
from .selection import Selection


@dataclass
class Corpus:
    """The papers of a run and everything computed about them that describing a group of them needs."""

    selection: Selection
    papers: list
    embeddings: np.ndarray
    adjacency: np.ndarray
    vocabulary: describe.Vocabulary
    describer: describe.Describer | None  # None: descriptions are switched off
    flagged: np.ndarray  # per paper: flagged by the AI-text detector


def community_tree(corpus: Corpus, leaves: list, parent=None) -> list:
    """One record per leaf, with the records of its sub-communities under "children".

    A record is the group's mean embedding turned into words (characteristic terms, the papers nearest to it and,
    when enabled, a written description) plus the similarity figures.  Everything relative is relative to the
    siblings: the terms of a sub-community are what sets it apart inside its `parent`, and
    "closest_communities" compares a group with the others in `leaves`."""
    embeddings = corpus.embeddings
    groups = [leaf.nodes for leaf in leaves]
    centers = describe.centroids(embeddings, groups)
    terms = [corpus.vocabulary.keywords(group, within=parent and parent.nodes) for group in groups]
    texts = [None] * len(groups)
    if corpus.describer:
        texts = corpus.describer.describe(corpus.papers, embeddings, groups, centers, terms)
    # How alike two groups are: cosine similarity of their mean embeddings, after removing the overall mean
    # embedding (the raw mean embeddings of any two groups of ML papers are all close to 1).
    relative = unit(centers - embeddings.mean(axis=0))
    between = relative @ relative.T
    records = []
    for c, (leaf, group) in enumerate(zip(leaves, groups)):
        inside = set(group.tolist())
        order, scores = describe.nearest_papers(embeddings, centers[c], 10)
        text = texts[c] or {}
        closest = [int(other) for other in np.argsort(-between[c]) if other != c][:4]
        records.append(
            {
                "id": c,
                "tree_path": leaf.path,
                "size": len(group),
                "name": text.get("name"),
                "description": text.get("description"),
                "keywords": terms[c],
                "mix": dict(
                    Counter(tag for i in group for tag in corpus.selection.tags(corpus.papers[i])).most_common(6)
                ),
                # mean adjacency weight between two members
                "mean_inner_similarity": float(corpus.adjacency[np.ix_(group, group)].mean()),
                # mean cosine similarity of the members to the group's mean embedding
                "similarity_to_mean": float((embeddings[group] @ unit(centers[c])).mean()),
                "closest_communities": [{"id": other, "cosine": float(between[c, other])} for other in closest],
                "nearest_to_centroid": [
                    {
                        "id": corpus.papers[i]["id"],
                        "title": corpus.papers[i]["title"],
                        "cosine": float(score),
                        "in_community": int(i) in inside,
                    }
                    for i, score in zip(order, scores)
                ],
                "ai_flagged": int(corpus.flagged[group].sum()),
                # how many of its papers were accepted as orals / spotlights / ordinary papers
                "acceptance": dict(Counter(state for state in (acceptance(corpus.papers[i]) for i in group) if state)),
                "members": group.tolist(),
                "children": community_tree(corpus, leaf.children, leaf) if leaf.children else [],
            }
        )
    return records


def _title(c: dict) -> str:
    return c["name"] or ", ".join(c["keywords"][:4])


def _outline(children: list, depth=0) -> list:
    """Sub-communities as a nested bullet list."""
    return [
        line
        for c in children
        for line in [f"{'  ' * depth}- {_title(c)} ({c['size']})", *_outline(c["children"], depth + 1)]
    ]


def _section(c: dict) -> list:
    closest = ", ".join(f"#{other['id']} ({other['cosine']:.2f})" for other in c["closest_communities"])
    return [
        f"## {c['id']}. {_title(c)}  ({c['size']} papers)",
        "",
        *([c["description"], ""] if c["description"] else []),
        f"*Terms:* {', '.join(c['keywords'])}  ",
        f"*Mix:* {', '.join(f'{tag} {count}' for tag, count in c['mix'].items())}  ",
        f"*Similarity:* members to the mean embedding {c['similarity_to_mean']:.3f}; mean pairwise weight inside "
        f"{c['mean_inner_similarity']:.3f}; closest communities {closest}",
        "",
        "Nearest to the mean embedding:",
        "",
        *(f"- [{p['title']}]({p['url']}) ({p['cosine']:.3f})" for p in c["nearest_to_centroid"][:6]),
        "",
        *(["Sub-communities:", "", *_outline(c["children"]), ""] if c["children"] else []),
    ]


def markdown(result: dict, urls: dict) -> str:
    """`urls` = {paper id: link}."""
    for c in result["communities"]:
        for paper in c["nearest_to_centroid"]:
            paper.setdefault("url", urls.get(paper["id"], ""))
    head = (
        f"{result['papers']} papers, embedded with `{result['model']}`; adjacency = max(0, cosine similarity)"
        f"{' of mean-centred embeddings' if result['centered'] else ''}; clustering method "
        f"`{result['clustering']}` gave {len(result['communities'])} communities (modularity "
        f"{result['modularity']:.3f}). {result['ai_flagged']} papers are flagged by the AI-text detector."
    )
    return "\n".join(
        [
            f"# Communities: {result['name']}",
            "",
            head,
            "",
            *(line for c in result["communities"] for line in _section(c)),
        ]
    )
