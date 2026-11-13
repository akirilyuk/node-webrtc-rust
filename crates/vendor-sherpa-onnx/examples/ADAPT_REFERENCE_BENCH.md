# Adaptive speaker reference bench

Decides the default of `speakerGate.adaptReference` (`DEFAULT_ADAPT_REFERENCE` in `crates/speech/src/config.rs`). Gate: caller FRR drops on the drift set, and FAR does not rise on any set (poison included). Source: `adapt_reference_bench.rs` (method in its header).

```text
cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example adapt_reference_bench -- \
  --model examples/voice-agent-local-sherpa/.models/speaker/nemo_en_titanet_small.onnx \
  --librispeech <LibriSpeech dir with test-clean decoded to wav> \
  --bed <tocumen48.f32> --out <file.md>
```

The adapted reference uses the gate's own `adapt_step` (`ADAPT_MARGIN` 0.1, `ADAPT_ALPHA` 0.1, `ANCHOR_MIN_COSINE` 0.8). An utterance commits only when every window in it scores `caller`. Sets:

- clean: no channel change.
- drift: the channel changes by thirds of the call (unchanged, then telephone band at -6 dB, then telephone band at -12 dB through 8-bit mu-law).
- bed: Tocumen airport bed at -12 dB.
- poison: the bed set, with the 3 bystanders nearest each caller.

## Result (2026-11-13, TitaNet-small)

Speakers: 40 (LibriSpeech test-clean, same-session call, up to 300 s), 1.5 s windows + 300 ms pre-roll, threshold 0.459, bed -12 dB. Run time 264 s.

| set    | caller windows | FRR fixed % | FRR adapted % | bystander windows | FAR fixed % | FAR adapted % | commits / by bystander / rejected (anchor) | mean final cos to anchor | min cos to anchor |
| ------ | -------------- | ----------- | ------------- | ----------------- | ----------- | ------------- | ------------------------------------------ | ------------------------ | ----------------- |
| clean  | 5956           | 2.74        | 0.89          | 7065              | 0.014       | 0.085         | 1304 / 0 / 24                              | 0.854                    | 0.801             |
| drift  | 5956           | 13.60       | 2.15          | 7065              | 0.014       | 0.113         | 1049 / 0 / 227                             | 0.812                    | 0.800             |
| bed    | 5956           | 2.99        | 1.07          | 7065              | 0.057       | 0.184         | 1303 / 0 / 18                              | 0.857                    | 0.801             |
| poison | 5956           | 2.99        | 1.07          | 6616              | 1.527       | 4.232         | 1305 / 2 / 18                              | 0.857                    | 0.801             |

Gate: FRR drops on drift: true; FAR does not rise on any set: false. Default adaptReference: off.

## Reading

- Adaptation does what it is for on drift: caller FRR goes from 13.6 % to 2.15 %, and from about 3 % to about 1 % on the other sets.
- It also raises FAR on every set. Clean goes from 0.014 % to 0.085 %, bed from 0.057 % to 0.184 %, and poison from 1.53 % to 4.23 %, almost three times as high. The reference moves toward the channel, and the channel is shared with the bystanders, so near voices on the same line get closer as well. In the poison set two bystander utterances were committed end to end, despite the all-caller rule.
- The anchor bound holds the reference at cos >= 0.80 from the enrollment (min 0.800 to 0.801 on every set; 227 rejects on drift). It does not stop the FAR rise, which happens well inside the bound.

**Gate fails (FAR rises on every set). `DEFAULT_ADAPT_REFERENCE` stays `false`.** The option stays available for callers who care more about FRR under a drifting channel than about bystander accepts.
