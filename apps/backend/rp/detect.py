"""Estimate which abstracts were written or rewritten by a language model.

What this is NOT: watermark detection.  A watermark can only be checked by whoever holds the generating model's
secret key, and most providers do not watermark text, so there is nothing in an arbitrary abstract to look up.

What it is: a small version of the recipe Pangram Labs published for their detector (Emi & Spero 2024,
"Technical Report on the Pangram AI-Generated Text Classifier", arXiv:2402.14873):

  synthetic mirrors     For a human document, a model is prompted to produce a document on the same topic and of
                        the same length.  Trained on such pairs, a classifier cannot use topic as a shortcut:
                        only how the text is written differs.  Here the human documents are abstracts submitted
                        to arXiv in 2019, before such models were in use, and each gets two mirrors: one a model
                        "written" from the title alone, one the human abstract "polished" by a model, which is
                        how these models are mostly used on papers.
  hard negative mining  Train; run the classifier over a large pool of human documents; take the ones it most
                        wrongly scores as machine-written; make mirrors for exactly those; add both and train
                        again.  Repeat while the classifier, measured on held-out documents, improves.  This is
                        what pushes false alarms down: the model is shown its own mistakes.
  the classifier        Pangram fine-tunes a transformer on tens of millions of documents.  That is out of
                        reach here, and the cheap substitute was tried and measured: a linear classifier on a
                        frozen embedding model's vector caught 57% of written and 3% of polished abstracts,
                        because such a vector encodes what a text is about, not how it is written.  So the
                        classifier here is a logistic regression on word and word-pair frequencies (language
                        models favour certain words: Kobak et al. 2024, "Delving into ChatGPT usage in academic
                        writing through excess vocabulary"), which caught 95% and 44% on the same held-out
                        papers.  It also explains itself: the words that raised a score can be listed.

Everything reported is measured on held-out papers.  A fifth of the pool, chosen by id, is never trained on and
never mined; half of that decides when mining stops and the other half, used for no decision at all, is what
the reported numbers come from.  The threshold is set where 1% of held-out human abstracts score higher
(FALSE_ALARMS); the share of
held-out mirrors above it is the catch rate, for "written" and "polished" separately.  `python -m rp.detect`
builds it all and prints the numbers.  What the numbers do not show:
  * the mirrors come from the models in GENERATORS only; other models' text may read differently;
  * the scale is thousands of documents where Pangram uses tens of millions, and one domain;
  * "human" is 2019 text: how often present-day human abstracts are flagged cannot be measured, since nobody
    knows which of them are purely human.
So a flag means "written the way these models write", never proof of how a paper was written.

Building the training set calls the Claude API (about 2,000 short generations).  Scoring a run's papers afterwards
is local and takes seconds.  Until the training set has been built nothing is flagged.
"""

import hashlib
from concurrent.futures import ThreadPoolExecutor
from enum import StrEnum

import numpy as np

from . import LIBRARY, arxiv, claude, store
from .library import text_hash

FALSE_ALARMS = 0.01
GENERATORS = ("claude-sonnet-5-5", "claude-haiku-5-5")
POOL_QUERY = "submittedDate:[201905010000 TO 201906302359] AND (cat:cs.LG OR cat:stat.ML OR cat:cs.CV OR cat:cs.CL)"
POOL = 5000  # human abstracts from 2019 to draw on
START = 400  # of them, how many get mirrors before any mining
MINED = 150  # hard negatives added (with mirrors) per round of mining
ROUNDS = 3  # at most this many rounds of mining
HELD_OUT_MIRRORS = 150  # held-out papers that get mirrors, to measure the catch rate
MIN_WORDS = 60  # shorter abstracts are not scored and never flagged
SAVE_EVERY = 40  # mirrors generated between two saves of the training set
PROMPTS = {
    "written": (
        "Write the abstract of a research paper with the given title, as it would appear on arXiv: one "
        "paragraph of 150 to 250 words. Output only the abstract."
    ),
    "polished": (
        "Improve the wording, clarity and flow of this research abstract. Keep every claim, number and "
        "technical term, and about the same length. Output only the rewritten abstract."
    ),
}

