"""Which papers a run is about: a set of conferences, or a submission-date range with optional categories.

The one place a request (from the HTTP API or the command line) is checked and turned into a Selection.
"""

import hashlib
import re
from dataclasses import asdict, dataclass
from datetime import date

MOST_NAMED = 3  # conferences spelled out in a run's name


def _clean(values) -> tuple:
    return tuple(v.strip() for v in values or () if v and v.strip())


@dataclass(frozen=True)
class Selection:
    venues: tuple = ()
    start: date | None = None
    end: date | None = None
    categories: tuple = ()
    include_workshops: bool = False

    @classmethod
    def parse(cls, venues=(), start=None, end=None, categories=(), include_workshops=False) -> "Selection":
        """Raises ValueError with a message meant for the person who made the request."""
        venues = _clean(venues)
        if venues:
            if any(not re.fullmatch(r".+ \d{4}", venue) for venue in venues):
                raise ValueError('Write each conference as "Name Year", e.g. NeurIPS 2025')
            return cls(venues=venues, include_workshops=bool(include_workshops))
        try:
            first, last = date.fromisoformat(str(start)), date.fromisoformat(str(end))
        except ValueError:
            raise ValueError("Give conferences, or a start date and an end date (YYYY-MM-DD)") from None
        if first > last:
            raise ValueError("The start date is after the end date")
        return cls(start=first, end=last, categories=_clean(categories))

    @property
    def name(self) -> str:
        """Folder name of the run."""
        if self.venues:
            names = [re.sub(r"\W+", "", venue).lower() for venue in self.venues]
            if len(names) > MOST_NAMED:  # a long list would not fit in a file name
                digest = hashlib.sha1(",".join(sorted(names)).encode()).hexdigest()[:6]
                names = names[:MOST_NAMED] + [f"and{len(names) - MOST_NAMED}more", digest]
            return "_".join(names)
        return f"arxiv_{self.start}_{self.end}" + ("_" + "-".join(self.categories) if self.categories else "")

    def tags(self, paper: dict) -> list:
        """What a paper counts as in a community's mix: its conferences, or its primary category."""
        if self.venues:
            return sorted(set(paper.get("venues", [])) & set(self.venues))
        return [paper["primary_category"]]

    def to_json(self) -> dict:
        return {key: str(value) if isinstance(value, date) else value for key, value in asdict(self).items()}
