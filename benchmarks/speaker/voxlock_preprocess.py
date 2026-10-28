#!/usr/bin/env python3
"""Run the S3 test-set scenes (tune set when there is no test set) of the speaker-bench fixtures through VoxLock (measurement only).

VoxLock is target-speaker extraction: it keeps the caller's voice and removes the others. It
needs sherpa-onnx >= 2.0 (Python); the Rust bench pins 1.13.2, so the audio is produced here and
read back by speaker_bench as `--pre precomputed:<name>=<out dir>`.

Only the API of the upstream examples online-speech-enhancement-voxlock.py and
online-speech-enhancement-voxlock-with-voice-sample.py (k2-fsa/sherpa-onnx python-api-examples) is
used: OnlineSpeechDenoiserConfig(model=OfflineSpeechDenoiserModelConfig(voxlock=
OfflineSpeechDenoiserVoxLockModelConfig(enroll_model, chunk_model, voice_sample), num_threads,
provider)), OnlineSpeechDenoiser(config), sd(chunk, sample_rate), sd.flush(), sd.sample_rate.

  voxlock_preprocess.py --fixtures <dir> --model <sherpa-onnx-voxlock-int8 dir> --out <dir>
      --mode live|sample [--threads 1] [--chunk-ms 20|40|80] [--cost-only]
  voxlock_preprocess.py --self-test --model <dir>

live:   no voice sample; VoxLock passes audio through until it has heard 3 s of speech, takes that
        as the speaker to keep (what a product would do).
sample: voice_sample = the scene's enrollment segments (manifest `enroll: true`) cut from the
        scene audio.

Output: <out>/scenes/<id>.16k.wav, <out>/scenes/<id>.wav (48 kHz stereo, linear-interpolation
upsample), <out>/manifest.json (copy), <out>/voxlock_cost.json (wall and process-CPU seconds per
scene; run the script under `taskset -c 2` for a one-core number).
"""

import argparse
import json
import shutil
import sys
import tempfile
import time
from pathlib import Path

import numpy as np

try:
    import sherpa_onnx
    import soundfile as sf
except ImportError as exc:  # pragma: no cover
    sys.exit(f"missing dependency ({exc}); pip install -r benchmarks/speaker/requirements-voxlock.txt")

SR = 16000


def require_voxlock_api():
    if not hasattr(sherpa_onnx, "OfflineSpeechDenoiserVoxLockModelConfig"):
        sys.exit(
            f"sherpa-onnx {getattr(sherpa_onnx, '__version__', '?')} has no "
            "OfflineSpeechDenoiserVoxLockModelConfig; install the version pinned in "
            "benchmarks/speaker/requirements-voxlock.txt"
        )


def make_denoiser(model_dir: Path, voice_sample: str, threads: int):
    enroll = model_dir / "enroll.onnx"
    chunk = model_dir / "chunk_int8.onnx"
    for f in (enroll, chunk):
        if not f.is_file():
            sys.exit(f"{f} not found; run download-enhance.mjs")
    config = sherpa_onnx.OnlineSpeechDenoiserConfig(
        model=sherpa_onnx.OfflineSpeechDenoiserModelConfig(
            voxlock=sherpa_onnx.OfflineSpeechDenoiserVoxLockModelConfig(
                enroll_model=str(enroll),
                chunk_model=str(chunk),
                voice_sample=voice_sample,
            ),
            debug=False,
            num_threads=threads,
            provider="cpu",
        )
    )
    if not config.validate():
        sys.exit("VoxLock config is invalid; see the log above")
    return sherpa_onnx.OnlineSpeechDenoiser(config)


def stream(sd, samples: np.ndarray, chunk_samples: int):
    """Feed `samples` in chunks; returns (output, wall seconds, cpu seconds)."""
    out = []
    wall0, cpu0 = time.perf_counter(), time.process_time()
    for off in range(0, len(samples), chunk_samples):
        res = sd(samples[off : off + chunk_samples], SR)
        out.append(np.asarray(res.samples, dtype=np.float32))
    out.append(np.asarray(sd.flush().samples, dtype=np.float32))
    wall, cpu = time.perf_counter() - wall0, time.process_time() - cpu0
    return (np.concatenate(out) if out else np.empty(0, np.float32)), wall, cpu


def to_48k_stereo(mono16: np.ndarray) -> np.ndarray:
    n = len(mono16)
    t = np.arange(n * 3) / 3.0
    up = np.interp(t, np.arange(n), mono16).astype(np.float32)
    return np.stack([up, up], axis=1)


