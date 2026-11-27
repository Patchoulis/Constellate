"""An overview of one community: what its papers have in common, with every claim tied to the papers behind it.

Claude is given all of the community's papers (title and abstract, numbered) and what the graph says about them:
its sub-communities, the papers at its centre, the papers most linked to, and where its links to other communities
go.  It answers in a fixed set of labelled sections, and each point cites papers by number.

Repetition is the test for a point being worth stating: one paper saying something is that paper's claim, several
papers saying it is a pattern of the community.  So a point must cite at least MIN_CITED papers, citations are
checked against the community's own list, and a point left with too few real citations is dropped rather than
shown.

An overview is kept in the library under the model and the exact set of papers, so it is written once.
"""
import hashlib
import json
from collections import Counter

from . import LIBRARY, RUNS, claude, store
from .library import Papers

MIN_CITED = 3

SECTIONS = {
    "common_methods": "Common methods",
    "recurring_findings": "Recurring findings",
    "promising_directions": "Promising directions",
    "structure_patterns": "Patterns in the structure",
    "open_questions": "Open questions and disagreements",
}

SYSTEM = f"""You are writing an overview of one research community for a researcher who wants to know what is \
going on in it without reading several hundred abstracts.

You are given every paper of the community, numbered, and facts about its structure from a similarity graph.

What counts is repetition. A method, result or idea is worth a point when several papers share it; something \
only one paper says is that paper's claim, not a pattern. Every point must cite at least {MIN_CITED} papers that \
really do share it, by their numbers, and should cite more when more share it (up to about 12, the clearest \
first). Do not pad a citation list with papers that are merely on the same topic. If fewer than {MIN_CITED} \
papers support something, leave it out.

Sections:
- common_methods: techniques, architectures, training recipes, benchmarks or analysis tools that recur.
- recurring_findings: empirical or theoretical results that several papers report or agree on.
- promising_directions: ideas that several recent papers converge on and that look like they are gaining \
momentum; say what makes each promising, from what the papers report.
- structure_patterns: what the graph facts show: how the sub-communities differ, what the central and most-linked \
papers have in common, which neighbouring communities this one borders and through which kind of paper.
- open_questions: places where papers disagree, report conflicting results, or name the same unsolved problem.

Each point has a title, which is a specific claim in at most 12 words (not a topic label: "Reward shaping from \
verifier feedback replaces human labels", not "Reward design"), and an insight of 1-3 sentences that states \
what the cited papers share, concretely. Order the points of a section by how many papers support them, most \
first. 3 to 7 points per section; fewer if the evidence is thin. Also give a title for the whole overview (at \
most 10 words) and a summary of 2-4 sentences. Write plainly; state only what the abstracts support."""

_POINT = {
    "type": "object",
    "properties": {
        "title": {"type": "string"},
        "insight": {"type": "string"},
        "papers": {"type": "array", "items": {"type": "integer"}},
    },
    "required": ["title", "insight", "papers"],
    "additionalProperties": False,
}
SCHEMA = {
    "type": "object",
    "properties": {"title": {"type": "string"}, "summary": {"type": "string"},
                   **{key: {"type": "array", "items": _POINT} for key in SECTIONS}},
    "required": ["title", "summary", *SECTIONS],
    "additionalProperties": False,
}


def _find(communities: list, path: str) -> dict:
    """The community with exactly this path, anywhere in the tree."""
    for community in communities:
        if community["tree_path"] == path:
            return community
        try:
            return _find(community["children"], path)
        except LookupError:
            pass
    raise LookupError(path)


def _paths(communities: list):
    for community in communities:
        yield community["tree_path"]
        yield from _paths(community["children"])


def _label(community: dict) -> str:
    return community["name"] or ", ".join(community["keywords"][:3])


