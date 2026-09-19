# sss website

Build from the repository root:

```sh
python3 site/build.py
python3 -m http.server 18765 --directory site/dist
```

Deploy `site/dist/` as static files under `/sss/`, or publish it with `sss sync site/dist --project <id>`. All asset and document links are relative. The installation prompt uses the current site URL, including version URLs.

The builder copies the root README, installation instructions, and embedded skill verbatim. Edit those originals when changing the product contract. No JavaScript packages or remote assets are required.

The intended public address is `https://combinatrix.ai/sss/`. DNS/routing and public release publication are separate from this build. Until releases are available, installation.md describes the source-build fallback and repository access requirements.
