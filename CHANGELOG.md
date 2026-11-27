# Changelog

What changed in Constellate, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The runs sidebar, the communities panel, the paper reader, the overview and the activity log can be resized by dragging their edge (or with the arrow keys once the edge is focused); double-click an edge to put it back, and the sizes are remembered.

- `npm run setup` (or `make setup`) gets a fresh clone ready to work on in one command.

- The engine uses any maker's graphics card: through DirectML on Windows and through Vulkan (WebGPU) on Linux, with
  the processor as the fallback. It is built and tested on Linux as well as Windows.

- Mark papers to keep them in view: right-click a paper on the graph (or use the star in the reader and in a
  community's list) and it gets a yellow ring with its closest papers always linked and named; the Marked tab lists
  a run's marked papers, and marks are remembered for each run. While one community is in focus, only the marked
  papers inside it are shown.
- The engine has the model prices and the exchange rates built in, with the day each rate is from: once it
  replaces the backend, the app no longer asks the internet for a rate. A weekly check compares the rates with the
  European Central Bank's and, when one has moved by more than 1%, prepares a new version with the new rates.
- The engine keeps the library in one small database on your computer instead of many loose files, so an
  interrupted write can no longer leave it half-saved, and an existing library is moved in without losing anything.
  A conference whose site did not answer is now fetched again later instead of being counted as downloaded, and
  papers whose PDF could not be had or read are remembered as such.

### Changed

- The redo buttons say what they cover ("Embed all again"), a fourth redoes all of it, and every run in the sidebar
  has its own button to redo all of that run.
- The installed app keeps its library in the system's folder for local, unsynced data (the local application-data
  folder on Windows, `~/.local/share` on Linux): a library in a synced folder can be corrupted while the app is open.
- The backend is being rewritten in Rust (`apps/engine`), organised by subject; the Python backend stays until the
  Rust one does everything it does.

### Security

- The new engine answers only the app itself: every request must carry a secret token and be addressed to the
  engine, and pages of other sites are refused. Run on another machine than the app, it requires HTTPS and a token
  of its own, and warns loudly if HTTPS is turned off.

## [0.1.0] - 2026-11-26

The first release.

### Added

- **Sources**: accepted papers from the conferences' own lists (NeurIPS, ICML, ICLR, CVPR, ICCV, ECCV, WACV,
  AISTATS, MLSys), other conferences through arXiv comments, and arXiv by date range and category, chosen from
  grouped multi-select menus.
- **Library**: everything downloaded, embedded, described or read is kept by id and never done twice; each step
  can be redone from Settings.
- **Embeddings** with a choice of models (Qwen3-Embedding-0.6B by default), on the GPU when there is one.
- **Communities**: similarity of all papers as max(0, cosine), clustered by spectral clustering, k-nearest
  spectral clustering, k-means or recursive Fiedler bisection, with sub-communities found by further splits.
  A comparison tool (`python -m rp.compare`) measures the methods against each other.
- **Names and descriptions** of communities written by Claude from each community's mean embedding, nearest
  papers and distinguishing terms; a choice of models.
- **Community overviews**: common methods, promising findings and patterns, each point citing its papers.
- **The graph**: one canvas, a glow colour per community, zooming in moves from communities to
  sub-communities, links appear on hover along with the linked papers' titles and tags.
- **The reader**, inside the window: Abstract, the paper's PDF, and its OpenReview page.
- **Acceptance** tiers (oral, spotlight, poster) from the conference lists, shown on papers and communities.
- **What authors declare**: the NeurIPS checklist and the limitations, impact, ethics, reproducibility and
  LLM-usage statements of other venues, read from the PDF, with flags derived from them.
- **Reviewers' ratings**, read from a paper's reviews page when it is opened in the reader.
- **Works without a graphics card**: Settings says whether one was found and what that means for speed, and the
  AI-text check can be switched off; Settings says whether it is on, off or not yet trained, and why.
- **"AI detected"** tag: a statistical estimate from word choice (a classifier trained on abstracts and
  model-written mirrors of them), with its measured error rates shown.
- **Show or dim by kind** on the graph: chips for orals, spotlights, posters, AI detected and not, with counts.
- **Filters**: leave out AI-detected papers, keep only orals or orals and spotlights, leave out papers by
  declaration flag.
- **Cost**: the sidebar shows the model (a menu to change it) and provider in use and what the Claude API has
  cost, in the currency of your region at the European Central Bank's rate.
- **Every number says what it is**: "0.45 cosine similarity", "523 papers", "CAD 2.13", "38k tokens".
- **Runs** can be renamed, deleted and redone.
- **Desktop app** (Electron) with a single build script; tests for the backend and the interface.
- **Windows installer**, built for every release. It needs nothing preinstalled: the first start sets up the
  app's own Python and packages, with the GPU build of PyTorch on machines with an NVIDIA card. The
  uninstaller offers to remove everything the app downloaded and computed as well.

### Security

- The local API answers only this machine and, for anything that changes state, only the app's own interface.
- Pages shown in the reader run in a sandboxed view with its own storage, no permissions, and navigation
  limited to the sites papers are published on.
- PDFs are fetched only from the publishers' hosts, with a size and time limit.

[Unreleased]: https://github.com/Patchoulis/Constellate/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Patchoulis/Constellate/releases/tag/v0.1.0
