"""User settings, stored in data/settings.json.  Unknown keys are dropped, missing keys take the default."""

from . import DATA, store

DEFAULTS = {
    "embedding_model": "Qwen/Qwen3-Embedding-0.6B",
    "center_embeddings": True,  # remove the mean embedding before cosine similarity
    "clustering": "spectral",  # how communities are cut: see cluster.METHODS
    "max_communities": 24,
    "min_community_size": 15,
    "min_split_modularity": 0.05,  # a Fiedler split is kept only if its two halves are this distinct
    "sub_levels": 2,  # how many times a community is split again into sub-communities
    "max_links": 8,  # most links drawn from one paper to its closest papers
    # Score every abstract for wording of the kind language models produce, and tag those above the threshold.
    # It needs a trained detector (detect.availability says whether there is one); it needs no graphics card.
    "detect_ai": True,
    # Papers the AI-text detector flags are always tagged; switch this off to leave them out of a run altogether.
    "include_ai_papers": True,
    # Which conference papers go into a run: "all", "highlights" (spotlights and orals) or "oral".
    "acceptance_filter": "all",
    # Papers whose authors' declarations raise one of these flags (proceedings.FLAGS) are left out.  It can only
    # act on papers whose declarations have been read.
    "exclude_flags": [],
    "describe": True,  # ask Claude to describe every community
    "describe_model": "claude-opus-5-5",
}


# acceptance_filter -> (name as shown, the tiers it lets through)
ACCEPTANCE = {
    "all": ("All accepted papers", ("oral", "spotlight", "poster")),
    "highlights": ("Spotlights and orals", ("oral", "spotlight")),
    "oral": ("Orals only", ("oral",)),
}

# What each number may be: (smallest, largest).
LIMITS = {
    "max_communities": (2, 500),
    "min_community_size": (2, 100_000),
    "min_split_modularity": (0.0, 1.0),
    "sub_levels": (0, 6),
    "max_links": (1, 50),
}


def _checked(key: str, value):
    """`value` as the setting `key` must be.  Raises ValueError with a message for the person changing it."""
    default = DEFAULTS[key]
    if isinstance(default, list):
        from .proceedings import FLAGS

        if not isinstance(value, list) or any(flag not in FLAGS for flag in value):
            raise ValueError(f"{key} must be a list of: {', '.join(FLAGS)}")
        return [flag for flag in FLAGS if flag in value]
    if isinstance(default, bool):
        if not isinstance(value, bool):
            raise ValueError(f"{key} must be true or false")
        return value
    if isinstance(default, (int, float)):
        try:
            number = float(value) if not isinstance(value, bool) else None  # true is not a number
        except (TypeError, ValueError):
            number = None
        low, high = LIMITS[key]
        if number is None or not low <= number <= high:  # also refuses NaN and infinity
            raise ValueError(f"{key} must be a number between {low} and {high}")
        if isinstance(default, int):
            if number != int(number):
                raise ValueError(f"{key} must be a whole number")
            return int(number)
        return number
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{key} must be a name")
    if key == "clustering":
        from .cluster import METHODS

        if value not in METHODS:
            raise ValueError(f"clustering must be one of {', '.join(METHODS)}")
    if key == "acceptance_filter" and value not in ACCEPTANCE:
        raise ValueError(f"acceptance_filter must be one of {', '.join(ACCEPTANCE)}")
    return value.strip()


def load() -> dict:
    stored = store.read_json(DATA / "settings.json", {})
    return {key: stored.get(key, default) for key, default in DEFAULTS.items()}


def save(changes: dict) -> dict:
    """Change some settings; unknown keys are ignored, a bad value changes nothing at all (ValueError)."""
    checked = {key: _checked(key, value) for key, value in changes.items() if key in DEFAULTS}
    store.update_json(DATA / "settings.json", lambda stored: stored.update(checked), indent=1)
    return load()
