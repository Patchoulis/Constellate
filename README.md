# Constellate

Take a set of papers (a conference's accepted papers, or everything submitted to arXiv in a date range), embed
them, split them into communities with Fiedler vectors, and explore the result as a field of glowing clusters you
can zoom into and read papers from. Everything is computed on your own computer.

```
apps/engine         Rust: paper sources, the library, embeddings, communities, descriptions, the HTTP API
apps/web            Next.js + Tailwind + d3: the interface and the single-canvas graph
apps/desktop        Electron: the window around the two, as a desktop app
packages/contracts  TypeScript types of the API, shared by the interface
scripts/build.mjs   build, run and package
data/               everything stored locally when run from source (not in git)
```

## Install

**Windows** (10 or 11, 64-bit): download `Constellate-Setup-x.y.z.exe` from the [latest release](../../releases/latest)
and run it. There is nothing else to install, and the app opens at once.

- The installer is not code-signed, so Windows SmartScreen may say it does not recognise it: choose _More info_,
  then _Run anyway_. Each release carries the SHA-256 checksum of every file (`SHA256SUMS`, and one `.sha256` per
  file).
- The embedding model runs on any graphics card through DirectML (DirectX 12), and on the processor without one.

**Linux** (64-bit, glibc 2.38 or newer: Ubuntu 24.04, Debian 13, Fedora 39 or later): download either

- `Constellate-x.y.z-x86_64.AppImage`, make it executable (`chmod +x`) and run it; or
- `Constellate-x.y.z-amd64.deb` (Debian, Ubuntu) and install it: `sudo apt install ./Constellate-x.y.z-amd64.deb`.

On an older system the app says that its glibc is too old and closes. The embedding model runs on any graphics card
through Vulkan (WebGPU), and on the processor without one.

**The first run** you make downloads the embedding model, `Qwen3-Embedding-0.6B` (about 1.2 GB), into the data
folder, once, as one of the run's steps: its progress is shown in megabytes. A model already in the Hugging Face
cache on this computer is used from there instead.

### Where your data is

Your library of papers, embeddings, PDFs and runs, the embedding model and the engine's log (`logs/engine.log`):

| System  | Folder                                                                                 |
| ------- | -------------------------------------------------------------------------------------- |
| Windows | `%LOCALAPPDATA%\Constellate\data`                                                      |
| Linux   | `~/.local/share/constellate/data` (`$XDG_DATA_HOME/constellate/data` when that is set) |

It is a folder nothing syncs: the library is a database that is open while the app runs. A library and runs made by
an earlier version of the app are in the same folder and are found and kept. To use another folder, set
`CONSTELLATE_DATA` to it before starting the app.

### Keys

Names, descriptions and overviews of communities are written by a language model, which needs its provider's API
key: `ANTHROPIC_API_KEY` for Claude, the default; also OpenAI (`OPENAI_API_KEY`), Google Gemini
(`GEMINI_API_KEY`), a dozen other hosted providers, and any OpenAI-compatible server such as Ollama or LM Studio on
your own computer (`CONSTELLATE_MODEL_BASE_URL`), chosen in the sidebar. Without a key, everything else works and
communities are named by their distinguishing terms.

**Keys go in one file: `.env` in the data folder above**, one `NAME=value` per line, then close the app and open
it again:

```
ANTHROPIC_API_KEY=...
```

`.env.example` in this repository lists every name. A variable already set in your environment is used too, and
wins over the file. The key is read by the engine alone, never shown (Settings says only whether it is set), never
written to a log, and never given to the interface or any page. Keep the file to yourself: it is a secret.

### Update

Download the newer release and install it the same way, over the one you have. Nothing needs to be uninstalled
first, and your library, runs, settings and the embedding model are kept.

- **Windows:** run the new `Constellate-Setup-x.y.z.exe`. It closes Constellate if it is open, replaces the program
  and leaves `%LOCALAPPDATA%\Constellate` as it is. Open Constellate again from the Start menu or the desktop.
- **Linux:** `sudo apt install ./Constellate-x.y.z-amd64.deb` replaces the installed version; an AppImage is
  replaced by the new file. `~/.local/share/constellate` is not touched.

What a newer version needs changed in your library it changes itself the first time it opens.

### Uninstall

- **Windows:** Settings, Apps, Installed apps, Constellate, Uninstall. The uninstaller asks whether to also remove
  what the app downloaded and computed, all of which is in `%LOCALAPPDATA%\Constellate`: the embedding model and
  your library of papers, embeddings and runs (and your `.env`). Answer No to keep them for a later reinstall.
- **Linux:** delete the AppImage, or `sudo apt remove constellate`. Your data stays in
  `~/.local/share/constellate`; delete that folder to remove it too.

### What it connects to

The conferences' sites and arXiv (papers and PDFs), Hugging Face (the embedding model, once), the language model's
provider you chose (names and overviews, if you set its key; nothing leaves your computer for a model you run on
it), OpenReview (only pages you open in the reader), and frankfurter.dev for the day's exchange rate when your
region's currency is not the US dollar. Citation networks use no citation service: they are built from the
reference lists of the PDFs Constellate reads, and a cited work not in your library is looked up on arXiv only.
Nothing about you or your library is sent anywhere.

## Build from source

Any platform; you need Node 22 or newer, git and Rust (stable). One command gets everything else (Rust through
rustup if it is missing, the formatter and linter, the npm packages, the git hooks):

```
git clone https://github.com/Patchoulis/Constellate && cd Constellate
npm run setup
npm run app
```

On Windows, Rust needs Microsoft's C++ build tools (`npm run setup` says how to get them). On Linux the engine needs
glibc 2.38 or newer and a C compiler (`build-essential` or your distribution's equivalent). `make setup`,
`make run`, `make pr` do the same for people who reach for make; `make` alone lists every command.

| Command         | What it does                                                                                                                |
| --------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `npm run app`   | Build, then open the desktop app. An unchanged tree rebuilds in seconds (Turborepo's cache, Cargo's).                       |
| `npm run build` | Build only: the interface (Next standalone output) and the engine (`cargo build --release -p constellate`).                 |
| `npm run dist`  | Build, then make this system's packages in `dist/`: the Windows installer on Windows, the AppImage and the `.deb` on Linux. |
| `npm run dev`   | The engine and the interface with hot reload, on http://localhost:3210 (`scripts/dev.mjs`).                                 |
| `npm run pr`    | Everything a pull request must pass: formatting, lint, types and every test.                                                |

Run from the repository, the app keeps its data in `data/` (or `CONSTELLATE_DATA`) and reads keys from `data/.env`.
How the engine is started, reached and configured is in [`apps/engine/README.md`](apps/engine/README.md).

### Where things are in the interface

`apps/web` has one folder per subject, and the same folders under `components/` (what is drawn), `lib/` (its
logic, plain functions where it can be) and `tests/`, so that a subject's view, logic and tests are found by one name.
The engine's own layout is in [`apps/engine/README.md`](apps/engine/README.md) ("Where things are").

```text
apps/web/
  app/                       Next's routes: the one page, its layout and styles
  proxy.ts                   the interface's server: forwards /api and /auth to the engine, the page's policy
  components/                brand (the app's mark, a paper's tags), math-text (titles and abstracts with LaTeX)
    features/                one folder per model feature, named as the engine names it:
      answers/ citation_contexts/ claim_tree/ critique/ detector/ explanation/ extraction/ stances/
    graph/                   the graph view and its canvas
    reader/                  the paper reader, its Text tab, citation network, evidence links
    community/               the community panel, overview, reviews taken together
    shell/                   the page's frame: sidebar, run dialogs, settings, model choice, workspace, confirmation
    ui/                      buttons, fields, menus, the resize handle
  lib/                       what everything uses: costs, units, the chosen model, fetching, acceptance tags
    features/                the same feature folders, and writing/ (naming communities)
    graph/                   layout, labels, glow layer, pick, glide, colouring, the paper filter
    reader/                  evidence, citations, the PDF frame
    community/               the community tree, marks
    shell/                   models, notices, the runs list, making a run again, settings saves, following jobs
    net/                     requests to the engine, the engine's mode, addresses, the page's policy
    ui/                      panel sizes, focus return
  tests/                     the same folders as lib/; shared helpers and tests of the root's files at the root
```

**Adding a feature** is one folder of the engine's name for it under each of `components/features/`,
`lib/features/` and `tests/features/`: its component(s), its logic and its tests. `npm test` finds a test file
in any folder under `tests/`.

## What a run does

The steps of a run (`apps/engine/src/pipeline`), each shown with its progress while the run is made:

1. **Gather** the papers. Conferences come from the conference's own site (NeurIPS, ICML, ICLR, CVPR, ICCV, ECCV,
   WACV, AISTATS and MLSys publish every accepted paper with its abstract). A date range comes from the arXiv API,
   one query per day, optionally restricted to categories. A conference without an official list falls back to
   arXiv papers whose comment names it, which is a partial sample.
2. **The embedding model**: its files, downloaded once and checked (the first run only).
3. **Embed** title + abstract with the embedding model (`Qwen/Qwen3-Embedding-0.6B`, 1024 dimensions, on the
   graphics card when there is one).
4. **Detect model-written abstracts.** Each abstract is scored for wording of the kind language models produce,
   and papers above a threshold are tagged "AI detected". See below for what that does and does not mean.
5. **Filter** what the settings leave out.
6. **Similarity** `A[i, j] = max(0, cosine similarity)`, zero diagonal. By default the mean embedding is removed
   first; otherwise every pair of ML papers is similar and the clip at 0 does nothing.
7. **Communities**, by the chosen method (default: spectral clustering). Each community is then split again into
   sub-communities with the same method, a set number of levels deep; a split is kept if both halves are big
   enough and the cut's modularity inside the group reaches the threshold.
8. **Summarise.** The embeddings of each (sub-)community are averaged. That mean embedding is turned back into
   words: the terms that set the group apart, and the papers nearest to the mean. Similarity figures come with it:
   members to their mean embedding, pairwise inside, and between communities.
9. **Describe**, with a language model's key set: a name and description written by the model from those papers,
   within the most you accepted for it.
10. **Write** `runs/<name>/` in the data folder: `report.md`, `communities.json`, `graph.json`, `centroids.npy`,
    `run.json`.

## Choosing the clustering method

Four methods are offered (Settings, or `clustering` in the data folder's `settings.json`): spectral, spectral on a
nearest-neighbour graph, k-means, and recursive Fiedler bisection. A developer's tool scores them on a run
(`cargo run --release -p constellate --example compare -- <run>`): agreement with the topics authors filed their
papers under, coherence of each community's wording (text only, so it favours no method), stability under random
starts and under removing a fifth of the papers, over several numbers of communities. On NeurIPS 2025 + ICML 2026
the three one-shot methods were close on agreement and coherence; spectral was the most stable and k-means the
least; recursive Fiedler bisection was last on agreement at every size. The table is written to the run's
`clustering_comparison.md`. It is one dataset: run it on yours.

## Community overviews

A community's detail panel has an Overview button. The language model reads every paper of the community plus what the graph
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
mirrors, and added. It reads the embedding model's vector of the abstract and its word frequencies.

Settings builds it (it asks the language model for the mirrors, within a budget it shows first) and shows what was
measured on papers it never saw: the share of machine abstracts caught at a threshold that flags 1% of human ones.
It knows the writing of the models it was trained against only, and it cannot be checked on present-day human
text. A tag means "written the way these models write". It is not evidence about any one author.

## Acceptance, declarations and the reader

- **Acceptance** (oral / spotlight / poster) comes from each conference's own list and is shown on the paper and counted per community.
- **What the authors declare** is read from the paper's PDF in the conference's proceedings (NeurIPS, ICML via PMLR, CVPR / ICCV / WACV via CVF; arXiv papers from arXiv): the NeurIPS checklist with its answers, and the limitations, impact, ethics, reproducibility and LLM-usage statements other conferences ask for. Flags derived from them: LLM use declared, code not released, no limitations discussed, human subjects. Read one paper from its reader, or all of a run's from Settings ("Read them all": one PDF per paper, kept locally).
- **Chips above the graph** show or dim papers by kind (orals, spotlights, posters, AI detected, not AI detected) without redoing the run.
- **Filters** (Settings, "Which papers"): leave out AI-detected papers, keep only orals or orals + spotlights, leave out papers carrying chosen flags. A flag filter only acts on papers whose PDF has been read.
- **The reader** opens inside the window with tabs: Abstract, Paper (the PDF), and Reviews (the paper's OpenReview page, desktop app only).
- **Tags explain themselves**: hover Oral / Spotlight, AI detected, a declaration flag or a rating to see what it means and where it came from.
- **Cost**: the bottom of the sidebar shows the model and provider in use (a menu to choose another, grouped by provider, with the effort where the model has one), its price, and what the language model has cost (total, and since the app was started); hover it for the breakdown by purpose and model. A model whose price is not known says "cost not known", never 0; a model on your own computer says it has no API cost. Counting starts when this was added.

## The graph

Papers are dots. What shows that papers belong together is the glow: every paper adds a faint halo in its
group's colour and the halos add up. Zooming in switches from communities to their sub-communities, each with its
own glow and title.

Links show what a paper is closest to. Each paper is linked only to its few most similar papers: among its best
matches, the list is cut at the largest drop in similarity (at most 8). Links are only drawn for the paper under
the pointer; the reader lists an open paper's links as "Closest papers".

Clicking a community in the list focuses it; clicking a paper opens it.

## Nothing is done twice

The library in the data folder is shared by every run and keyed by paper id:

- a download (one conference, one arXiv day) that finished is not fetched again; a known paper is not stored again;
- an embedding is reused when the same model already embedded that id with the same title and abstract
  (text hash), so only changed or new papers are embedded;
- a description is reused when the same set of papers was already described by the same model;
- a PDF opened in the reader is fetched once and served from disk after that.

Settings (the gear) has a button per step that runs the open run again with that step done from scratch.