FOLDER = LIBRARY / "ai_detection"
TRAINING = FOLDER / "training.json"  # {"pool": [...], "machine": [...], "train": [paper ids, in the order added],
#  "rounds": [how many of them were in after each round]}
REPORT = FOLDER / "validation.json"
SCORES = FOLDER / "scores.json"  # {paper id: [text hash, classifier version, score or null]}


def _side(paper_id: str) -> str:
    """Which part a paper belongs to, decided by its id so that a paper and its mirrors always go together:
    "train"       80%: may be trained on and mined
    "validation"  10%: never trained on; used to decide whether a round of mining helped
    "test"        10%: never trained on and never used for any decision; everything reported comes from here"""
    bucket = int(hashlib.sha1(paper_id.encode()).hexdigest(), 16) % 10
    return {0: "validation", 5: "test"}.get(bucket, "train")


def _held_out(paper_id: str) -> bool:
    return _side(paper_id) != "train"


class Classifier:
    """A logistic regression on word and word-pair frequencies."""

    def __init__(self, humans: list, machine: list):
        """`humans` and `machine` are texts.  Fitted on all of them."""
        from sklearn.feature_extraction.text import TfidfVectorizer
        from sklearn.linear_model import LogisticRegression

        texts = humans + machine
        self.words = TfidfVectorizer(
            lowercase=True,
            ngram_range=(1, 2),
            min_df=3,
            sublinear_tf=True,
            max_features=60_000,
            token_pattern=r"(?u)\b[a-zA-Z][a-zA-Z'\-]+\b",
        )
        self.model = LogisticRegression(C=2.0, max_iter=2000, class_weight="balanced")
        self.model.fit(self.words.fit_transform(texts), [False] * len(humans) + [True] * len(machine))
        self.terms = np.array(self.words.get_feature_names_out())
        self.threshold = float("inf")  # set from held-out human abstracts by whoever measures it
        self.version = hashlib.sha1(
            "|".join(sorted(hashlib.sha1(t.encode()).hexdigest() for t in texts)).encode()
        ).hexdigest()[:10]

    def score(self, texts) -> np.ndarray:
        """Log-odds of "machine": higher = more machine-like; NaN for a text too short to judge."""
        texts = list(texts)
        scores = self.model.decision_function(self.words.transform(texts)) if texts else np.zeros(0)
        return np.where([len(text.split()) >= MIN_WORDS for text in texts], scores, np.nan)

    def markers(self, text: str, top=6) -> list:
        """The words and phrases in `text` that pushed its score up the most."""
        weights = self.words.transform([text]).multiply(self.model.coef_[0]).tocoo()
        order = np.argsort(-weights.data)[:top]
        return [str(self.terms[weights.col[i]]) for i in order if weights.data[i] > 0]


def _caught(scores, threshold) -> float:
    return float(np.mean(scores > threshold)) if len(scores) else float("nan")


def _measure(model: Classifier, held_human: list, held_machine: list) -> dict:
    """What a classifier does on held-out texts: the threshold at FALSE_ALARMS, and the catch rate above it."""
    from sklearn.metrics import roc_auc_score

    human = model.score(held_human)
    human = human[~np.isnan(human)]
    machine = {kind: model.score([m["text"] for m in held_machine if m["kind"] == kind]) for kind in PROMPTS}
    threshold = float(np.quantile(human, 1 - FALSE_ALARMS))
    everything = np.nan_to_num(np.concatenate(list(machine.values())), nan=-np.inf)
    return {
        "threshold": threshold,
        **{f"{kind}_caught": _caught(np.nan_to_num(machine[kind], nan=-np.inf), threshold) for kind in PROMPTS},
        "auc": float(
            roc_auc_score(
                [0] * len(human) + [1] * len(everything),
                np.concatenate([human, np.where(np.isinf(everything), human.min(), everything)]),
            )
        ),
    }


def _fit(data: dict, train_ids) -> Classifier:
    """The classifier trained on these papers of the pool and their mirrors."""
    wanted = set(train_ids)
    return Classifier(
        [p["abstract"] for p in data["pool"] if p["id"] in wanted],
        [m["text"] for m in data["machine"] if m["id"] in wanted],
    )


