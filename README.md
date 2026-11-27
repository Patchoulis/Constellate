# Constellate

Take a set of papers (a conference's accepted papers, or everything submitted to arXiv in a date range), embed
them, split them into communities with Fiedler vectors, and explore the result as a field of glowing clusters you
can zoom into and read papers from.

```
apps/backend        Python: paper sources, embeddings, communities, descriptions, HTTP API (Flask)
apps/web            Next.js + Tailwind + d3: the interface and the single-canvas graph
apps/desktop        Electron: the window around the two, as a desktop app
packages/contracts  TypeScript types of the API, shared by the interface
scripts/build.mjs   build, run and package
data/               everything stored locally (not in git)
```

## Install

**Windows:** download `Constellate-Setup-x.y.z.exe` from the [latest release](../../releases/latest) and run it.
There is nothing else to install.

- The installer is not code-signed, so Windows SmartScreen may say it does not recognise it: choose _More info_,
  then _Run anyway_. Each release also carries the installer's SHA-256 checksum.
- The first start sets up the app's own Python and packages (shown in a small window). That is a download of a
  few gigabytes and takes several minutes; it happens once. With an NVIDIA graphics card the GPU build of PyTorch
  is installed and embedding is fast; without one everything still works, slowly.
- The embedding model (about 1.2 GB) is downloaded the first time papers are embedded.
- Names, descriptions and overviews of communities are written by Claude and need an API key in the
  `ANTHROPIC_API_KEY` environment variable. Without one, everything else works and communities are named by
  their distinguishing terms. To set it: Start, "Edit environment variables for your account", New.

**Uninstall:** Windows Settings, Apps, Installed apps, Constellate, Uninstall. The uninstaller asks whether to also
remove what the app downloaded and computed, all of which is in `%APPDATA%\\Constellate`: its own Python and
packages (several GB) and your library of papers, embeddings and runs. Answer No to keep them for a later
reinstall. The embedding model sits in the shared Hugging Face cache (`%USERPROFILE%\\.cache\\huggingface`),
which other programs may use too, so it is left alone: delete that folder by hand if nothing else needs it.

**What it connects to:** the conferences' sites and arXiv (papers), Hugging Face and PyPI (the model and packages,
once), the Claude API (names and overviews, if you set a key), OpenReview (only pages you open in the reader), and
frankfurter.dev for the day's exchange rate when your region's currency is not the US dollar. Nothing about you
or your library is sent anywhere.

**From source** (developers; any platform). You need Node 22 or newer and git; one command gets everything else
(Rust through rustup, the formatter and linter, the npm packages, the git hooks):

```
git clone https://github.com/Patchoulis/Constellate && cd Constellate
npm run setup
npm run app
```

`make setup`, `make run`, `make pr` do the same for people who reach for make; `make` alone lists every command.
On Linux the engine needs glibc 2.38 or newer for now (Ubuntu 24.04, Debian 13, Fedora 39 and later), and a C
compiler (`build-essential` or your distribution's equivalent). It uses any graphics card through Vulkan.
While the backend is being moved from Python to Rust, running the app from source also needs Python 3.12 or
newer with `pip install -r apps/backend/requirements.txt`.

## Build and run

```
npm install
npm run setup --workspace @rp/backend      # Python packages; install torch (CUDA) yourself first
```

| Command                                                   | What it does                                                                                                                                                 |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `npm run app`                                             | Build, then open the desktop app. An unchanged tree rebuilds in about a second (Turborepo cache).                                                            |
| `npm run build`                                           | Build only.                                                                                                                                                  |
| `npm run dist`                                            | Build, then make the Windows installer in `dist/`. The installed app still needs Python with the backend's packages.                                         |
| `npm run compare --workspace @rp/backend -- <run>`        | Score k-means and spectral clusterings of a finished run against the authors' topics, with stability; writes `clustering_comparison.md` in the run's folder. |
| `npm test`                                                | Run the tests: the backend's (pytest) and the interface's (Node's test runner).                                                                              |
| `npm run dev`                                             | Development servers with hot reload: the interface on http://localhost:3210, the API on :8765.                                                               |
| `npm run pipeline -- --venues "NeurIPS 2025" "ICML 2026"` | Run the pipeline without any interface. Also `--start 2026-11-01 --end 2026-11-07 --categories cs.LG`, and `--redo embeddings`.                              |

## What a run does

Seven stages (`apps/backend/rp/pipeline.py`), each a function over one shared record:

1. **Download.** Conferences come from the conference's own site (NeurIPS, ICML, ICLR, CVPR, ICCV, ECCV, WACV,
   AISTATS and MLSys publish every accepted paper with its abstract). A date range comes from the arXiv API, one
   query per day, optionally restricted to categories. A conference without an official list falls back to arXiv
   papers whose comment names it, which is a partial sample.
2. **Embed** title + abstract with the chosen embedding model (default `Qwen/Qwen3-Embedding-0.6B`, 1024
   dimensions, on the GPU when there is one).
3. **Detect model-written abstracts.** Each abstract is scored for wording of the kind language models produce,
   and papers above a threshold are tagged "AI detected". See below for what that does and does not mean.
4. **Adjacency** `A[i, j] = max(0, cosine similarity)`, zero diagonal. By default the mean embedding is removed
   first; otherwise every pair of ML papers is similar and the clip at 0 does nothing.
5. **Communities**, by the chosen method (default: spectral clustering). Each community is then split again into
   sub-communities with the same method, a set number of levels deep; a split is kept if both halves are big
   enough and the cut's modularity inside the group reaches the threshold.
6. **Describe.** The embeddings of each (sub-)community are averaged. That mean embedding is turned back into
   words three ways: the terms that set the group apart, the papers nearest to the mean, and, with
   `ANTHROPIC_API_KEY` set for the backend, a name and description written by Claude from those papers.
   Similarity figures come with it: members to their mean embedding, pairwise inside, and between communities.
7. **Write** `data/runs/<name>/`: `report.md`, `communities.json`, `graph.json`, `adjacency.npy`,
   `centroids.npy`, `run.json`.

## Choosing the clustering method

Four methods are offered (Settings, or `clustering` in `data/settings.json`): spectral, spectral on a
nearest-neighbour graph, k-means, and recursive Fiedler bisection. `rp/compare.py` scores them on a run: agreement
with the topics authors filed their papers under, coherence of each community's wording (text only, so it favours
no method), stability under random starts and under removing a fifth of the papers, over several numbers of
communities. On NeurIPS 2025 + ICML 2026 the three one-shot methods were close on agreement and coherence;
spectral was the most stable and k-means the least; recursive Fiedler bisection was last on agreement at every
size. The table is in the run's `clustering_comparison.md`. It is one dataset: run it on yours.

