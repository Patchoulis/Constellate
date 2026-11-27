"""Accepted papers of a conference, from the conference's own site.

NeurIPS, ICML, ICLR, CVPR and others (catalog.py lists them) publish the full list of accepted papers as data
files on their virtual-conference sites:

    https://<host>/static/virtual/data/<slug>-<year>-orals-posters.json     every poster and oral
    https://<host>/static/virtual/data/<slug>-<year>-abstracts.json         {event id: abstract}, newer years only
                                                                            (older years carry the abstract inline)

That is the complete, official list, unlike matching "Accepted at ..." in arXiv comments, which only finds the
papers that are on arXiv and say so.
"""
import re

import requests

from .catalog import official_host

HEADERS = {"User-Agent": "research-partitioner/0.1 (python-requests)"}


class NoOfficialList(Exception):
    """This conference (or this year of it) has no list in the known place."""


def _data(host: str, slug: str, year: str, part: str, required=True):
    url = f"https://{host}/static/virtual/data/{slug}-{year}-{part}.json"
    response = requests.get(url, headers=HEADERS, timeout=300)
    if response.status_code == 200 and "json" in response.headers.get("content-type", ""):
        return response.json()
    if required:
        raise NoOfficialList(f"{url} answered HTTP {response.status_code}")
    return {}


def _forum(entry: dict) -> str | None:
    """The OpenReview forum id, which is the same for a paper's poster and its oral."""
    found = re.search(r"[?&]id=([\w-]+)", entry.get("paper_url") or "")
    return found.group(1) if found else None


def accepted_papers(venue: str, log=print) -> list:
    """Library records for every accepted paper of e.g. "NeurIPS 2025".  Raises NoOfficialList."""
    name, year = venue.rsplit(" ", 1)
    slug, host = name.lower(), official_host(name)
    if host is None:
        raise NoOfficialList(f"no official list is known for {name}")
    entries = _data(host, slug, year, "orals-posters")["results"]
    abstracts = _data(host, slug, year, "abstracts", required=False)
    found, seen = [], set()
    for entry in sorted(entries, key=lambda e: e.get("eventtype") != "Poster"):      # posters first: one per paper
        # A paper with an oral is listed again for the talk, often without its paper link (or with a made-up
        # one), so two entries are the same paper when they share the link OR the title.
        forum = _forum(entry)
        keys = {re.sub(r"\W+", "", entry["name"].lower())} | ({forum} if forum else set())
        if keys & seen:
            continue
        seen |= keys
        topic = entry.get("topic") or ""
        found.append({
            "id": f"{slug}-{year}-{entry['id']}",
            "title": re.sub(r"\s+", " ", entry["name"]).strip(),
            "abstract": re.sub(r"\s+", " ", entry.get("abstract") or abstracts.get(str(entry["id"])) or "").strip(),
            "authors": [author["fullname"] for author in entry.get("authors") or []],
            "published": entry.get("starttime") or f"{year}-01-01",
            "updated": entry.get("starttime") or f"{year}-01-01",
            "comment": entry.get("decision") or "",
            "primary_category": topic,
            "categories": [topic] if topic else [],
            "url": entry.get("paper_url") or f"https://{host}{entry.get('virtualsite_url', '')}",
            "pdf_url": f"https://openreview.net/pdf?id={forum}" if forum else None,
        })
    log(f"    {len(found)} accepted papers on {host} ({sum(not p['abstract'] for p in found)} without an abstract)")
    return found