def _held(data: dict, side: str) -> tuple:
    """(human abstracts, mirrors) of the "validation" or the "test" papers."""
    held = [p for p in data["pool"] if _side(p["id"]) == side]
    ids = {p["id"] for p in held}
    return [p["abstract"] for p in held], [m for m in data["machine"] if m["id"] in ids]


class Trainer:
    """Builds the training set: pool, mirrors, rounds of hard negative mining.  Everything it generates is saved
    as it goes, so a run that stops can be continued without paying for the same mirror twice."""

    def __init__(self, log=print):
        self.log = log
        self.data = store.read_json(TRAINING, {"pool": [], "machine": [], "train": [], "rounds": []})
        self.clients = {}

    def _save(self):
        store.write_json(TRAINING, self.data)

    def _pool(self):
        if len(self.data["pool"]) >= POOL:
            return
        self.log(f"  downloading {POOL} abstracts from 2019 ...")
        pool = []
        for paper in arxiv.query(POOL_QUERY, log=lambda line: None):
            if len(paper["abstract"].split()) >= MIN_WORDS:
                pool.append({"id": paper["id"], "title": paper["title"], "abstract": paper["abstract"]})
            if len(pool) >= POOL:
                break
        self.data["pool"] = pool
        self._save()

    def _mirror(self, papers: list):
        """Make the mirrors that these papers do not have yet."""
        have = {(m["id"], m["kind"]) for m in self.data["machine"]}
        jobs = [
            (paper, kind, GENERATORS[index % len(GENERATORS)])
            for index, paper in enumerate(papers)
            for kind in PROMPTS
            if (paper["id"], kind) not in have
        ]
        if not jobs:
            return
        self.log(f"  generating {len(jobs)} mirrors ...")
        for model in GENERATORS:
            self.clients.setdefault(model, claude.connect(model))

        def generate(job):
            paper, kind, model = job
            source = paper["title"] if kind == "written" else paper["abstract"]
            try:
                text = claude.ask(
                    self.clients[model],
                    model,
                    PROMPTS[kind],
                    source,
                    max_tokens=2000,
                    purpose="training the AI-text detector",
                )
            except claude.NoAnswer:
                return None
            return {"id": paper["id"], "kind": kind, "generator": model, "text": text}

        with ThreadPoolExecutor(max_workers=12) as pool:
            for done, made in enumerate(pool.map(generate, jobs), start=1):
                if made and len(made["text"].split()) >= MIN_WORDS:
                    self.data["machine"].append(made)
                if done % SAVE_EVERY == 0:
                    self._save()  # a run that stops here keeps what it has paid for
        self._save()

    def build(self) -> dict:
        self._pool()
        pool = self.data["pool"]
        held = [p for p in pool if _held_out(p["id"])]
        free = [p for p in pool if not _held_out(p["id"])]
        self._mirror(held[:HELD_OUT_MIRRORS])
        held_human, held_machine = _held(self.data, "validation")  # rounds are judged here, not on "test"
        headline = lambda measured: (measured["written_caught"] + measured["polished_caught"]) / 2  # noqa: E731

        train = [p["id"] for p in free[:START]]
        self._mirror(free[:START])
        model = _fit(self.data, train)
        measured = _measure(model, held_human, held_machine)
        rounds = [len(train)]
        self.log(
            f"  round 0: {len(train)} humans with mirrors -> written {measured['written_caught']:.0%}, "
            f"polished {measured['polished_caught']:.0%} caught at {FALSE_ALARMS:.0%} false alarms"
        )
        for number in range(1, ROUNDS + 1):
            # the human abstracts the current classifier most wrongly takes for machine text
            rest = [p for p in free if p["id"] not in set(train)]
            hardest = [rest[i] for i in np.argsort(-model.score(p["abstract"] for p in rest))[:MINED]]
            self._mirror(hardest)
            candidate = train + [p["id"] for p in hardest]
            next_model = _fit(self.data, candidate)
            next_measured = _measure(next_model, held_human, held_machine)
            self.log(
                f"  round {number}: +{len(hardest)} hard negatives -> written "
                f"{next_measured['written_caught']:.0%}, polished {next_measured['polished_caught']:.0%} caught"
            )
            if headline(next_measured) <= headline(measured):
                self.log("  no better than the round before: stopping there")
                break
            train, model, measured = candidate, next_model, next_measured
            rounds.append(len(train))
        self.data.update(train=train, rounds=rounds)
        self._save()
        return self.data


