# Speaker-ID bench

A bench for deciding whether, and how, the voice agent can tell its caller (the main voice) from other people talking nearby. It has two parts:

- `speaker_fixtures` builds the test scenes (`crates/vendor-sherpa-onnx/examples/speaker_fixtures.rs`).
- `speaker_bench` measures speaker-embedding models and the end-to-end effect on transcripts (`crates/vendor-sherpa-onnx/examples/speaker_bench.rs`).

Nothing here changes product behaviour. The models and audio are test and bench assets and are never shipped.

## Fixture design

A **main speaker** says the test phrases. **Other voices** talk in the background, quieter than the main voice, at several levels. The bench measures how much of the main speech is recognized correctly and attributed to the main voice, and how much speech is attributed to the other voices.

- **Main speaker:** `vits-piper-en_US-amy-low`, sid 0. This is the voice the 3000-run phrase qualification used (`examples/voice-agent-local-sherpa/stt-phrase-qualification.json`), so its phrases are valid oracle targets. The main text is every phrase of that file with `below075 == 0` and `keywordMissing` 0 or null.
- **Other voices**, split so a threshold is never tuned on the voices it is tested on:
  - tune set (thresholds are chosen here): `en_US-ryan-medium`, `en_US-kristin-medium`, `en_US-hfc_male-medium`, `en_US-libritts_r-medium` sids 10 and 50;
  - test set (pass rates are reported here): `en_US-joe-medium`, `en_US-hfc_female-medium`, `en_US-lessac-medium`, `en_US-libritts_r-medium` sids 120 and 300.
- **Long sentences:** 16 customer-service style sentences of 12-20 words (`crates/vendor-sherpa-onnx/examples/support/long_sentences.txt`), also spoken by the main voice. They are **not qualified** by the 3000-run sweep, so the bench never judges them pass/fail; it reports their word error rate (WER) and word similarity, labelled "long (measured, not qualified)". Qualified phrases keep the 0.75 oracle and are reported separately.
- **Background text:** 40 short neutral sentences we wrote ourselves (`crates/vendor-sherpa-onnx/examples/support/background_sentences.txt`). None shares a content word (4+ letters) with a main phrase or a long sentence, so leaked background words are detectable in main transcripts. The generator has a test for this.
- **Relative level:** the RMS over voiced 20 ms frames (frame RMS above -45 dBFS) of the other voice divided by that of the main voice: **-6, -10, -15, -20 dB**, plus **0 dB** for a bystander as loud as the caller. The main voice is normalized to -20 dBFS in every scene; a peak limiter (gain `min(1, 0.98 / max|mix|)` over +-5 ms, instant attack, 50 ms release) keeps every scene below 0.98 and leaves quieter scenes untouched. The manifest records `limiter_max_gr_db` (deepest gain reduction) and `effective_main_db` (voiced RMS of the main voice after the limiter, null without main speech); Grid B labels caller levels with it, in 0.5 dB steps.

Scenes (`<out>/scenes/<id>.wav` is 48 kHz stereo s16le with the same audio in both channels, what the agent receives; `<id>.16k.wav` is 16 kHz mono, for embedding and STT):

