#!/usr/bin/env python3
"""Embeds docs/demo/transcript.json into docs/index.html (works from file:// and GitHub Pages)."""
import json, pathlib
here = pathlib.Path(__file__).parent
t = json.loads((here / "demo" / "transcript.json").read_text())
html = (here / "index.template.html").read_text().replace("/*TRANSCRIPT*/null", json.dumps(t))
(here / "index.html").write_text(html)
print("wrote", here / "index.html")