_classifier = {"stamp": None, "value": None, "report": None}


def classifier() -> Classifier | None:
    """The classifier for the current training set, or None when no training set has been built."""
    if not TRAINING.exists():
        return None
    stamp = TRAINING.stat().st_mtime
    if _classifier["stamp"] != stamp:
        data = store.read_json(TRAINING)
        if not data.get("train"):
            return None
        held_human, held_machine = _held(data, "test")  # untouched by training and by every choice
        model = _fit(data, data["train"])
        measured = _measure(model, held_human, held_machine)
        model.threshold = measured["threshold"]
        # what each round of mining bought: the same measurement with only the papers that were in by then
        sizes = [step["humans"] if isinstance(step, dict) else step for step in data["rounds"]]
        rounds = [
            {
                "round": number,
                "humans": size,
                **{
                    k: v
                    for k, v in _measure(_fit(data, data["train"][:size]), held_human, held_machine).items()
                    if k.endswith("caught")
                },
            }
            for number, size in enumerate(sizes)
        ]
        weights = model.model.coef_[0]
        _classifier.update(
            stamp=stamp,
            value=model,
            report={
                "generators": list(GENERATORS),
                "trained_on": len(data["train"]),
                "held_out": len(held_human),
                "false_alarms": FALSE_ALARMS,
                "threshold": round(model.threshold, 3),
                "written_caught": measured["written_caught"],
                "polished_caught": measured["polished_caught"],
                "auc": measured["auc"],
                "rounds": rounds,
                "machine_words": model.terms[np.argsort(-weights)[:25]].tolist(),
                "human_words": model.terms[np.argsort(weights)[:25]].tolist(),
            },
        )
    return _classifier["value"]


class Availability(StrEnum):
    """Whether abstracts get scored in the next run.  Exactly one holds, and `availability` says why."""

    OFF = "off"  # switched off in Settings
    UNTRAINED = "untrained"  # switched on, but there is no detector on this computer to score with
    READY = "ready"  # switched on and there is one


def availability(enabled: bool) -> dict:
    """{"state": an Availability, "why": a sentence for the interface}."""
    if not enabled:
        return {
            "state": Availability.OFF,
            "why": "Switched off: abstracts are not scored and no paper is tagged. Papers tagged in earlier runs "
            "keep their tag in those runs.",
        }
    if classifier() is None:
        return {
            "state": Availability.UNTRAINED,
            "why": "Not available yet: no detector has been trained on this computer, so no paper is tagged. "
            "Training has a language model write and polish about 1,700 abstracts to learn its wording from; that "
            "needs an Anthropic API key and costs a few US dollars once. Run `python -m rp.detect --train` in "
            "apps/backend. It does not need a graphics card.",
        }
    return {
        "state": Availability.READY,
        "why": "On: every abstract is scored once (a fraction of a second per thousand, no graphics card needed) "
        "and papers above the threshold are tagged.",
    }


