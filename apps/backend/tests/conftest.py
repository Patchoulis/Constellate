"""Tests never touch the real data folder: RP_DATA is pointed at a throwaway folder before `rp` is imported."""

import os
import sys
import tempfile
from pathlib import Path

os.environ["RP_DATA"] = tempfile.mkdtemp(prefix="rp-tests-")
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import numpy as np  # noqa: E402
import pytest  # noqa: E402


@pytest.fixture
def blobs():
    """Three well-separated groups of 40 points in 16 dimensions: (unit vectors, the group of each)."""
    rng = np.random.default_rng(0)
    centres = rng.normal(size=(3, 16)) * 4
    truth = np.repeat(np.arange(3), 40)
    points = centres[truth] + rng.normal(size=(120, 16))
    return points.astype(np.float32), truth


def paper(paper_id, title="A title", abstract="An abstract.", **more):
    return {
        "id": paper_id,
        "title": title,
        "abstract": abstract,
        "authors": [],
        "published": "2026-01-02T00:00:00Z",
        "updated": "2026-01-02T00:00:00Z",
        "comment": "",
        "primary_category": "cs.LG",
        "categories": ["cs.LG"],
        **more,
    }
