# Changelog

What changed in Constellate, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `npm run setup` (or `make setup`) gets a fresh clone ready to work on in one command.

- The engine uses any maker's graphics card: through DirectML on Windows and through Vulkan (WebGPU) on Linux, with
  the processor as the fallback. It is built and tested on Linux as well as Windows.

### Changed

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
