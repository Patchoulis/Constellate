"""Research Partitioner backend: arXiv download, embeddings, similarity graph, Fiedler communities.

Everything on disk lives under <repo>/data (override with the RP_DATA environment variable):

    data/settings.json                         user settings
    data/library/papers.jsonl                  every paper ever downloaded, one record per arXiv id
    data/library/download_done.json            queries that are finished and need not be asked again
    data/library/embeddings/<model>/           one vector per (paper id, text hash), shared by every run
    data/runs/<name>/                          one analysis: adjacency, communities, graph, report
"""
import os
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
DATA = Path(os.environ.get("RP_DATA") or REPO / "data")
LIBRARY = DATA / "library"
RUNS = DATA / "runs"