def _prompt(community: dict, roots: list, graph: dict, papers: list) -> str:
    """`papers` = the run's papers in graph order.  Members are numbered from 1 in the order listed."""
    members = community["members"]
    number = {index: n for n, index in enumerate(members, start=1)}
    inside = set(members)
    root_of = {index: root for root in roots for index in root["members"]}
    degree, bridges = Counter(), Counter()
    for a, b, _ in graph["edges"]:
        for here, there in ((a, b), (b, a)):
            if here in inside:
                degree[here] += 1
                if there not in inside:
                    bridges[_label(root_of[there])] += 1
    listed = lambda indices: ", ".join(f"[{number[i]}]" for i in indices)                    # noqa: E731
    by_id = {papers[index]["id"]: index for index in members}
    central = [by_id[p["id"]] for p in community["nearest_to_centroid"] if p["id"] in by_id]
    lines = [
        f"Community: {_label(community)} ({len(members)} papers)",
        f"Its characteristic terms: {', '.join(community['keywords'])}", "",
        "STRUCTURE (from the similarity graph)",
        f"Papers closest to the community's mean embedding, i.e. its centre: {listed(central)}",
        f"Papers with the most links to other papers of the community: {listed(i for i, _ in degree.most_common(12))}",
        "Its links to other communities go to: "
        + ("; ".join(f"{name} ({count} links)" for name, count in bridges.most_common(6)) or "none"),
    ]
    for child in community["children"]:
        lines.append(f"Sub-community \"{_label(child)}\" ({child['size']} papers; terms: "
                     f"{', '.join(child['keywords'][:8])}): {listed(child['members'])}")
    lines += ["", "PAPERS"]
    lines += [f"[{number[index]}] {papers[index]['title']}\n{papers[index]['abstract']}\n" for index in members]
    return "\n".join(lines)


def _checked(answer: dict, members: list, papers: list) -> dict:
    """The answer with paper numbers turned into papers, unknown numbers removed, and thin points dropped."""
    sections = []
    for key, heading in SECTIONS.items():
        points = []
        for point in answer[key]:
            cited = [members[n - 1] for n in dict.fromkeys(point["papers"]) if 1 <= n <= len(members)]
            if len(cited) >= MIN_CITED:
                points.append({"title": point["title"], "insight": point["insight"],
                               "papers": [{"id": papers[i]["id"], "title": papers[i]["title"]} for i in cited]})
        if points:
            sections.append({"heading": heading, "points": sorted(points, key=lambda p: -len(p["papers"]))})
    return {"title": answer["title"], "summary": answer["summary"], "sections": sections}


class Overviews:
    """Overviews already written, and the writing of new ones."""

    def __init__(self):
        self.path = LIBRARY / "overviews.json"

    def _stored(self) -> dict:
        return store.read_json(self.path, {})

    def _load(self, run: str, path: str, model: str):
        """The community at `path` of a run (LookupError if there is none), with what writing about it needs."""
        folder = RUNS / run
        roots = json.loads((folder / "communities.json").read_text(encoding="utf-8"))["communities"]
        paths = list(_paths(roots))
        if len(paths) != len(set(paths)):
            raise ValueError("This run was made by an older version, in which two communities could share a name. "
                             "Run it again (everything is reused) and ask for the overview then.")
        community = _find(roots, path)
        library = Papers().by_id
        papers = [library[paper_id] for paper_id in
                  json.loads((folder / "run.json").read_text(encoding="utf-8"))["paper_ids"]]
        ids = sorted(papers[index]["id"] for index in community["members"])
        key = hashlib.sha1((model + "|" + ",".join(ids)).encode()).hexdigest()
        return folder, roots, community, papers, key

    def get(self, run: str, path: str, model: str) -> dict | None:
        """The overview `model` wrote of a community, if it has."""
        return self._stored().get(self._load(run, path, model)[-1])

    def write(self, run: str, path: str, model: str, log=print) -> dict:
        """Write (or fetch) the overview of the community at `path`.  Raises claude.Unavailable / NoAnswer."""
        folder, roots, community, papers, key = self._load(run, path, model)
        stored = self._stored()
        if key in stored:
            return stored[key]
        graph = json.loads((folder / "graph.json").read_text(encoding="utf-8"))
        log(f"reading {len(community['members'])} papers with {model} ...")
        answer = claude.ask_json(claude.connect(model), model, SYSTEM, _prompt(community, roots, graph, papers),
                                 SCHEMA, effort="medium", max_tokens=32000,
                                 purpose="community overviews")
        overview = _checked(answer, community["members"], papers) | {"model": model, "papers": len(community["members"])}
        store.update_json(self.path, lambda stored: stored.__setitem__(key, overview), indent=1)
        return overview
