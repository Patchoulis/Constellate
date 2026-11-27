"""The arXiv API: every paper matching a search query, as library records (metadata and abstract, no PDF)."""

import re
import time
import xml.etree.ElementTree as ET

import requests

API = "https://export.arxiv.org/api/query"
ATOM = "{http://www.w3.org/2005/Atom}"
ARXIV = "{http://arxiv.org/schemas/atom}"
PAGE = 500
DELAY = 3.1  # arXiv asks for one request every three seconds
HEADERS = {"User-Agent": "research-partitioner/0.1 (python-requests)"}


def _text(node, tag):
    found = node.find(tag)
    return re.sub(r"\s+", " ", found.text).strip() if found is not None and found.text else ""


def _parse(xml: str):
    root = ET.fromstring(xml)
    total = int(root.findtext("{http://a9.com/-/spec/opensearch/1.1/}totalResults") or 0)
    papers = []
    for entry in root.findall(ATOM + "entry"):
        url = _text(entry, ATOM + "id")
        if "/abs/" not in url:
            continue
        paper_id = re.sub(r"v\d+$", "", url.split("/abs/")[1])
        primary = entry.find(ARXIV + "primary_category")
        papers.append(
            {
                "id": paper_id,
                "title": _text(entry, ATOM + "title"),
                "abstract": _text(entry, ATOM + "summary"),
                "authors": [_text(a, ATOM + "name") for a in entry.findall(ATOM + "author")],
                "published": _text(entry, ATOM + "published"),
                "updated": _text(entry, ATOM + "updated"),
                "comment": _text(entry, ARXIV + "comment"),
                "primary_category": primary.get("term") if primary is not None else "",
                "categories": [c.get("term") for c in entry.findall(ATOM + "category")],
                "url": f"https://arxiv.org/abs/{paper_id}",
                "pdf_url": f"https://arxiv.org/pdf/{paper_id}",
            }
        )
    return total, papers


def _page(search: str, start: int, log) -> tuple:
    """One page of results: (total matches, papers).  Retries with a growing pause; the API sometimes answers
    with an error or an empty page."""
    params = {
        "search_query": search,
        "start": start,
        "max_results": PAGE,
        "sortBy": "submittedDate",
        "sortOrder": "ascending",
    }
    for attempt in range(1, 7):
        time.sleep(DELAY * attempt)
        try:
            response = requests.get(API, params=params, headers=HEADERS, timeout=90)
            if response.status_code != 200:
                raise RuntimeError(f"HTTP {response.status_code}")
            total, papers = _parse(response.text)
            if papers or start >= total:
                return total, papers
            raise RuntimeError("empty page")
        except Exception as error:  # noqa: BLE001
            log(f"    retry {attempt} at offset {start}: {error}")
    raise RuntimeError(f"arXiv API kept failing at offset {start} for query: {search}")


def query(search: str, log=print):
    """Yield every paper matching an arXiv search query, oldest first."""
    start, total = 0, None
    while total is None or start < total:
        total, papers = _page(search, start, log)
        yield from papers
        start += PAGE
        log(f"    {min(start, total)}/{total}")
