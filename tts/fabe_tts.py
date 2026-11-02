"""FABE Anons canlı anons motoru (ema-lightning).

The app starts this program once and keeps it running, so the model is loaded only once.
It talks over stdin/stdout, one JSON object per line:

    -> {"id": 1, "text": "Ahmet Bey kasaya", "speed": 1.0, "gain": 6.0, "out": "C:/.../x.wav"}
    <- {"id": 1, "ok": true, "seconds": 1.8, "ms": 240}
    <- {"id": 1, "ok": false, "error": "..."}

The first line it writes is {"ready": true, "ms": ...} once the model is loaded and warmed up,
or {"ready": false, "error": "..."} if it cannot start. Nothing else is written to stdout.
"""
import json
import os
import sys
import time
import traceback
from pathlib import Path

# Never reach the network: the models ship with the app.
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
os.environ["HF_HUB_DISABLE_SYMLINKS_WARNING"] = "1"

# Turkish text must survive the pipe whatever the Windows code page is.
for stream in (sys.stdin, sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
OUT = sys.stdout
sys.stdout = sys.stderr  # library prints must not break the protocol


def send(obj):
    OUT.write(json.dumps(obj, ensure_ascii=False) + "\n")
    OUT.flush()


def models_dir():
    if len(sys.argv) > 1:
        return Path(sys.argv[1])
    here = Path(getattr(sys, "_MEIPASS", Path(__file__).parent))
    for cand in (here / "models", here.parent / "models", here.parent.parent / "models"):
        if (cand / "ema.pt").is_file():
            return cand
    return here / "models"


def load():
    import torch
    from ema_lightning.api import EMA, Frontend, load_acoustic, load_decoder

    torch.set_num_threads(max(1, min(4, os.cpu_count() or 1)))
    device = torch.device("cpu")
    d = models_dir()
    ema_pt, dec_pt = d / "ema.pt", d / "decoder.pt"
    if not ema_pt.is_file() or not dec_pt.is_file():
        raise FileNotFoundError(f"Model dosyaları bulunamadı: {d}")
    model = load_acoustic(str(ema_pt), device)
    decoder = load_decoder(str(dec_pt), device)
    return EMA._from_parts(model, decoder, Frontend(model.vocab), device)


def speak(ema, req):
    import numpy as np
    from ema_lightning.audio import write_wav

    text = str(req.get("text", "")).strip()
    if not text:
        raise ValueError("Metin boş")
    # Text typed with Caps Lock would otherwise be spelled letter by letter.
    text = text.replace("I", "ı").replace("İ", "i").lower()
    speed = float(req.get("speed", 1.0))
    gain = float(req.get("gain", 1.0))
    speech = ema.say(text, speed=min(2.0, max(0.5, speed)))
    if speech.audio is None or len(speech.audio) == 0:
        raise ValueError("Metin okunamadı")
    audio = np.clip(speech.audio * gain, -1.0, 1.0)
    out = Path(req["out"])
    out.parent.mkdir(parents=True, exist_ok=True)
    write_wav(str(out), audio, speech.sample_rate)
    return speech.duration


def main():
    start = time.perf_counter()
    try:
        ema = load()
        ema.say("hazır")  # warm-up so the first real announcement is fast
    except Exception as e:
        traceback.print_exc()
        send({"ready": False, "error": f"{type(e).__name__}: {e}"})
        return 1
    send({"ready": True, "ms": round((time.perf_counter() - start) * 1000)})

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        req = {}
        try:
            req = json.loads(line)
            if req.get("quit"):
                break
            t = time.perf_counter()
            seconds = speak(ema, req)
            send({"id": req.get("id"), "ok": True, "seconds": round(seconds, 2),
                  "ms": round((time.perf_counter() - t) * 1000)})
        except Exception as e:
            traceback.print_exc()
            send({"id": req.get("id"), "ok": False, "error": f"{type(e).__name__}: {e}"})
    return 0


if __name__ == "__main__":
    sys.exit(main())
