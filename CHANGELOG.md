# Changelog

What changed in Constellate, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- **"AI detected"** tag: a statistical estimate from word choice (a classifier trained on abstracts and
  model-written mirrors of them), with its measured error rates shown.
- **Show or dim by kind** on the graph: chips for orals, spotlights, posters, AI detected and not, with counts.
- **Filters**: leave out AI-detected papers, keep only orals or orals and spotlights, leave out papers by
  declaration flag.
- **Cost**: the sidebar shows the model and provider in use and what the Claude API has cost.
- **Runs** can be renamed, deleted and redone.
- **Desktop app** (Electron) with a single build script; tests for the backend and the interface.

### Security

- The local API answers only this machine and, for anything that changes state, only the app's own interface.
- Pages shown in the reader run in a sandboxed view with its own storage, no permissions, and navigation
  limited to the sites papers are published on.
- PDFs are fetched only from the publishers' hosts, with a size and time limit.

[Unreleased]: https://github.com/OWNER/constellate/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/OWNER/constellate/releases/tag/v0.1.0
