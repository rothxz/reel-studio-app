"""Local speech worker for the agent's voiceover (MLX models on Apple Silicon).

The editor starts this once per model and keeps it: loading takes a few
seconds, a line far less. Usage: `speech_worker.py <model repo or path>`.
Two families:

- Kokoro (82M, fast): a named voice ("af_heart") and a speed; no tone.
- Qwen3-TTS CustomVoice: a named speaker ("Aiden") and a tone ("instruct").

One JSON request per stdin line:

    {"text": "...", "voice": "af_heart", "instruct": "warm", "speed": 1.0,
     "lang": "english", "out": "/path/line.wav"}

and one JSON answer per stdout line: {"ok": true, "out": ..., "seconds": ...}
or {"ok": false, "error": "..."}. The first line out is
{"ready": true, "voices": [...]} once the model is loaded. Logs go to stderr.
"""

import json
import os
import sys
import time


def answer(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def kokoro_lang(voice):
    # The voice's first letter is its accent: a = American, b = British, …
    v = (voice or "a").lower()
    return v[0] if v[:1] in "abefhijpz" else "a"


def main():
    model_id = sys.argv[1]
    kokoro = "kokoro" in model_id.lower()
    try:
        import numpy as np
        import soundfile as sf
        from mlx_audio.tts.utils import load

        model = load(model_id)
        if kokoro:
            voices = []
        else:
            voices = list(model.get_supported_speakers() or [])
    except Exception as e:  # noqa: BLE001 - reported to the editor
        answer({"ready": False, "error": f"{type(e).__name__}: {e}"})
        return
    answer({"ready": True, "voices": voices})

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            started = time.time()
            voice = req.get("voice") or None
            if kokoro:
                runs = model.generate(
                    text=req["text"],
                    voice=voice or "af_heart",
                    speed=float(req.get("speed") or 1.0),
                    lang_code=kokoro_lang(voice),
                    # One take for the whole line: no splitting on newlines.
                    split_pattern=r"\n\n+",
                )
            else:
                runs = model.generate(
                    text=req["text"],
                    voice=voice,
                    instruct=req.get("instruct") or None,
                    lang_code=req.get("lang") or "auto",
                    split_pattern="",
                )
            chunks, rate = [], 24000
            for r in runs:
                chunks.append(np.array(r.audio, dtype=np.float32).reshape(-1))
                rate = r.sample_rate
            if not chunks:
                raise RuntimeError("the model made no sound")
            audio = np.concatenate(chunks)
            tmp = req["out"] + ".part.wav"
            sf.write(tmp, audio, rate, subtype="PCM_16")
            os.replace(tmp, req["out"])
            answer(
                {
                    "ok": True,
                    "out": req["out"],
                    "seconds": round(len(audio) / rate, 3),
                    "took": round(time.time() - started, 2),
                }
            )
        except Exception as e:  # noqa: BLE001 - reported to the editor
            answer({"ok": False, "error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    main()
