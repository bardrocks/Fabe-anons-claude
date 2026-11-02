"""Downloads the ema-lightning model files into the given folder (run at build time only).

The app never downloads anything: these files ship inside the installer.
"""
import shutil
import sys
from pathlib import Path

from huggingface_hub import HfApi, hf_hub_download

REPO = "canberkkkkkk/ema-lightning"
FILES = ("ema.pt", "decoder.pt", "config.json")


def main(dest):
    dest = Path(dest)
    dest.mkdir(parents=True, exist_ok=True)
    sha = HfApi().model_info(REPO).sha
    print(f"{REPO} @ {sha}")
    for name in FILES:
        path = hf_hub_download(REPO, name, revision=sha)
        shutil.copyfile(path, dest / name)
        print(f"  {name}: {(dest / name).stat().st_size:,} bayt")
    (dest / "SURUM.txt").write_text(f"{REPO}\n{sha}\n", encoding="utf-8")


if __name__ == "__main__":
    main(sys.argv[1])
