# CAVS Landing Page

A self-contained, single-file landing page for CAVS, built to give the project
visibility and surface its real benchmarks. No build step, no dependencies — just
static HTML/CSS/JS.

## Files

```
landing/
├── index.html      # the whole page (inline CSS + JS)
├── .nojekyll       # tell GitHub Pages to serve assets/ verbatim
├── assets/
│   ├── logo.png        # nav / footer logo + favicon
│   ├── logo-large.png  # transparent large logo (spare)
│   └── og-thumb.webp   # Open Graph / social preview image
└── README.md
```

## Preview locally

Open `index.html` directly in a browser, or serve it:

```bash
cd landing
python3 -m http.server 8080
# open http://localhost:8080
```

## Publish on GitHub Pages

**Option A — `/docs` on the default branch (simplest)**

1. Rename or copy this folder to `docs/` at the repo root.
2. Repo → Settings → Pages → Source: *Deploy from a branch* → branch `main`, folder `/docs`.
3. Save. The page goes live at `https://orelvis15.github.io/cavs-oss/`.

**Option B — GitHub Actions from `landing/`**

Add `.github/workflows/pages.yml`:

```yaml
name: Deploy landing to Pages
on:
  push:
    branches: [main]
    paths: ["landing/**"]
permissions:
  pages: write
  id-token: write
concurrency:
  group: pages
  cancel-in-progress: true
jobs:
  deploy:
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deploy.outputs.page_url }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/upload-pages-artifact@v3
        with:
          path: landing
      - id: deploy
        uses: actions/deploy-pages@v4
```

Then set Settings → Pages → Source: *GitHub Actions*.

## Editing

Everything lives in `index.html`. The design tokens (colors, fonts, radii) are
CSS variables in `:root` at the top of the `<style>` block. Benchmark numbers are
plain HTML tables — edit them in place when new releases land.

All figures on the page come from the measured benchmarks in `docs/BENCHMARKS.md`
and the project changelog.
