# OpenAI through VoiceThere on node-webrtc-rust 0.12.2, staging (EU), 2026-10-20

One conversation, 10 counting-echo turns, OpenAI `gpt-4o-mini-transcribe` STT and `tts-1` TTS,
08:29–08:35 UTC. Same setup as [`../2026-10-20-staging-eu/`](../2026-10-20-staging-eu/) (cluster
to itself, TTS phrase cache off, one laptop as the client), but the agent runners were on
node-webrtc-rust 0.12.2, which buffers 250 ms of OpenAI audio before a reply starts playing.
`tts_first_audio_ms` is therefore when playback starts. This folder replaces the OpenAI row of
`../2026-10-20-staging-eu/vendors/openai-openai`, which ran on 0.12.0.

The client confirmed every reply (`inbound_echo_ok` 10/10); no host suspend or client starvation.