## Community overviews

A community's detail panel has an Overview button. Claude reads every paper of the community plus what the graph
says about it (its sub-communities, its central and most-linked papers, where its links to other communities go)
and writes labelled sections: common methods, recurring findings, promising directions, patterns in the
structure, open questions. Repetition is the test: every point must cite at least three papers that share it,
citations are checked against the community's own papers, and a point left with too few is dropped. Each cited
paper opens on a click. An overview is written once and kept.

## "AI detected"

There is no watermark to look up: watermarks can only be checked with the generating model's secret key. The tag
comes from a classifier built the way Pangram Labs describe theirs (arXiv:2402.14873), at a far smaller scale:
human abstracts from 2019 are paired with "mirrors" a model wrote from the same title or polished from the same
abstract, so only the wording differs; then the human abstracts the classifier gets most wrong are mined, given
mirrors, and added. It reads a transformer's vector of the abstract and its word frequencies.

`python -m rp.detect` (in `apps/backend`) builds it and prints what was measured on papers it never saw: the
share of machine abstracts caught at a threshold that flags 1% of human ones. Settings shows the same numbers.
It knows the writing of the models it was trained against only, and it cannot be checked on present-day human
text. A tag means "written the way these models write". It is not evidence about any one author.

## Acceptance, declarations and the reader

- **Acceptance** (oral / spotlight / poster) comes from each conference's own list and is shown on the paper and counted per community.
- **What the authors declare** is read from the paper's PDF in the conference's proceedings (NeurIPS, ICML via PMLR, CVPR / ICCV / WACV via CVF; arXiv papers from arXiv): the NeurIPS checklist with its answers, and the limitations, impact, ethics, reproducibility and LLM-usage statements other conferences ask for. Flags derived from them: LLM use declared, code not released, no limitations discussed, human subjects. Read one paper from its reader, or all of a run's from Settings ("Read them all": one PDF per paper, kept locally).
- **Chips above the graph** show or dim papers by kind (orals, spotlights, posters, AI detected, not AI detected) without redoing the run.
- **Filters** (Settings, "Which papers"): leave out AI-detected papers, keep only orals or orals + spotlights, leave out papers carrying chosen flags. A flag filter only acts on papers whose PDF has been read.
- **The reader** opens inside the window with tabs: Abstract, Paper (the PDF), and Reviews (the paper's OpenReview page, desktop app only).
- **Reviewers' ratings** (Rating, Quality, Clarity, Significance, Originality, Soundness, ...) are read from the Reviews page when you open it, since OpenReview does not allow programs to fetch reviews; they are kept and then shown on the paper and in the graph's hover labels. Scores are shown per field on the conference's own scale, never merged.
- **Tags explain themselves**: hover Oral / Spotlight, AI detected, a declaration flag or a rating to see what it means and where it came from.
- **Cost**: the bottom of the sidebar shows the model and provider in use and what the Claude API has cost (total, and since the app was started); hover it for the breakdown by purpose and model. Counting starts when this was added.

## The graph

Papers are dots. What shows that papers belong together is the glow: every paper adds a faint halo in its
group's colour and the halos add up. Zooming in switches from communities to their sub-communities, each with its
own glow and title.

Links show what a paper is closest to. Each paper is linked only to its few most similar papers: among its best
matches, the list is cut at the largest drop in similarity (at most 8). Links are only drawn for the paper under
the pointer; the reader lists an open paper's links as "Closest papers".

Clicking a community in the list focuses it; clicking a paper opens it.

## Nothing is done twice

`data/library/` is shared by every run and keyed by paper id:

- a download (one conference, one arXiv day) that finished is not fetched again; a known paper is not stored again;
- an embedding is reused when the same model already embedded that id with the same title and abstract
  (text hash), so only changed or new papers are embedded;
- a description is reused when the same set of papers was already described by the same model;
- an arXiv PDF opened in the reader is fetched once and served from disk after that.

Settings (the gear) has a button per step that runs the open run again with that step done from scratch; on the
command line that is `--redo download embeddings descriptions`.
