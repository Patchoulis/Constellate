"""What can be picked from a menu: conferences and arXiv categories grouped by field, models, methods.

Conferences come in two kinds, and the difference matters when comparing them:

  official   the conference's own site publishes every accepted paper (see conferences.py).  Complete.
  arxiv      no such list; papers are found by the conference's name in arXiv comments ("Accepted at ACL 2025").
             Only the papers that are on arXiv and say so, so a partial and self-selected sample.

Fields outside computer science publish in journals, not conference proceedings; for those the arXiv categories
(used with a date range) are the real source.  Chemistry has no conference here for that reason.
"""

from . import claude, cluster, proceedings, settings

# field -> [(conference name, host of its official list or None)]
CONFERENCES = {
    "Machine learning": [
        ("NeurIPS", "neurips.cc"), ("ICML", "icml.cc"), ("ICLR", "iclr.cc"),
        ("AISTATS", "virtual.aistats.org"), ("MLSys", "mlsys.org"),
        ("AAAI", None), ("IJCAI", None), ("UAI", None), ("COLT", None), ("KDD", None), ("SIGIR", None),
    ],
    "Computer vision": [
        ("CVPR", "cvpr.thecvf.com"), ("ICCV", "iccv.thecvf.com"), ("ECCV", "eccv.ecva.net"),
        ("WACV", "wacv.thecvf.com"), ("BMVC", None),
    ],
    "Language and speech": [
        ("ACL", None), ("EMNLP", None), ("NAACL", None), ("COLING", None), ("Interspeech", None), ("ICASSP", None),
    ],
    "Robotics": [("ICRA", None), ("IROS", None), ("CoRL", None), ("RSS", None)],
    "Medicine": [("MICCAI", None), ("MIDL", None)],
    "Physics": [("ICRC", None), ("Quark Matter", None), ("Lattice", None), ("ICHEP", None)],
}

# field -> [(arXiv category, what it is)]
CATEGORIES = {
    "Machine learning": [("cs.LG", "Machine learning"), ("stat.ML", "Statistical ML"), ("cs.AI", "Artificial intelligence"),
                         ("cs.NE", "Neural and evolutionary computing")],
    "Vision, language, robotics": [("cs.CV", "Computer vision"), ("cs.CL", "Computation and language"),
                                   ("cs.RO", "Robotics"), ("eess.AS", "Audio and speech"), ("cs.IR", "Information retrieval")],
    "Chemistry and materials": [("physics.chem-ph", "Chemical physics"), ("cond-mat.mtrl-sci", "Materials science"),
                                ("cond-mat.soft", "Soft condensed matter"), ("q-bio.BM", "Biomolecules")],
    "Physics": [("quant-ph", "Quantum physics"), ("hep-th", "High energy theory"), ("hep-ph", "High energy phenomenology"),
                ("cond-mat.str-el", "Strongly correlated electrons"), ("cond-mat.stat-mech", "Statistical mechanics"),
                ("gr-qc", "General relativity"), ("astro-ph.CO", "Cosmology"), ("physics.comp-ph", "Computational physics")],
    "Biology and medicine": [("q-bio.QM", "Quantitative methods"), ("q-bio.GN", "Genomics"), ("q-bio.NC", "Neurons and cognition"),
                             ("eess.IV", "Image and video processing"), ("physics.med-ph", "Medical physics")],
    "Mathematics and statistics": [("math.OC", "Optimization and control"), ("math.ST", "Statistics theory"),
                                   ("stat.ME", "Methodology"), ("math.PR", "Probability"), ("cs.IT", "Information theory")],
}

_HOSTS = {name.lower(): host for venues in CONFERENCES.values() for name, host in venues if host}


def official_host(conference: str) -> str | None:
    """The site that publishes this conference's accepted papers, if there is one."""
    return _HOSTS.get(conference.lower())


# Embedding models that were loaded and tried on this machine: (Hugging Face name, what to say about it).
EMBEDDING_MODELS = [
    ("Qwen/Qwen3-Embedding-0.6B", "1024 dimensions; the default"),
    ("BAAI/bge-large-en-v1.5", "1024 dimensions, general purpose"),
    ("BAAI/bge-base-en-v1.5", "768 dimensions, general purpose, fast"),
    ("allenai/specter2_base", "768 dimensions, trained on scientific papers"),
    ("sentence-transformers/all-mpnet-base-v2", "768 dimensions, a long-standing baseline"),
]


def as_json() -> dict:
    # "name" is what gets stored; "label" is what a menu shows when that is not the same thing
    named = lambda table: [{"name": name, "label": label, "note": note} for name, (label, note) in table.items()]   # noqa: E731
    return {
        "embedding_models": [{"name": name, "note": note} for name, note in EMBEDDING_MODELS],
        "describe_models": named(claude.MODELS),
        "clustering": named(cluster.METHODS),
        "acceptance": [{"name": name, "label": label, "note": ""} for name, (label, _) in settings.ACCEPTANCE.items()],
        "flags": [{"field": "Leave out papers where", "items": [{"name": flag, "label": meaning, "note": proceedings.FLAG_NOTES[flag]}
                                                              for flag, meaning in proceedings.FLAGS.items()]}],
        "conferences": [{"field": field, "items": [{"name": name, "note": "official list" if host else
                                                    "from arXiv comments: partial"} for name, host in venues]}
                        for field, venues in CONFERENCES.items()],
        "categories": [{"field": field, "items": [{"name": name, "note": what} for name, what in categories]}
                       for field, categories in CATEGORIES.items()],
    }
