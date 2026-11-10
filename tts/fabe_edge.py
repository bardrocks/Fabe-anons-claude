"""FABE Anons yedek canlı anons motoru: Microsoft Edge TTS (internet gerekir).

The app starts this program once and keeps it running. It talks over stdin/stdout, one JSON
object per line, the same way as fabe_tts.py:

    -> {"id": 1, "text": "Ahmet Bey kasaya", "voice": "tr-TR-EmelNeural", "speed": 1.0, "out": "C:/.../x.mp3"}
    <- {"id": 1, "ok": true, "ms": 640}
    <- {"id": 1, "ok": false, "error": "..."}

The first line it writes is {"ready": true}. Nothing else is written to stdout.
The edge-tts version is pinned in edge-requirements.txt; when Microsoft changes its service,
raising that version and releasing the app is the fix.
"""
import asyncio
import json
import os
import sys
import time
import traceback

for stream in (sys.stdin, sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
OUT = sys.stdout
sys.stdout = sys.stderr  # library prints must not break the protocol

VOICES = {"tr-TR-EmelNeural", "tr-TR-AhmetNeural"}
TIMEOUT = 20  # seconds; the app gives up a little later and falls back


def send(obj):
    OUT.write(json.dumps(obj, ensure_ascii=False) + "\n")
    OUT.flush()


def percent(speed):
    """1.0 -> "+0%", 0.8 -> "-20%" (Edge's prosody rate)."""
    speed = min(2.0, max(0.5, float(speed)))
    return f"{round((speed - 1.0) * 100):+d}%"


async def synth(edge_tts, text, voice, speed, out):
    tmp = out + ".part"
    try:
        com = edge_tts.Communicate(text, voice, rate=percent(speed))
        await asyncio.wait_for(com.save(tmp), TIMEOUT)
        if os.path.getsize(tmp) < 1000:
            raise RuntimeError("Microsoft ses göndermedi")
        os.replace(tmp, out)
    finally:
        if os.path.exists(tmp):
            os.remove(tmp)


def friendly(e):
    s = f"{type(e).__name__}: {e}"
    if isinstance(e, asyncio.TimeoutError):
        return f"Microsoft {TIMEOUT} sn içinde yanıt vermedi"
    low = s.lower()
    if "getaddrinfo" in low or "name or service" in low or "cannot connect" in low or "dns" in low:
        return "İnternete bağlanılamadı"
    if "403" in s:
        return "Microsoft isteği reddetti (403); uygulamanın güncellenmesi gerekebilir"
    return s


def main():
    try:
        import edge_tts
    except Exception as e:  # noqa: BLE001
        send({"ready": False, "error": f"edge-tts yüklenemedi: {e}"})
        return
    print(f"edge-tts {getattr(edge_tts, '__version__', '?')} hazır", file=sys.stderr, flush=True)
    send({"ready": True})
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except ValueError:
            continue
        if req.get("quit"):
            break
        rid = req.get("id")
        start = time.time()
        try:
            voice = req.get("voice") or "tr-TR-EmelNeural"
            if voice not in VOICES:
                raise ValueError(f"bilinmeyen ses: {voice}")
            text = " ".join(str(req.get("text", "")).split())
            if not text:
                raise ValueError("boş metin")
            asyncio.run(synth(edge_tts, text, voice, req.get("speed", 1.0), str(req["out"])))
            send({"id": rid, "ok": True, "ms": int((time.time() - start) * 1000)})
        except Exception as e:  # noqa: BLE001
            traceback.print_exc(file=sys.stderr)
            sys.stderr.flush()
            send({"id": rid, "ok": False, "error": friendly(e)})


if __name__ == "__main__":
    main()
