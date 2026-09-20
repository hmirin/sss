# sss website

Build from the repository root:

```sh
python3 site/build.py
python3 -m http.server 18765 --directory site/dist
```

Deploy `site/dist/` as static files under `/sss/`, or publish it with `sss sync site/dist --project <id>`. Assets and installation.md use relative links; documentation and the skill link to GitHub. The installation prompt uses the current site URL, including version URLs.

The builder copies the root installation.md verbatim. README and skill links point to their originals on GitHub. No JavaScript packages or remote assets are required.

The public address is `https://combinatrix.ai/sss/`. The company website proxies this path to GitHub Pages.

## Release deployment

Like dlgt, `.github/workflows/pages.yml` builds and deploys GitHub Pages on a
`v*` tag push. It checks that the tagged commit belongs to `main`, then builds
from that exact checkout. The deployed installation.md therefore matches the
release tag; later edits on `main` appear on the next deployment. Ordinary
branch pushes run the site build check without deploying.

The workflow also supports manual dispatch for a website-only update or recovery.
Select `main` for the latest website and documentation, or a release tag to
deploy that version. A manual site deployment does not publish a binary release. Pages must use GitHub Actions as its build
source. Site deployment and binary release publication run independently.
