"""HTTP API for the web app.

python -m rp.server          http://127.0.0.1:8765

POST /api/runs               start the pipeline (one at a time)      GET /api/status       job state + log
GET  /api/runs               finished runs                           GET /api/runs/<name>  communities
PATCH /api/runs/<name>       rename it                               DELETE /api/runs/<name>  remove its results
GET  /api/runs/<name>/graph  nodes + edges                           GET/PUT /api/settings
GET/POST /api/runs/<name>/overview/<path>   a community's overview: its state / start writing it
GET  /api/papers/<id>        a paper's record                        GET /api/pdf/<id>     its PDF (cached)
GET  /api/library            what is stored locally                  GET /api/catalog      what there is to pick from
"""

import json
import os
import shutil

from flask import Flask, jsonify, request, send_file

from . import LIBRARY, RUNS, catalog, claude, currency, detect, pipeline, proceedings, reviews, settings, store, usage
from .jobs import Busy, Job, State
from .library import Papers, acceptance
from .overview import Overviews
from .selection import Selection

app = Flask(__name__)
app.config["MAX_CONTENT_LENGTH"] = 4 * 1024 * 1024  # the largest thing sent here is a page's text
job = Job()


LOCAL = {"127.0.0.1", "localhost", "[::1]"}


def _host(address: str) -> str:
    """The host of "host:port" or of "scheme://host:port/..."."""
    return address.split("://")[-1].split("/")[0].rsplit(":", 1)[0].lower()


@app.before_request
def only_from_this_machine():
    """This API can spend money (Claude) and delete runs, so it answers only requests addressed to this machine
    and, for anything that changes state, only ones that come from the app itself: a web page open in a browser
    here must not be able to post to it (cross-site request forgery), nor reach it under another name (DNS
    rebinding)."""
    if _host(request.host) not in LOCAL:
        return jsonify(error="this API only answers on this machine"), 403
    origin = request.headers.get("Origin")
    if request.method != "GET" and origin and not _ours(origin):
        return jsonify(error="requests from other sites are refused"), 403


def _ours(origin: str) -> bool:
    """Is this the app's own interface?  The desktop app says where its interface is (RP_ORIGIN), and then only
    that exact origin counts: another program's page on this machine is another site.  Without it (development),
    any page served from this machine counts."""
    expected = os.environ.get("RP_ORIGIN")
    return origin.rstrip("/").lower() == expected.rstrip("/").lower() if expected else _host(origin) in LOCAL


def _body() -> dict:
    """The request's JSON object.  Anything else is the sender's mistake."""
    body = request.get_json(silent=True)
    if not isinstance(body, dict):
        raise ValueError("the request body must be a JSON object")
    return body


@app.errorhandler(LookupError)
def not_found(_):
    return jsonify(error="not found"), 404


@app.errorhandler(ValueError)
def bad_request(error):
    return jsonify(error=str(error)), 400


@app.errorhandler(Busy)
def busy(_):
    return jsonify(error="A run is already in progress"), 409


@app.post("/api/runs")
def start():
    """Start a run: a new selection, or `rerun` = the name of an earlier run, to do its selection again.
    `redo` lists the steps to do again despite what is in the library."""
    body = _body()
    if body.get("rerun"):
        stored = json.loads((_run_folder(body["rerun"]) / "run.json").read_text(encoding="utf-8"))
        body = stored["selection"] | {"redo": body.get("redo")}
    selection = Selection.parse(
        body.get("venues"), body.get("start"), body.get("end"), body.get("categories"), body.get("include_workshops")
    )
    redo = tuple(body.get("redo") or ())
    job.start(lambda log: pipeline.run(selection, redo, log))
    return jsonify(ok=True)


@app.get("/api/status")
def status():
    return jsonify(job.snapshot(int(request.args.get("since", 0))))


def _run_folder(name: str):
    """The folder of a finished run.  Raises LookupError for anything that is not one."""
    folder = (RUNS / name).resolve()
    # the name must be exactly a folder inside the runs folder: no "..", no path that merely ends up there
    if folder.parent != RUNS.resolve() or folder.name != name or not (folder / "run.json").exists():
        raise LookupError()
    return folder


def _title(folder) -> str:
    """The name a run is shown under: the one it was given, or its folder name.  It is kept in its own file, so
    doing the run again does not lose it."""
    file = folder / "title.txt"
    return file.read_text(encoding="utf-8").strip() if file.exists() else folder.name


@app.get("/api/runs")
def runs():
    found = []
    for path in sorted(RUNS.glob("*/communities.json"), key=os.path.getmtime, reverse=True):
        data = json.loads(path.read_text(encoding="utf-8"))
        found.append(
            {
                "name": path.parent.name,
                "title": _title(path.parent),
                "papers": data["papers"],
                "communities": len(data["communities"]),
                "modularity": data["modularity"],
            }
        )
    return jsonify(found)


@app.get("/api/runs/<name>", defaults={"file": "communities.json"})
@app.get("/api/runs/<name>/graph", defaults={"file": "graph.json"})
def run_file(name, file):
    return app.response_class((_run_folder(name) / file).read_text(encoding="utf-8"), mimetype="application/json")


@app.patch("/api/runs/<name>")
def rename_run(name):
    """Give a run another title.  An empty title goes back to the folder name."""
    folder = _run_folder(name)
    title = " ".join(str(_body().get("title", "")).split())[:120]
    if title and title != folder.name:
        store.write_text(folder / "title.txt", title)
    else:
        (folder / "title.txt").unlink(missing_ok=True)
    return jsonify(name=name, title=_title(folder))


