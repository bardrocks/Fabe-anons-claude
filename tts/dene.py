"""Build-time check: starts the built engine the way the app does and reads one sentence."""
import json
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


def main(exe, models):
    # The Windows runner's console is cp1252; Turkish text must not crash the check itself.
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    out = Path(tempfile.gettempdir()) / "fabe-tts-deneme.wav"
    out.unlink(missing_ok=True)
    start = time.time()
    p = subprocess.Popen([exe, models], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr,
                         text=True, encoding="utf-8")
    timer = threading.Timer(300, p.kill)
    timer.start()
    try:
        check(p, out, start)
    finally:
        timer.cancel()
        if p.poll() is None:
            p.kill()


def check(p, out, start):
    ready = json.loads(p.stdout.readline())
    print("açılış:", ready, f"{time.time() - start:.1f} sn")
    assert ready.get("ready"), ready
    p.stdin.write(json.dumps({"id": 7, "text": "Ahmet Yılmaz kasaya, 3 numaralı reyona.", "speed": 1.0,
                              "gain": 1.0, "out": str(out)}, ensure_ascii=False) + "\n")
    p.stdin.flush()
    reply = json.loads(p.stdout.readline())
    print("yanıt:", reply)
    assert reply.get("ok") and reply.get("id") == 7, reply
    size = out.stat().st_size
    print(f"ses dosyası: {size:,} bayt")
    assert size > 20_000, "ses çok kısa"
    p.stdin.write('{"quit": true}\n')
    p.stdin.close()
    p.wait(timeout=30)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