class Detector:
    """Scores for papers, kept in the library so each abstract is scored once per classifier."""

    def __init__(self, log=print):
        self.log = log
        self.classifier = classifier()

    @property
    def threshold(self) -> float:
        return self.classifier.threshold if self.classifier else float("inf")

    def scores(self, papers) -> np.ndarray:
        """One score per paper; NaN where there is no verdict (no classifier yet, or the abstract is too short)."""
        if self.classifier is None:
            self.log("  no detector has been trained yet (python -m rp.detect): nothing is flagged")
            return np.full(len(papers), np.nan)
        version = self.classifier.version
        stored = store.read_json(SCORES, {})
        fresh = lambda paper: stored.get(paper["id"], [None, None])[:2] == [text_hash(paper), version]  # noqa: E731
        missing = [paper for paper in papers if not fresh(paper)]
        self.log(f"  {len(papers) - len(missing)} scored before, {len(missing)} to score")
        if missing:
            new = {
                paper["id"]: [text_hash(paper), version, None if np.isnan(score) else round(float(score), 3)]
                for paper, score in zip(missing, self.classifier.score(p["abstract"] for p in missing))
            }
            stored = store.update_json(SCORES, lambda content: content.update(new))
        return np.array([np.nan if stored[p["id"]][2] is None else stored[p["id"]][2] for p in papers])

    def verdict(self, paper: dict) -> dict | None:
        """What a run found for one paper, for showing; None when it was never scored by this classifier."""
        entry = store.read_json(SCORES, {}).get(paper["id"])
        if (
            self.classifier is None
            or entry is None
            or entry[:2] != [text_hash(paper), self.classifier.version]
            or entry[2] is None
        ):
            return None
        return {
            "score": entry[2],
            "threshold": round(self.threshold, 3),
            "flagged": entry[2] > self.threshold,
            "false_alarms": FALSE_ALARMS,
            "markers": self.classifier.markers(paper["abstract"]),
        }


# One week of arXiv submissions per year, in the same categories as the training pool.
YEARS = {
    "2019": "201908050000 TO 201908092359",
    "2021": "202110040000 TO 202110082359",
    "2022": "202210030000 TO 202210072359",
    "2023": "202310020000 TO 202310062359",
    "2024": "202410070000 TO 202410112359",
    "2025": "202510060000 TO 202510102359",
}


def over_the_years(sample=1000, log=print) -> dict:
    """The share of abstracts flagged, for one week of arXiv submissions in each of several years.

    This is the check that the classifier reads machine wording and not merely newer wording: its human examples
    are all from 2019, so if writing style had simply drifted, abstracts from 2021 and 2022 (before ChatGPT, at
    the end of November 2022) would already be flagged more often.  A rate that stays at the false-alarm level
    until then and rises afterwards is what language-model use looks like; a rate that climbs steadily from 2019
    would mean the classifier is dating the text instead."""
    model = classifier()
    categories = POOL_QUERY.split(" AND ", 1)[1]
    shares = {}
    for year, dates in YEARS.items():
        texts = []
        for paper in arxiv.query(f"submittedDate:[{dates}] AND {categories}", log=lambda line: None):
            texts.append(paper["abstract"])
            if len(texts) >= sample:
                break
        scores = model.score(texts)
        scores = scores[~np.isnan(scores)]
        shares[year] = {"abstracts": len(scores), "flagged": float(np.mean(scores > model.threshold))}
        log(f"  {year}: {shares[year]['flagged']:.1%} of {len(scores)} abstracts flagged")
    store.update_json(REPORT, lambda content: content.__setitem__("by_year", shares), indent=1)
    return shares


def report(log=print) -> dict | None:
    """What was measured about the current classifier on held-out papers; also saved for the interface."""
    if classifier() is None:
        return None
    r = _classifier["report"] | {"by_year": (store.read_json(REPORT) or {}).get("by_year")}
    store.write_json(REPORT, r, indent=1)
    log(
        f"trained on {r['trained_on']} human abstracts and their mirrors; measured on {r['held_out']} held-out "
        f"human abstracts from 2019 and the mirrors of {HELD_OUT_MIRRORS} of them"
    )
    log(f"at a threshold that flags {r['false_alarms']:.0%} of held-out human abstracts:")
    log(
        f"  written from the title {r['written_caught']:.0%} caught, polished by a model "
        f"{r['polished_caught']:.0%} caught, AUC {r['auc']:.3f}"
    )
    for step in r["rounds"]:
        log(
            f"  mining round {step['round']}: {step['humans']} humans -> written {step['written_caught']:.0%}, "
            f"polished {step['polished_caught']:.0%}"
        )
    log(f"words that point to a model: {', '.join(r['machine_words'])}")
    log(f"words that point to a human: {', '.join(r['human_words'])}")
    return r


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--train", action="store_true", help="build the training set (calls the Claude API)")
    parser.add_argument("--years", action="store_true", help="also check the flag rate on arXiv, 2019 to 2025")
    arguments = parser.parse_args()
    if arguments.train or not TRAINING.exists():
        Trainer().build()
    report()
    if arguments.years:
        over_the_years()
