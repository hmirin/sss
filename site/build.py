"""Build a dependency-free site with documentation from the repository root."""
from pathlib import Path
import shutil
root = Path(__file__).resolve().parent.parent
out = root / "site" / "dist"
out.mkdir(exist_ok=True)
for name in ("index.html", "style.css", "script.js"):
    shutil.copyfile(root / "site" / name, out / name)
for name in ("README.md", "installation.md", "skill.md"):
    shutil.copyfile(root / name, out / name)
print(out)
