"""Build-time check: starts the built Edge engine the way the app does and reads one sentence per voice."""
import json
import subprocess
import sys
import tempfile
import threading
from pathlib import Path


def main(exe):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr,
                         text=True, encoding="utf-8")
    timer = threading.Timer(120, p.kill)
    timer.start()
    try:
        ready = json.loads(p.stdout.readline())
        print("açılış:", ready)
        assert ready.get("ready"), ready
        for n, voice in enumerate(["tr-TR-EmelNeural", "tr-TR-AhmetNeural"], 1):
            out = Path(tempfile.gettempdir()) / f"fabe-edge-deneme-{n}.mp3"
            out.unlink(missing_ok=True)
            p.stdin.write(json.dumps({"id": n, "text": "Sayın personelimiz Ahmet Bey, lütfen kasaya gelin.",
                                      "voice": voice, "speed": 0.9, "out": str(out)}, ensure_ascii=False) + "\n")
            p.stdin.flush()
            reply = json.loads(p.stdout.readline())
            print(voice, "yanıt:", reply)
            assert reply.get("ok") and reply.get("id") == n, reply
            size = out.stat().st_size
            print(f"  ses dosyası: {size:,} bayt")
            assert size > 10_000, "ses çok kısa"
        p.stdin.write('{"quit": true}\n')
        p.stdin.close()
        p.wait(timeout=30)
    finally:
        timer.cancel()
        if p.poll() is None:
            p.kill()


if __name__ == "__main__":
    main(sys.argv[1])