@app.delete("/api/runs/<name>")
def delete_run(name):
    """Remove a run's results.  The library (papers, embeddings, descriptions) is untouched, so running the same
    selection again is quick."""
    shutil.rmtree(_run_folder(name))
    return jsonify(ok=True)


overviews = Overviews()
overview_jobs = {}  # (run, community path) -> Job: at most one overview of a community is written at a time


@app.route("/api/runs/<name>/overview/<path>", methods=["GET", "POST"])
def community_overview(name, path):
    """GET: where the overview of one community stands.  POST: start writing it (a minute or two)."""
    _run_folder(name)
    model = settings.load()["describe_model"]
    ready = overviews.get(name, path, model)
    if ready:
        return jsonify(state=State.DONE.value, overview=ready, error=None)
    writing = overview_jobs.setdefault(
        (name, path), Job()
    )  # not per model: changing it mid-write must not start a second
    if request.method == "POST" and writing.state is not State.RUNNING:
        writing.start(lambda log: overviews.write(name, path, model, log))
    now = writing.snapshot()
    return jsonify(state=now["state"], overview=now["result"], error=now["error"])


class _PaperIndex:
    """The library's papers by id; the file is read again only when it has changed."""

    def __init__(self):
        self.stamp, self.by_id = None, {}

    def get(self, paper_id) -> dict:
        path = LIBRARY / "papers.jsonl"
        stamp = path.stat().st_mtime if path.exists() else None
        if stamp != self.stamp:
            self.stamp, self.by_id = stamp, Papers().by_id
        if paper_id not in self.by_id:
            raise LookupError()
        return self.by_id[paper_id]


papers = _PaperIndex()


@app.get("/api/papers/<path:paper_id>")
def paper(paper_id):
    """A paper's record, plus what is known about it: how it was accepted, whether its PDF can be shown here,
    what its authors declare (null until read), and the AI-text detector's verdict (null if never scored)."""
    found = papers.get(paper_id)
    return jsonify(
        found
        | {
            "acceptance": acceptance(found),
            "readable": proceedings.pdf_url(found) is not None,
            "declarations": proceedings.known(paper_id),
            "reviews": reviews.known(paper_id),
            "ai": detect.Detector(log=lambda line: None).verdict(found),
        }
    )


@app.get("/api/pdf/<path:paper_id>")
def paper_pdf(paper_id):
    """The paper's PDF from where it is published (see proceedings.py), fetched once and kept."""
    return send_file(proceedings.fetch_pdf(papers.get(paper_id), LIBRARY / "pdfs"), mimetype="application/pdf")


@app.post("/api/declarations/<path:paper_id>")
def paper_declarations(paper_id):
    """Read what the authors of one paper declare in it (its checklist and statements), from its PDF."""
    return jsonify(proceedings.declarations(papers.get(paper_id), LIBRARY / "pdfs"))


@app.get("/api/reviews")
def all_ratings():
    """The reviewers' scores of every paper whose reviews page has been opened: {paper id: ratings}."""
    return jsonify(reviews.stored())


@app.post("/api/reviews/<path:paper_id>")
def paper_ratings(paper_id):
    """Read the reviewers' scores from the text of a paper's reviews page, as the reader is showing it."""
    body = _body()
    text, shown = body.get("text"), body.get("url")
    if not isinstance(text, str) or not isinstance(shown, str):
        raise ValueError("`text` must be the page's text and `url` its address")
    return jsonify(reviews.keep(papers.get(paper_id), text, shown))


@app.post("/api/runs/<name>/declarations")
def run_declarations(name):
    """Read the declarations of every paper of a run: a long job (it downloads each paper's PDF), with the same
    progress log as a run."""
    ids = json.loads((_run_folder(name) / "run.json").read_text(encoding="utf-8"))["paper_ids"]
    chosen = [papers.get(paper_id) for paper_id in ids]

    def work(log):
        log(f"reading what the authors of {len(chosen)} papers declare")
        proceedings.read_all(chosen, log)
        return name

    job.start(work)
    return jsonify(ok=True)


@app.get("/api/catalog")
def menu():
    return jsonify(catalog.as_json())


@app.route("/api/settings", methods=["GET", "PUT"])
def user_settings():
    return jsonify(settings.save(_body()) if request.method == "PUT" else settings.load())


@app.get("/api/usage")
def spent():
    """What the Claude API has cost so far, who provides it, and the model that writes names and overviews.
    `?currency=CAD` adds what a US dollar is worth in that currency, so the amounts can be shown in it."""
    model = settings.load()["describe_model"]
    return jsonify(
        usage.summary()
        | {
            "model": {"name": model, "label": claude.MODELS.get(model, (model,))[0]},
            "currency": currency.of(request.args.get("currency")),
        }
    )


@app.get("/api/library")
def library():
    file = LIBRARY / "papers.jsonl"
    count = 0
    if file.exists():
        with open(file, encoding="utf-8") as handle:
            count = sum(1 for line in handle if line.strip())
    models = {
        index.parent.name: len(json.loads(index.read_text())) for index in (LIBRARY / "embeddings").glob("*/index.json")
    }
    return jsonify(papers=count, embeddings=models, ai_validation=store.read_json(detect.REPORT))


if __name__ == "__main__":
    app.run(host="127.0.0.1", port=int(os.environ.get("RP_PORT", 8765)), debug=False, threaded=True)