def read16k(path: Path) -> np.ndarray:
    data, rate = sf.read(str(path), always_2d=True, dtype="float32")
    if rate != SR:
        sys.exit(f"{path}: expected {SR} Hz, got {rate}")
    return np.ascontiguousarray(data[:, 0])


def run(args):
    require_voxlock_api()
    fixtures, out = Path(args.fixtures), Path(args.out)
    manifest = json.loads((fixtures / "manifest.json").read_text())
    convs = [s for s in manifest["scenes"] if s.get("kind") == "conversation"]
    # Same choice as speaker_bench Part 3: the test set, else (quick fixtures) the tune set.
    use = "test" if any(s.get("set") == "test" for s in convs) else "tune"
    scenes = [s for s in convs if s.get("set") == use]
    if not scenes:
        sys.exit("no S3 conversation scene in the manifest")
    chunk = args.chunk_ms * SR // 1000
    (out / "scenes").mkdir(parents=True, exist_ok=True)
    shutil.copyfile(fixtures / "manifest.json", out / "manifest.json")
    cost = []
    for scene in scenes:
        sid = scene["id"]
        audio = read16k(fixtures / "scenes" / f"{sid}.16k.wav")
        voice_sample, tmp = "", None
        if args.mode == "sample":
            parts = [
                audio[s["start_ms"] * SR // 1000 : s["end_ms"] * SR // 1000]
                for s in scene["segments"]
                if s.get("enroll")
            ]
            if not parts:
                sys.exit(f"{sid}: no enrollment segment for --mode sample")
            tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
            sf.write(tmp.name, np.concatenate(parts), SR)
            voice_sample = tmp.name
        sd = make_denoiser(Path(args.model), voice_sample, args.threads)
        if sd.sample_rate != SR:
            sys.exit(f"VoxLock output rate {sd.sample_rate} != {SR}")
        enhanced, wall, cpu = stream(sd, audio, chunk)
        if tmp:
            Path(tmp.name).unlink(missing_ok=True)
        cost.append(
            {
                "id": sid,
                "audio_s": len(audio) / SR,
                "wall_s": wall,
                "cpu_s": cpu,
                "in_samples": int(len(audio)),
                "out_samples": int(len(enhanced)),
            }
        )
        print(f"{sid}: {len(audio) / SR:.1f} s audio, wall {wall:.1f} s, cpu {cpu:.1f} s", flush=True)
        if not args.cost_only:
            sf.write(str(out / "scenes" / f"{sid}.16k.wav"), enhanced, SR)
            sf.write(str(out / "scenes" / f"{sid}.wav"), to_48k_stereo(enhanced), 48000, subtype="PCM_16")
    (out / "voxlock_cost.json").write_text(
        json.dumps(
            {
                "mode": args.mode,
                "chunk_ms": args.chunk_ms,
                "threads": args.threads,
                "sherpa_onnx": sherpa_onnx.__version__,
                "scenes": cost,
            },
            indent=2,
        )
    )


def self_test(model: str) -> int:
    model_dir = Path(model)
    if not (model_dir / "enroll.onnx").is_file():
        print(f"SKIP self-test: VoxLock model not found in {model_dir}")
        return 0
    require_voxlock_api()
    t = np.arange(2 * SR) / SR
    tone = (0.2 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)
    for chunk_ms in (20, 40, 80):
        sd = make_denoiser(model_dir, "", 1)
        out, _, _ = stream(sd, tone, chunk_ms * SR // 1000)
        slack = chunk_ms * SR // 1000
        if abs(len(out) - len(tone)) > slack:
            print(f"FAIL chunk {chunk_ms} ms: output {len(out)} vs input {len(tone)} samples")
            return 1
        print(f"ok chunk {chunk_ms} ms: output {len(out)} vs input {len(tone)} samples")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--fixtures")
    ap.add_argument("--model", required=True)
    ap.add_argument("--out")
    ap.add_argument("--mode", choices=["live", "sample"], default="live")
    ap.add_argument("--threads", type=int, default=1)
    ap.add_argument("--chunk-ms", type=int, choices=[20, 40, 80], default=40)
    ap.add_argument("--cost-only", action="store_true", help="time only; write no wavs")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        sys.exit(self_test(args.model))
    if not args.fixtures or not args.out:
        ap.error("--fixtures and --out are required")
    run(args)


if __name__ == "__main__":
    main()
