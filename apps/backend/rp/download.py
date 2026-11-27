"""Bring the library up to date for a selection.

A selection is fetched as a series of slices, each remembered in the library once it is finished:
  * a submission-date range: one arXiv query per day (optionally restricted to categories), so no single query
    runs into the arXiv API's deep-paging limit;
  * conferences: one slice per conference, from the conference's own list of accepted papers; only when a
    conference has no such list, from arXiv papers whose comment names it ("Accepted at CVPR 2026").

A slice that finished once is not fetched again, and a paper whose id is already in the library is not stored
again, unless `redo` is set.
"""

import re
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from datetime import date, timedelta

from . import arxiv, conferences
from .library import Papers
from .selection import Selection


@dataclass
class Slice:
    key: str  # how the library remembers that this slice is finished
    label: str
    fetch: Callable[[Callable], Iterable[dict]]  # fetch(log) -> papers
    venue: str | None = None  # tag stored on the papers
    final: bool = True  # False: the slice can still gain papers, fetch again next time


def _from_arxiv_comments(venue: str, include_workshops: bool, log) -> Iterable[dict]:
    name, year = venue.rsplit(" ", 1)
    names_venue = re.compile(rf"{re.escape(name)}\W*{year}", re.I).search
    for paper in arxiv.query(f'co:"{venue}"', log):
        comment = paper["comment"]
        if names_venue(comment) and (include_workshops or not re.search("workshop", comment, re.I)):
            yield paper


def _venue_slice(venue: str, include_workshops: bool) -> Slice:
    def fetch(log):
        try:
            return conferences.accepted_papers(venue, log)
        except conferences.NoOfficialList as error:
            log(f"    {error}; using arXiv papers whose comment names {venue} instead")
            return _from_arxiv_comments(venue, include_workshops, log)

    return Slice(f"venue|{venue}|{include_workshops}|official", venue, fetch, venue)


def _day_slice(day: date, categories) -> Slice:
    stamp = day.strftime("%Y%m%d")
    cats = " OR ".join(f"cat:{c}" for c in categories)
    search = f"submittedDate:[{stamp}0000 TO {stamp}2359]" + (f" AND ({cats})" if cats else "")
    return Slice(
        f"day|{day.isoformat()}|{','.join(sorted(categories))}",
        day.isoformat(),
        lambda log: arxiv.query(search, log),
        final=day < date.today() - timedelta(days=2),
    )  # recent days can still grow


def slices(selection: Selection) -> list:
    if selection.venues:
        return [_venue_slice(venue, selection.include_workshops) for venue in selection.venues]
    days = (selection.end - selection.start).days + 1
    return [_day_slice(selection.start + timedelta(days=offset), selection.categories) for offset in range(days)]


def download(selection: Selection, redo=False, log=print) -> Papers:
    library = Papers()
    todo = [piece for piece in slices(selection) if redo or piece.key not in library.done]
    skipped = len(slices(selection)) - len(todo)
    if skipped:
        log(f"  {skipped} of {skipped + len(todo)} already downloaded: skipped")
    for piece in todo:
        log(f"  {piece.label}")
        new, known = library.add(piece.fetch(log), venue=piece.venue, overwrite=redo)
        log(f"    {new} new, {known} already in the library")
        library.save(piece.key if piece.final else None)
    return library