| Scene                 | Content                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| S1 `main_over_babble` | Per main phrase (every qualified phrase and every long sentence) x level (-6/-10/-15/-20) x set: 300 ms lead, the phrase, 1500 ms tail. Babble (two simultaneous other voices reading background sentences with 150-400 ms gaps, level-matched together, then scaled to the level) runs across the whole clip.                                                                                                                                                                                                                                             |
| S2 `bystander_alone`  | Per set 60 utterances (30 at 0 dB, 30 at -10 dB): one other voice says one background sentence at 0 or -10 dB re the main voice, with babble at -20 dB underneath. There is no main speech.                                                                                                                                                                                                                                                                                                                                                                |
| S3 `conversation`     | Per level x set one file of about a minute: two enrollment utterances of the main voice, both long sentences (babble at -20 dB), then 16 turns alternating main (4 qualified phrases and 4 long sentences, never an enrollment sentence) and bystander (one other voice, a background sentence, at 0 dB), with 1400-2000 ms gaps (longer than the default VAD's 1100 ms `min_silence_duration_ms` by a margin, so the default VAD ends an utterance in every gap; a unit test guards it). Babble runs throughout, at the scene level after the enrollment. |

`manifest.json` lists per scene `id`, `kind`, `set`, `level_db`, `babble_db`, `seed`, `limiter_max_gr_db`, `effective_main_db`, the sha256 of both wav files and `segments: [{start_ms, end_ms, role, voice, sid, text, kind, expected}]` with `role` one of `main`, `bystander`, `babble`, `kind` one of `qualified`, `long`, `background` and `expected: {speaker: "caller"|"other", transcript: <text, null for babble>}`. Enrollment utterances carry `"enroll": true`. Every random choice is seeded from 20261027 and the scene id.

`--main-levels <dBFS,...>` (default `-20`) adds test-set S3 conversations at other main-voice levels for Grid B, at relative levels -15 and -10 dB (quick: one tune-set scene at -10 dB per extra level). Their ids get a `_l26` / `_l14` suffix (the default -20 keeps its ids), the manifest scene carries `main_db`, and the texts, gaps and babble are the same as at -20. S1 also has `main_clean` scenes (`s1c_*`, the same phrases without babble, `level_db` -99 standing in for minus infinity; test set, quick: both sets), the clean row of the denoiser WER table.

`--quick` builds a smoke set: S1 for 3 qualified phrases and 2 long sentences at -10 dB, S2 with 4 utterances per set, and one S3 at -10 dB (tune set) of 6 turns.

### Determinism

VITS sampling is random, so synthesizing the same text twice gives different audio. The generator therefore keeps every synthesized clip in a cache (default `.models/speaker-tts-cache`, `--cache <dir>` to change it), keyed by bundle, speaker id and text. Running it twice with the same arguments reads the same clips and writes byte-identical files; the sha256 of `manifest.json` (which contains the sha256 of every wav) is printed last. Delete the cache to draw new voices.

## What the bench measures

**Part 1, embedding level**, per embedding model x check length (0.5, 1.0, 1.5 s) x level row:

- _reference_: the embedding of the two enrollment utterances of the main voice in the S3 scene of the same set and level, concatenated;
- _genuine_: the first `check` seconds of every S1 main phrase and S3 main turn, excluding phrases that are an enrollment phrase of the reference scene (same text, same waveform); _impostor_: the first `check` seconds of every S2 bystander utterance and S3 bystander turn;
- _score_: cosine similarity of the clip embedding and the reference; a clip is accepted when its score is above the threshold;
- **EER**: the error rate where false accepts (FAR) and false rejects (FRR) are equal;
- **FRR at FAR 1 %** and the **threshold at FAR 1 %**: both from the **tune set** only; then that threshold is applied to the **test set** and its FRR and FAR are reported;
- the row `real voices (clean)` uses the Sherpa real-voice clips (enroll `*-sr-*`, test `*-test-sr-*`); it has no tune/test split;
- `compute` time per clip (stream, waveform, embedding), p50 and p99, on one core.

**Part 2, end to end**, on the S3 scenes of the test set: each scene is fed in 20 ms frames of 48 kHz stereo PCM through the speech crate's `VadEngine` with `VadConfig::default()` (the energy VAD).

**Silero is not offered.** It was evaluated as a second VAD variant and merged every babble conversation into one utterance (workflow run 40529240112), so the bench no longer builds or runs it.

Each VAD utterance is transcribed with the Zipformer online recognizer and embedded (first `check` seconds, but never past the end of the utterance, with the model and length with the lowest tune-set EER and its tune-set threshold), then attributed to the caller or to someone else. Both the recognizer input and the embedding clip start where the product STT feed starts: the SpeechStart frame minus the **pre-roll** (`speech_pad_ms + min_speech_duration_ms`, the capacity `SttPreRollBuffer::from_vad_config` uses; a unit test guards it). The utterance is matched to the manifest segment it overlaps most, if that is at least 50 % of the utterance. Per VAD variant and level it reports:

- (a) share of **qualified** main turns whose transcript passes the roundtrip oracle (`wordSimilarity` >= 0.75); long sentences are not judged: the table shows their WER (a turn no utterance matches counts as all words deleted) and `results.json` their mean word similarity;
- (b) share of main turns attributed to the caller (all, qualified only, long only);
- (c) share of bystander turns attributed to others;
- (d) share of bystander turns attributed to the caller (false accepts);
- (e) background-word leakage: background words (4+ letters, not in a main phrase) found in main-turn transcripts, divided by the words of those transcripts;
- (f) VAD utterances that match no segment, and how they were attributed;
- merged utterances: VAD utterances that cover (at least half of) two or more turns, which usually means continuous babble kept the VAD from closing the utterance between turns.

A turn that no VAD utterance matches counts as missed; the counts are in the table. `--vad-min-silence-ms <n>` optionally overrides the VAD's `min_silence_duration_ms` (the workflow does not pass it).

**Part 3, denoisers and caller gain** (`--enhance <denoiser models dir>`, `--pre raw,rnnoise,gtcrn,...` to restrict the list). It runs the Part 2 pipeline in the order the product would: a **preprocessor** on the 20 ms 48 kHz stereo frames, an optional **AGC**, then the split into the VAD (on that 48 kHz PCM) and STT / speaker embedding (on its mono 16 kHz conversion, `stereo_48k_to_mono_16k`, starting at SpeechStart minus the product pre-roll). The enrollment is cut from the same processed signal. The speaker model is TitaNet-small at 1.5 s with the Part 1 tune threshold (the Part 2 pick if TitaNet is missing).

- **Preprocessors** (`support/enhance.rs`, `process` returns exactly the input length): `raw`; `rnnoise` (the product's `Stereo48kRnnoise`); `gtcrn` (`gtcrn_simple.onnx`) and the DPDFNet variants `dpdfnet_baseline`, `dpdfnet2`, `dpdfnet4`, `dpdfnet8`, `dpdfnet2_48khz_hr` through sherpa-onnx `OnlineSpeechDenoiser` (one thread, CPU). The Sherpa path downmixes to mono, averages 48 kHz down to the model rate (16 kHz; the `_48khz_hr` model runs at 48 kHz), runs the denoiser, upsamples linearly and duplicates to stereo. An output FIFO pre-filled with silence equal to the model's frame shift keeps every call at the input length; a call that finds the FIFO short is counted as an underrun (reported per row; the model's own window needs a few calls at the start of a stream).
- **AGC**, the exact algorithm the product would copy, per 20 ms frame after the preprocessor:
  - `frame_db` = 20 log10(RMS of the mono downmix, floor 1e-6);
  - noise floor `n` (start -60 dB): `n = min(n + 0.02, frame_db)` per frame (falls at once, rises 1 dB/s);
  - a frame is speech when `frame_db > n + 10`;
  - speech level `s` (start -30 dB): on speech frames only, `s += 0.05 (frame_db - s)`;
  - target gain `t = clamp(-18 - s, 0, +18)` dB; applied gain `g` (start 0): `g += 0.02 (t - g)` per frame;
  - the frame is multiplied by `10^(g/20)`; if its sample peak would exceed -1 dBFS (0.891), the whole frame is scaled to exactly that peak.

  Gain alone raises the babble by the same amount, so it does not change SNR. The default energy VAD threshold is absolute (0.15 RMS, about -16.5 dBFS), so a caller at -26 dBFS can sit under it; that is the effect the AGC is meant to test. A denoiser is the only stage that raises the caller relative to the babble.

- **Grid A**: every preprocessor x AGC off / on over the S3 test scenes at main -20 dBFS and the four relative levels. Per combo and level: VAD utterances, merged utterances, missed main and bystander turns, (a) oracle % of qualified phrases, WER % of long sentences, (b) main to caller %, (c) bystander to other %, (e) leakage %, (f) unmatched utterances, the median VAD onset delay (utterance start minus segment start, over detected main turns, ms) and the underruns. A pooled `all` row per combo follows its levels.
- **Grid B, caller level**: `raw`, `rnnoise`, `gtcrn`, `dpdfnet2` x AGC off / on over the scenes with the main voice at -26 / -20 / -14 dBFS (fixtures `--main-levels -26,-20,-14`) at relative levels -15 and -10 dB, same metrics. The bystander follows the main level (0 dB) and the babble follows the relative level, so only the absolute level changes; the texts, gaps and babble are the same at the three levels.
- **S1 whole-clip WER**: per preprocessor (AGC off) x level (-6 / -10 / -15 / -20 dB and `clean`, the `main_clean` scenes without babble), the whole S1 clip goes through the preprocessor and the online recognizer with no VAD. Qualified phrases: oracle %; long sentences: WER %. This separates the denoiser effect from VAD effects.
- **Cost**, per preprocessor and for the AGC alone, on one core (the workflow pins the process with `taskset -c 2`):
  - _RTF_: processing time per 20 ms frame divided by 20 ms, p50 / p99 / mean over `--cost-seconds` (default 60) of S3 audio, the first 25 frames not counted;
  - _streams per core_: N real-time paced streams on one thread (one frame per stream every 20 ms) for `--streams-seconds` (default 10) each, N in 1, 2, 4, 8, 12, 16, 24, 32; the largest N with at most 1 % of frames finishing more than 20 ms after they were due (stops at the first failing N; `+` = the last step passed);
  - _added delay_: the lag (0 to 800 ms, 8 kHz) that maximizes the cross-correlation of the input and the processed signal on a clean S1 clip (`main_clean`; an S1 babble clip if none).

Outputs: the Part 3 tables are appended to `summary.md` and `results.json` has a `part3` object (`grid`, `grid_a_pooled`, `s1_wer`, `cost`).

Outputs: `results.json` (all numbers plus `nproc`, CPU model, git sha from `NWR_BENCH_GIT_SHA` or `git rev-parse HEAD`, manifest sha256) and `summary.md`, plus the **per-turn record**:

- `turns.jsonl`: one row per manifest segment (enrollment, main, bystander) of every S3 scene and run, in time order. Each row has `who` (role, voice, sid), `said` (kind, text), `expected` (speaker, transcript), `noticed` (ids and start/end ms of the VAD utterances overlapping the segment, the transcript of the utterance it matched, the speaker score rounded to 3 decimals, the verdict `caller` / `other` / `not noticed`; `enrollment` for the enrollment utterances), `merged` (segment ids when an overlapping utterance covered two or more turns) and `result` (attribution ok, WER for long sentences, oracle for qualified phrases). VAD utterances that match no segment get a row of their own with `who.role` `none/babble`. Every row carries `pre` and `agc` (`part2` and `false` for the Part 2 path, which has no preprocessor).
- `conversations.md`: the same as one timeline table per scene (and, for Part 3, per preprocessor and AGC setting; only raw/off, gtcrn/on and dpdfnet2/on are written, to keep it readable): `time | who spoke | said | expected | noticed (transcript) | verdict | ✓/✗`.

The scores and transcripts are bench data from synthetic voices, not customer content.

## VoxLock (measured only, not a product path)

VoxLock is target-speaker extraction: it keeps the caller's voice and removes the other voices (Apache-2.0, 27 M parameters). Its author's numbers put it at about one CPU core per call, and its Rust API needs sherpa-onnx >= 2.0, while this repo pins 1.13.2. So it is measured here with the same fixtures, pipeline and metrics, and becomes a follow-up only if the numbers justify the cost (gate G3 of the plan).

- `voxlock_preprocess.py` (Python, `requirements-voxlock.txt`: sherpa-onnx 2.0.1, the first PyPI release with `OfflineSpeechDenoiserVoxLockModelConfig`) streams every S3 conversation scene (test set; tune set for `--quick` fixtures) through VoxLock in `--chunk-ms` chunks (20, 40 or 80) and writes a processed copy of the scenes (`scenes/<id>.16k.wav`, plus a 48 kHz stereo `scenes/<id>.wav` by linear-interpolation upsampling) and `voxlock_cost.json`.
- Two enrollment modes. `live`: no voice sample; VoxLock passes audio through until it has heard 3 s of speech, takes that as the speaker to keep (what a product would do). `sample`: `voice_sample` is the scene's two enrollment segments cut from the scene audio (upstream uses its loudest 3 s).
- `speaker_bench --pre raw,precomputed:voxlock-live=<dir>,precomputed:voxlock-sample=<dir>` reads the scene audio from `<dir>/scenes/<id>.wav` instead of the fixtures and runs the identical pipeline (energy VAD, AGC off and on, STT, gate, metrics). Rows appear as `voxlock-live` / `voxlock-sample` in Grid A, Grid B and `turns.jsonl`; `conversations.md` also renders `voxlock-live` with AGC on.
- Cost method: the workflow runs the script under `taskset -c 2` (one core), and `voxlock_cost.json` holds wall and process-CPU seconds per scene. The bench's cost table shows process CPU seconds per audio second in the RTF columns (a ratio over whole scenes, not per 20 ms frame); streams per core and delay are not measured for it. The 20 ms and 80 ms chunk runs are cost-only and are uploaded as `voxlock_cost_live_<ms>ms.json`.
- The S1 whole-clip WER table has no VoxLock rows (only S3 scenes are processed).

## Licenses

Speaker-embedding models (release tag `speaker-recongition-models` (sic) of `k2-fsa/sherpa-onnx`). A model whose license does not allow commercial use, or states none, is dropped.

| Model                                               | License                                  | Source                                                                                                                                                                                                                                                                                                                                                                                                                  | Kept |
| --------------------------------------------------- | ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---- |
| `3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx` | Apache-2.0                               | ModelScope card `iic/speech_campplus_sv_en_voxceleb_16k` ("Apache License 2.0", https://modelscope.cn/models/iic/speech_campplus_sv_en_voxceleb_16k); repo license https://github.com/modelscope/3D-Speaker/blob/main/LICENSE                                                                                                                                                                                           | yes  |
| `wespeaker_en_voxceleb_resnet34_LM.onnx`            | CC BY 4.0 (attribution required)         | Hugging Face card `Wespeaker/wespeaker-resnet34-LM` (`license: cc-by-4.0`, https://huggingface.co/Wespeaker/wespeaker-resnet34-LM); WeSpeaker docs ("the pretrained model on VoxCeleb follows Creative Commons Attribution 4.0", https://github.com/wenet-e2e/wespeaker/blob/master/docs/pretrained.md)                                                                                                                 | yes  |
| `nemo_en_titanet_small.onnx`                        | License of the NeMo Toolkit (Apache-2.0) | NGC model card `nvidia/nemo/titanet_small` ("License to use this model is covered by the license of the NeMo Toolkit", https://catalog.ngc.nvidia.com/orgs/nvidia/teams/nemo/models/titanet_small); toolkit license https://github.com/NVIDIA/NeMo/blob/main/LICENSE. The Hugging Face card of the large model says CC BY 4.0; the small model has no Hugging Face card we could open. Both terms allow commercial use. | yes  |

Piper voices (release tag `tts-models`). They are test-only assets, never shipped, but their dataset licenses are recorded. Source for each: `MODEL_CARD` of the voice in https://huggingface.co/rhasspy/piper-voices (the same file is in the extracted bundle).

| Voice                        | Dataset license (as stated on the card)  | Dataset                                                                           |
| ---------------------------- | ---------------------------------------- | --------------------------------------------------------------------------------- |
| `en_US-amy-low` (main voice) | "See URL", no license stated on the card | https://github.com/MycroftAI/mimic3-voices                                        |
| `en_US-ryan-medium`          | CC BY-NC-SA 4.0                          | https://www.kaggle.com/datasets/roholazandie/ryanspeech                           |
| `en_US-kristin-medium`       | public domain                            | https://librivox.org                                                              |
| `en_US-hfc_male-medium`      | CC BY-NC-SA 4.0                          | https://ast-astrec.nict.go.jp/en/release/hi-fi-captain/                           |
| `en_US-libritts_r-medium`    | CC BY 4.0                                | http://www.openslr.org/141/                                                       |
| `en_US-joe-medium`           | CC0                                      | https://github.com/OHF-Voice/voice-datasets                                       |
| `en_US-hfc_female-medium`    | CC BY-NC-SA 4.0                          | https://ast-astrec.nict.go.jp/en/release/hi-fi-captain/                           |
| `en_US-lessac-medium`        | Blizzard 2013 Lessac license             | https://www.cstr.ed.ac.uk/projects/blizzard/2013/lessac_blizzard2013/license.html |

The non-commercial and unstated licenses (ryan, hfc_male, hfc_female, lessac, amy) are acceptable only because the audio is generated for tests and bench runs and is not redistributed in a product, image or package. The test-set `.wav` artifacts of the workflow expire after 14 days.

## Run it locally

Models: `npm run download-tts:en`, `npm run download-stt:en`, `npm run download-speaker` and `npm run download-enhance` (all with `--workspace=@node-webrtc-rust/example-voice-agent-local-sherpa`). `download-speaker` checks every file against the release's `checksum.txt` and exits 1 naming the file on a mismatch.

`download-enhance` checks each denoiser file against the `checksum.txt` of the `speech-enhancement-models` release where that lists it (`gtcrn_simple.onnx`), and against a hash pinned in `sherpa-model-catalog.mjs` otherwise (the DPDFNet files are not in that `checksum.txt`; the hashes are the digests GitHub shows for the release assets). It also fetches `sherpa-onnx-voxlock-int8.tar.bz2` (hash pinned in the script itself) and extracts it to `.models/enhance/sherpa-onnx-voxlock-int8/` (`enroll.onnx`, `chunk_int8.onnx`).

Heavy local runs go through the workspace wrapper (it waits for free memory and stops a runaway process):

```bash
cd node-webrtc-rust
M=examples/voice-agent-local-sherpa/.models
H=/Users/akirilyuk/GIT/webrtc-cloud/development/scripts/host-safety/heavy-run.sh
$H --label speaker-fixtures -- cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
  --models $M/speaker --main-tts $M/vits-piper-en_US-amy-low \
  --phrases examples/voice-agent-local-sherpa/stt-phrase-qualification.json --out .test-logs/speaker-fixtures [--quick] [--main-levels -26,-20,-14]
$H --label speaker-bench -- cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_bench -- \
  --fixtures .test-logs/speaker-fixtures --models $M/speaker \
  --stt-model $M/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 --out .test-logs/speaker-bench \
  [--enhance $M/enhance [--pre raw,gtcrn,precomputed:voxlock-live=<dir>]]
```

The other voices' bundles are read from the parent of `--models` (`--voices-root` overrides it).

## Run it on the EU runner

The workflow `.github/workflows/speaker-bench.yml` is dispatch-only. Load work always goes to the EU self-hosted runners, never to the ARC scale sets:

```bash
gh workflow run speaker-bench.yml -R akirilyuk/node-webrtc-rust --ref main -f runner=self-hosted -f quick=false
```

It runs `speaker_bench` under `taskset -c 2`, writes `summary.md` to the step summary, uploads `results.json`, `summary.md`, `manifest.json`, `turns.jsonl` and `conversations.md` as `speaker-bench-<run id>` and the test-set conversation scenes as `speaker-scenes-<run id>` (14 days). The workflow file must be on `main` before it can be dispatched. Check `gh run list -R akirilyuk/node-webrtc-rust --status in_progress` and `--status queued` first: one long run at a time.

## Adaptive VAD

`--vad energy,adaptive` (default both) runs Part 2 once per VAD provider; `turns.jsonl` rows carry `vad`, and Part 2 reports missed main turns and the median onset per VAD. `--adaptive-sweep` first tunes the adaptive VAD on the tune-set conversations (raw audio, AGC off): `noiseMarginDb` in {4, 6, 8, 10} x `useSpeechLevel` in {false, true}. The pick is the candidate with the most detected main turns whose merged + unmatched utterances do not exceed the energy VAD's; ties go to the larger margin, then `useSpeechLevel` false. The table and the pick are written to `results.json` (`adaptive_sweep`) and `summary.md`, and the test scenes use the pick. With `--enhance`, Part 3 runs once per VAD (cost and S1 WER only for the first); the first is `part3` in `results.json`, the others `part3_<vad>`. The workflow takes the inputs `vad` and `voxlock` (default false: no VoxLock step), and its `--pre` default is `raw,rnnoise`.
