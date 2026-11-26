# Testing

```sh
cargo test --workspace                          # everything; oracle tests skip without ffmpeg
cargo test -p filmcraft-h264                    # one crate
cargo test --release -p filmcraft-hevc          # codec tests are much faster in release
cargo xtask ci                                  # all gates (fmt, clippy, tests, layers, assets, wasm)
cargo xtask fixtures                            # pre-generate the ffmpeg fixture matrix
FILMCRAFT_REQUIRE_ORACLES=1 cargo test --workspace   # CI: missing ffmpeg fails instead of skipping
```

## 1. Kinds of tests

| Kind | Where | Examples |
|---|---|---|
| Unit | `src/` `#[cfg(test)]` modules, `src/tests.rs` | CABAC engines, VLC tables (prefix-freeness, Kraft sums), colour matrices, frame cache |
| Property (`proptest`) | `time`, `bitstream`, `edit`, `isobmff/tests/roundtrip.rs`, `interchange/tests/*` | tick↔frame round trips for every rate, DF timecode, edit invariants (no overlaps, durations conserved), mux→demux round trips, interchange export→import |
| Synthetic streams | `h264/src/synth_tests.rs`, `hevc/src/synth_tests.rs` | hand-built bitstreams for features the reference encoders don't emit; expected output known exactly |
| Scripted UI | `crates/ui-egui/tests/scripted.rs` | headless app (egui_kittest) driven over the control channel: razor/undo, insert, apply effect, playback, click by automation id (§4) |
| Monitor view / Graphics menu | `crates/ui-egui/tests/view_ui.rs`, `crates/engine/src/graphics_tests.rs` | View menu items and checkmarks, playback/paused resolution, channel / comparison / multi-camera / waveform display modes, magnification and Hand-tool panning, guides dragged from the rulers (move, lock, remove, Add Guide…, templates in the preferences), snapping a graphic to the frame centre and a guide; align to frame / as group / to selection, distribute (centres and gaps), arrange, select next/previous graphic and layer, reset parameters / duration, vertical text, New Layer from file (`FILMCRAFT_UI_SHOTS=<dir>` writes `view-*.png`) |
| Settings | `crates/engine/src/settings_tests.rs`, `crates/ui-egui/tests/settings_ui.rs` | schema keys and defaults, persistence in the data directory, v1 → v2 migration and repair of bad values, validation, per-category reset; each wired setting (still / transition durations, step many, label defaults and colours, media scaling, timebase, recent projects, media cache policy, output mapping, smart quotes, auto-transcribe); the dialog: every category from its command and Cmd+,, edits by automation id, OK / Cancel / Escape / Reset…, theme, label names in Edit ▸ Label, tooltips, frame cache (`FILMCRAFT_UI_SHOTS=<dir>` writes `settings-*.png`) |
| Scopes and panels (M8.9 / M12.6) | `crates/scopes/src/tests.rs`, `crates/engine/src/{scopes,panels}_tests.rs`, `crates/ui-egui/tests/panels_ui.rs` | scope maths on generated frames (flat colours → exact histogram bins, ramps → waveform row = code value, 75 % bars → parade levels and vectorscope target cells for BT.601/709/2020, HLS hue angles, YC chroma, Clamp Signal, decimation, NaN); `scopes.read` on a colour matte (exact bins, 78.43 %) and on Bars and Tone (the six targets); metadata edits (one undo step, read-only fields refused, saved in the project), the event log (failed / disabled commands, repeats, jobs started / finished / failed / cancelled); every panel from Window ▸, the scopes' wrench menu, presets, five-scope grid, typing into a Metadata field, Timecode rows and modes, Events filter / Clear All, Progress cancel, Reference Monitor park / gang / scopes (`FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `panels-*.png`); `perf_scopes_at_1080p` (ignored) times each scope |
| Keyframe navigator and Effect Controls ruler | `crates/engine/src/tests.rs` (`add_keyframe_toggles_the_keyframe_at_the_playhead`), `crates/ui-egui/tests/keyframes_ui.rs`, `lane_tests` in `crates/ui-egui/src/panels/effect_controls.rs` | `effects.addKeyframe` adds a keyframe at the playhead or removes the one there (the value falls back to the remaining keyframes; undo, redo, parameters that name nothing); headless: the Properties diamond turns animation on, adds and removes the keyframe at the playhead (Scale 50 % → 100 % again), its ◀ ▶ arrows and Effect Controls' step between keyframes, Crop is applied before its first keyframe; a click on the Effect Controls time ruler and a drag of the playhead's handle move the playhead, and no playhead is drawn off the clip; ruler labels are sequence timecode, and clip times, frame rates and panel widths at their limits don't panic (`FILMCRAFT_UI_SHOTS=<dir>` writes `keyframes-*.png`) |
| Golden images | `crates/golden/tests/golden.rs` | CPU renders vs committed PNGs; GPU vs CPU on the same scenes (§3) |
| Engine / command | `crates/engine/src/tests.rs` | run commands on the demo project, assert the sequence, undo/redo, disabled cases |
| Render | `crates/render/src/tests.rs` | compositing, opacity, Motion, cross dissolve midpoint, ½-res vs full, GPU plan vs reference, audio mix, audio-effect continuity |
| Multi-camera / sync | `crates/audio-dsp/src/sync.rs`, `crates/engine/src/multicam_tests.rs`, `crates/ui-egui/tests/multicam_ui.rs` | audio offsets within one sample: Rust-generated multi-mic speech (gains −20…+10 dB, SNR 40/25/10/3 dB, zero-phase colouring, fractional delays, a 0 dB-SNR room with a −4 dB echo) and ffmpeg-made camera/recorder files (pink-noise bursts, independent noise; exact lags recovered); timecode (± hours) and marker sync within a frame; Merge Clips; timeline Synchronize keeps frame-aligned starts; multicam structure, angle render (PSNR vs the camera), Switch Audio mix = the angle's track, live switching from a simulated playback clock (cuts, healing, one undo step), Ctrl+N cuts, flatten (frames identical), save/load; 20 angles: paging (auto / 2×2 / 3×3), page-relative keys, absolute cameras beyond 16, a page decoding only its angles at the cell scale (recording source), Auto-Adjust scale, Selection Top Down; headless: page arrows, angle 17–20 cells, grid-only view, Edit Cameras thumbnails; `perf_grid_of_four_1080p_angles` (ignored) prints the grid cost |
| Project panel and Media Browser (M12.7) | `crates/engine/src/{project_panel,media_browser}_tests.rs`, `crates/ui-egui/tests/project_panel_ui.rs` | view settings and their preference round trip; Metadata Display columns (custom fields, Freeform keys hidden), numeric vs text sorting, view presets (save / save as / restore / rename / delete, ten slots, persisted in `preferences.json`); Freeform moves keeping offsets, snapping, Align to Grid, Clip Size, stacks, arrangements, Reset (each one undo step, saved in the project); bin rename / nesting; the Media Browser against a fake filesystem (folders first, hidden and unsupported files left out, file-type filter, back / forward / up, recent directories, Favorites, import of a selection or a folder, Open in Source Monitor, ingest, probe columns). Headless: List view (sort by header, drag a column wider, inline rename), Icon view (hover scrub + I / O set the clip's In / Out, Shift+] / [), Freeform (drag a card, stack from the card menu, options dialog), panel menu, Metadata Display dialog, view presets, bins in place / tab / window, footer buttons, Media Browser navigation, Favorites, import, Shift+O, Edit Columns…, thumbnails, drag to the Timeline (`FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `project-*.png`, `media-browser-*.png`) |
| Menu long tail (M3.11) | `crates/render/src/scene.rs`, `crates/engine/src/{scene_detect,sequence_extras,project_tools}_tests.rs`, `crates/interchange/src/ale.rs`, `crates/project/src/find.rs`, `crates/ui-egui/tests/menus_ui.rs` | Scene Edit Detection on synthetic frames (hard cuts vs pans, flashes, sensitivity) and on generated movies with known cut points (our own ProRes render of demo scenes; an ffmpeg-made H.264 concat of testsrc2 / SMPTE bars / Mandelbrot): cuts with linked audio, clip markers, subclips, one undo step, background job and cancel; Normalize Mix Track to a target peak; Simplify Sequence; Find / Find Next and search bins; Automate to Sequence (overlap, transitions, insert, unnumbered markers); ALE round trip; Project Settings and scratch disks; menu order and shortcuts against Premiere's; the dialogs by automation id |
| Mixer | `crates/render/src/mixer_tests.rs`, `crates/engine/src/mixer_tests.rs` | sample-exact fader gain and pan laws, automation at block boundaries (bit-identical however requests are cut), solo/mute/solo-safe, sends and submix routing, latency-compensation alignment, render-vs-playback identity, Touch ramp-back, recorder modes (Latch/Touch/Write), thinning, Audio Gain, transition curves; `perf_24_tracks_3_effects_realtime_factor` prints the realtime factor |
| 5.1 / multichannel | `crates/audio-dsp/src/channels.rs`, `crates/render/src/mixer_tests.rs`, `crates/export/src/surround_tests.rs`, `crates/engine/src/mixer_tests.rs`, `crates/ui-egui/tests/audio51_ui.rs` | BS.775 downmix / upmix coefficients exact, mixdown types; 5.1 panner point gains equal-power (Σg² = 1 over a 21×21 grid × 3 centre settings), speaker positions exact, 5.1 through the default puck bit-exact identity; graph: stereo/mono tracks into a 5.1 Mix (L/R, C, rear right, room centre −6 dB each, LFE = mean), 5.1 track through a 5.1 Mix and folded into stereo, a 5.1 submix, `pan51` automation block-invariant, six-channel meters; export: 6-ch WAV (`WAVE_FORMAT_EXTENSIBLE`, mask 0x3F) and ProRes MOV (6 × 24-bit PCM) decoded by ffmpeg **sample-exact**, ffprobe `channels` 6 / `channel_layout` 5.1; AAC 5.1 in MP4: every decoded channel's own tone ≥ 30 dB above the others' (no swapped channels), stereo export of a 5.1 Mix = BS.775 within 2·10⁻⁶; `sequence.settings {mix}`, `file.newSequence {mix, trackType}`, panner lanes, Write pass records the puck; UI: puck drag (one undo step), Center / LFE, six meter bars |
| Audio Clip Mixer automation, Track Mixer view | `crates/engine/src/mixer_tests.rs`, `crates/ui-egui/tests/audio51_ui.rs` | Touch: a 201-point fader ramp thins to ≤ 6 clip keyframes, values within 0.1 dB, live override while held, values outside the gesture unchanged, one undo step; Latch holds until stop; Write records volume and pan from the start; a gesture across two clips writes both; Read commits once without a pass; UI: Clip Mixer mode dropdown, a recorded fader drag writes clip keyframes; Show/Hide Tracks, Meter Input(s) Only, transport record button |
| Voice-over | `crates/engine/src/voiceover_tests.rs`, `crates/ui-egui/tests/voiceover_ui.rs` | synthetic click train lands in `mix_sequence` exactly at R + k·period (pre-roll dropped, exact sample count), punch-in between In/Out (ramp source, every sample checked), one undo step + undo/redo, take numbering, record-arm/targeting track choice, sync/discard/zero-length/locked cases, cue times; UI: right-click Mic → Voice-Over Record Settings dialog (edits persist on OK, Cancel discards), the Mic button starts and stops a take on its track with pre-roll and overlay, and records past the end of the timeline (also `voiceover_playback_tests` in `crates/ui-egui/src/lib.rs`) |
| Text to Speech | `crates/tts/src/**/*.rs` (Kokoro tests need `FILMCRAFT_KOKORO_DIR`), `crates/tts-text`, `crates/engine/src/narration_tests.rs`, `crates/ui-egui/tests/tts_ui.rs` | built-in voices: determinism, finite samples, exact-silence pause markers, pace scales length, measured pitch per voice and per semitone, hostile scripts; engine: narration placed at the playhead with clip length = speech (sample-exact), never overwrites (busy tracks → next free track → new track), occupied/locked/unknown track refused with no change, edit keeps start/length/speed (longer speech: whole speech in the file; shorter: file padded with exact zeros), edits write beside the previous file, undo/redo restore the project exactly, enabled only with a narration clip (or `clip`), 16 hostile parameter sets refused without change, preview changes nothing, save/reload keeps narrations, the narration is audible in `mix_sequence`; UI: type + Add pause + Add to timeline, the new clip is selected and loaded, clicking the clip on the timeline loads its script, Save edits it in place as one undo step, Advanced twirl, panel opens beside Properties; natural voices: 50-sentence Whisper-base WER 2.1–3.6 % for all 9 voices (crate README), loaders fuzzed with hostile files, `tts.render` cache, download refused without the feature |
| Remix | `crates/audio-dsp/src/remix.rs`, `crates/render/src/remix.rs`, `crates/engine/src/remix_tests.rs`, `crates/ui-egui/tests/remix_ui.rs` | generated rhythmic music (Rust-synthesised drums, bass, chord sections at 90–128 BPM, 22.05/44.1/48 kHz): tempo within 0.3 BPM, every beat within 10 ms (measured ≤ 8.9 ms); remix targets from 0.4× to 2.2× the source within one beat (measured ≤ 12 ms at default sliders), every cut on a detected beat, intro and outro kept, pieces ≥ 4 beats, deterministic; silence, tone and noise refused; rendering independent of request cuts, source-exact outside the 20 ms equal-power crossfades, no clicks; the engine's mix equals the source pieces (< 1e-5); undo, redo and revert exact; overlaps refused; Remix Properties dialog and Remix tool drag driven by automation id |
| Oracle | `crates/*/tests/*oracle*.rs`, `conformance.rs` | compare with ffmpeg/ffprobe (§2) |
| Robustness / fuzz | `*/tests/robustness.rs`, `*/tests/fuzz.rs` | seeded mutation and truncation of real and synthetic files; nothing may panic |
| Program monitor while dragging | `crates/ui-egui/tests/perf_ui.rs` | headless: a parameter changed on every UI pass (a value dragged in Effect Controls or Properties) and a backward scrub leave the Program monitor's picture at most one step behind (it shows the frame asked for the pass before while the frame due renders; it used to freeze until the mouse rested); a frame still rendering is kept when the same frame is asked for at a newer revision, so frames slower than a refresh (CPU effects) are shown too; `perf.stats` reports the picture's frame and revision against the playhead and the project |
| Performance | `*/tests/perf.rs` (`#[ignore]`) | §5 |

Commit `*.proptest-regressions` files so failing cases are re-run.

## 2. ffmpeg oracle tests

ffmpeg and ffprobe are **external processes** used to generate fixtures and to check results. They
are never linked or shipped ([AGENTS.md](../AGENTS.md) §2).

- The tools are found by `filmcraft-testkit` (a dev-dependency-only crate, `crates/testkit`), in
  this order: `FILMCRAFT_FFMPEG` / `FILMCRAFT_FFPROBE` (explicit paths; a wrong path is an error),
  then every directory on `PATH` (`ffmpeg` and `ffmpeg.exe`), then well-known install directories
  (`/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `C:\ffmpeg\bin`, …).
- If a tool is missing the test prints `SKIPPED (<test>): ffmpeg not found …` and passes. Set
  **`FILMCRAFT_REQUIRE_ORACLES=1`** (CI) to make every such skip a failure. In new tests use
  `let ff = filmcraft_testkit::require_ffmpeg!();` (also `require_ffprobe!`, `require_oracles!`).
- Some oracles need a recent ffmpeg: the APV tests (`crates/apv/tests/oracle.rs`) skip when
  `ffmpeg -formats` does not list `apv` (ffmpeg ≥ 8.0), naming the detected version.
- Fixtures are generated on first use into `<workspace>/target/fixtures/<crate>/`
  (`filmcraft_testkit::fixtures_dir`; independent of `CARGO_TARGET_DIR`, so agents with private
  target dirs share them; `FILMCRAFT_FIXTURES_DIR` overrides the root) and reused afterwards.
  Generators write to a per-process/thread temporary name (`testkit::temp_path`) and rename it into
  place, so concurrent tests never see half-written files. Delete the directory to regenerate.
  Never commit media.
- Every crate's `tests/common/mod.rs` delegates discovery and fixture paths to testkit, except
  `crates/codecs/src/tests.rs`, which still has its own lookup (the crate was being edited
  concurrently; migrate it when convenient).
- `cargo xtask fixtures [crate…]` pre-generates the whole fixture matrix up front (useful before a
  parallel test run or on a fresh machine). It runs each crate's ignored `generate_fixtures` test —
  the same generators the oracle tests call — for `h264`, `hevc`, `isobmff`, `matroska`, `dnx`,
  `prores`, `mpeg2v`, `ac3` and `codecs` (MXF, Ogg and MPEG TS / PS), and prints one `made` / `cached` / `skipped` line per fixture plus a summary. The
  `aac`, `opus` and `h264enc` oracles generate small per-test signals on demand and are not part of
  the matrix.
- Fixture sources are synthetic: `testsrc2`, `mandelbrot`, SMPTE bars, noise and fades, sine tones.
  H.264 and HEVC fixtures need ffmpeg built with libx264 and libx265. VideoToolbox fixtures are
  generated only on macOS. The Media Foundation parity tests (`crates/platform/tests/media_foundation.rs`,
  Windows only) make their 640x360, 1080p and 2160p H.264 / HEVC / Main 10 fixtures with ffmpeg and skip
  without a Direct3D 11 video device or the HEVC / VP9 / AV1 codec extensions of the Microsoft Store. The VP9 and
  AV1 fixtures (libvpx-vp9, libaom-av1; 360p, 1080p, 2160p, hidden alt-ref frames, two GOPs) need an ffmpeg with those encoders.

The NVENC tests (`crates/platform/tests/nvenc.rs` and `nvenc_export.rs` for H.264, `nvenc_rgba_input.rs` for
the GPU's RGB → 4:2:0 conversion, `nvenc_hevc.rs`,
`nvenc_hevc_export.rs`, `nvenc_hevc_probe.rs` and `nvenc_hevc_warm.rs` for H.265, `nvenc_hevc_main10.rs` and
`nvenc_hevc_hdr_export.rs` for Main 10 HDR; Windows only) skip, printing `SKIPPED`, without an NVIDIA GPU
with NVENC (H.265: when the HEVC probe says there is no HEVC encoder; Main 10: when the Main 10 probe says
there is no 10-bit encoder; on a machine that has one, a configuration NVENC refuses fails the test). The FFI layout tests in
`crates/platform/src/nvenc/abi_tests.rs` were generated from a C program built with MSVC
(`cl` after `vcvars64.bat`) against NVIDIA's MIT-licensed `nvEncodeAPI.h` (12.1); the header and the
program are not in the repository. To regenerate them, put the header in a scratch directory and
write a program that prints, for every type, field, constant and GUID of `src/nvenc/ffi.rs` that the
asserts name (H.264, HEVC and the HEVC picture parameters with their SEI payload array), `sizeof` / `alignof` / `offsetof`, the enum and macro values cast to
64-bit integers and the GUIDs as 128-bit hex; for the bit-fields of the `flags` words, which
`offsetof` cannot take, zero a structure, set that one field to its maximum and print the 32-bit word.
Compare the numbers with the asserts (a difference is drift, which must be understood before
anything is "fixed" to match) and add asserts for anything new.

### Pass criteria per codec

| Crate | Oracle check | Criterion |
|---|---|---|
| `h264` | decode every fixture single-threaded and frame-threaded; compare with `ffmpeg -f rawvideo -pix_fmt yuv420p` | **bit-exact**, every frame |
| `hevc` | same, `yuv420p` / `yuv420p10le` | **bit-exact** (8- and 10-bit) |
| `h264enc` | `ffmpeg -v warning -err_detect +crccheck+bitstream+buffer+explode` decodes our stream | no output at all (a `corrupt decoded frame` warning fails; a negative control checks dropped and truncated slices are reported), exact frame count, **bit-identical to the encoder's reconstruction**; B-frame order checked with ffprobe; rate-control targets |
| `prores` decode | ffmpeg `prores_ks` / `prores_aw` fixtures, compared at native depth | within **±1 LSB** (different integer IDCT); alpha bit-exact |
| `prores` encode | ffmpeg decodes with `-xerror` | no errors; agrees with our decoder within ±1 |
| `dnx` decode | ffmpeg VC-3 fixtures (13 DNxHD CIDs, DNxHR LB/SQ/HQ/HQX/444), compared at native depth | within **±2 LSB**, ≤ 100 samples per million beyond ±1 (ffmpeg's integer IDCT; we evaluate the exact IDCT) |
| `dnx` encode | ffmpeg decodes with `-xerror` | no errors; agrees with our decoder within ±2; HQ luma PSNR ≥ 45 dB |
| `aac` decode | ffmpeg's decode of the same stream | max abs error ~1e-7 (float) |
| `aac` encode | ffmpeg decodes our stream | no errors, no clipping, CBR within ±5% of target; SNR reported |
| `isobmff` | `ffprobe -show_packets` on ffmpeg-made MP4/MOV | packet offsets, sizes, pts/dts, durations, key flags and stream parameters equal; remuxed files decode with `ffmpeg -v error` silent |
| `matroska` | `ffprobe -show_packets` | every packet (stream, size, key flag, pts, duration) equal; seeks land on the latest keyframe ≤ target |
| `audio-dsp` loudness | `tests/loudness_oracle.rs`: signals generated in Rust (997 Hz sine, pink noise at 48/96 kHz, speech-like bursts at 48/44.1 kHz, stereo with silence and sub-gate passages, an fs/4 inter-sample-peak tone) written as float WAV and measured with `ffmpeg -af ebur128=peak=true:metadata=1` | momentary and short-term every 100 ms **±0.1 LU**, integrated **±0.1 LU**, LRA **±0.5 LU**, true peak **±0.2 dB** (against the analytic value when one exists). Measured: ΔM/ΔS ≤ 0.0005 LU, ΔI ≤ 0.007 LU, ΔLRA ≤ 0.04 LU, ΔTP ≤ 0.045 dB against ffmpeg. On the fs/4 tone ffmpeg's own true peak is +0.6 dB high (−0.32 vs the analytic −0.92 dBTP); ours is −0.05 dB |
| `audio-dsp` effects | `src/effects/premiere_tests.rs`, `src/design.rs`, registry tests in `src/effects/mod.rs`: generated tones, noise and impulses | every effect: neutral settings = delayed identity (≤ 2·10⁻⁴), bit-exact determinism, latency constant and equal to where an impulse lands, block-size independence, finite / denormal-free tails at extreme settings, > 1× realtime; per effect a level or frequency-response check (measured tone vs analytic `response_db` within 0.1–0.2 dB, Butterworth vs closed form 10⁻⁶ dB, Chebyshev/elliptic ripple and stop-band bounds, LR4 crossover sum flat within 0.02 dB, comb-notch depth, echo positions to the sample, Schroeder RT60 of generated impulses within 25 %, click repair −20 dB) |
| `mxf` (via `codecs/tests/mxf_oracle.rs`) | ffmpeg-written OP1a / OP-Atom / D-10 files decoded by ffmpeg | H.264 (long GOP, B pictures without temporal offsets) **bit-exact** every frame and at 25 random seeks; DNxHR ±2, ProRes ±1; PCM / AES3 **sample-exact**; frame count = ffprobe packets; timecode (25 fps, 29.97 DF); MPEG-2 long GOP, XDCAM HD422 (1080i 4:2:2) and D-10 / IMX (720×608 4:2:2) within ±4 (`mpeg2v` criterion); truncation/mutation never panics |
| `mpeg2v` | ffmpeg-encoded MPEG-1/2 elementary streams (progressive, TFF/BFF interlaced, 4:2:2 intra and long GOP, 1080i, escapes + custom matrices, MPEG-1) and our synthetic streams with field pictures / 16x8 / dual prime / concealment vectors, decoded by ffmpeg (`-xerror`) | per frame max ±4 and PSNR ≥ 58 dB, ≤ 3 % of samples differing (ffmpeg's integer IDCT vs our IEEE 1180-accurate one; measured max ±1-3, 65-75 dB); frame count, display order, picture types, interlacing and field order **exact** (ffprobe); IEEE 1180 procedure passes |
| MPEG TS / PS (via `codecs/tests/mpeg_oracle.rs`) | ffmpeg-written TS (MPEG-2 + MP2, H.264 + AAC, HEVC + LATM AAC), BDAV `.m2ts` / `.mts` (H.264 + Blu-ray LPCM / AC-3), VOB (MPEG-2 + MP2 / DVD LPCM 16 and 24-bit / AC-3), MPEG-1 system streams, `.m2v`; MPEG-2 in MOV (XDCAM `xd5c`), MP4 (`mp4v`), MKV | frame count and every frame's **PTS exact** (ffprobe); H.264 / HEVC **bit-exact**, MPEG-2 within the `mpeg2v` criterion; random seeks = sequential decode; LPCM **sample-exact**; AAC ≤ 2.1·10⁻⁷, MP2 (symphonia) ≤ 3.2·10⁻⁵, AC-3 ≤ 5·10⁻³ (decoder-specific dither; measured 2.3·10⁻³); random-access audio = sequential |
| `ac3` | ffmpeg AC-3 encodes (2/0 with coupling and rematrixing, 1/0 32 kHz, 3/2+LFE 448 kb/s, noise) decoded by ffmpeg | SNR per channel ≥ 40-60 dB on tonal content (measured 66-94 dB), ≥ 18 dB on noise (27.7 dB, the same as between two of our own dither seeds) |
| `ogg` (via `codecs/tests/ogg_oracle.rs`) | ffmpeg (libopus, Vorbis) files decoded by libopus / ffmpeg | length **exact** (pre-skip, end trimming); SNR ≥ 50 dB CELT (measured 77.7), ≥ 10 dB SILK, Vorbis 139 dB; random seeks within 6·10⁻⁶ of the continuous decode |
| MP4 audio at another rate (`codecs/tests/mp4_oracle.rs`) | ffmpeg-written AAC in MP4 (48 kHz, edit list skipping the priming) | reads resampled to 44.1 kHz in frame-sized requests equal the native decode interpolated at the same times (difference < 10⁻⁴, peak ≤ the source level) |
| image sequences / BWF (`codecs/tests/media_oracle.rs`) | ffmpeg-written PNG/TIFF/BMP/JPEG stills read by ffmpeg's image2 demuxer; ffmpeg BWF (`-write_bext`) | PNG/TIFF/BMP **exact**, JPEG ±12 (measured 3); missing frames hold the previous one; BWF TimeReference = ffprobe's, start timecode, samples exact |
| `export` | our own demuxer/decoder reads the file back; ffprobe counts frames when present | expected size, duration, colour, audio level; exact frame count |
| `export` settings and presets (M6.5) | `export/src/settings_tests.rs`, engine `export_tests.rs`: every built-in preset exports a slice of the demo sequence; ffprobe reads codec, profile, size, rate, audio format and data rate; our decoders read pixels back | codec / profile / container / size / `r_frame_rate` / sample format exact; H.264 bitrate ≤ VBR max × 1.1 and ≥ ½ target over 3 s; ProRes ≤ 1.5 × nominal; DNxHR within 25 % of nominal; loudness normalization within **0.5 LU** of the target (WAV and AAC), true peak ≤ ceiling + 0.1 dB; image sequences numbered `<name>000…`; limiter, overlays, metadata (`title`/`copyright` tags), MOV multiplexer, two-pass/CBR progress |

Each codec README has the full fixture matrix and the measured results.

## 3. Rendering: CPU reference and GPU parity

- The CPU compositor in `filmcraft-render` is the reference. Its tests check computed values
  (opacity blends, dissolve midpoints, Motion placement, ½-resolution against downsampled full
  resolution) and that `render::plan` matches `render_sequence`.
- `crates/gpu/src/tests.rs` composites the same frame plan (a YUV layer plus a transformed,
  semi-transparent RGBA layer) on the GPU and on the CPU. It requires a 99th-percentile channel
  difference of ≤ 6 and a mean of < 1.5 (8-bit levels). It skips when no GPU adapter is available.
- Effects implemented only on the CPU need no GPU test: the plan pre-renders those layers on the
  CPU.
- **Golden images** (`crates/golden`, a test-only crate): `tests/golden.rs` builds small procedural
  projects (demo-generator footage, bars, no media files) at 320×180 and renders one frame of each
  through the CPU compositor: Motion transform + opacity, four blend modes (Multiply, Screen,
  Overlay, Difference), Gaussian Blur, Lumetri basic correction, Crop, Cross Dissolve at 50 %, Dip
  to Black at 25 %, Wipe at 50 %, Timecode / Clip Name burn-in text, graphic clips (also a built-in
  graphics template with overridden properties and per-character styles with a pinned box), and colour
  management: S-Log3/S-Gamut3.Cine footage interpreted into Rec. 709 (`log_to_rec709`) and
  Rec. 2100 PQ footage tone mapped into Rec. 709 (`hdr_tone_map`; the footage is demo frames
  re-encoded by the test's `Encoded` source). Each frame is compared
  with `crates/golden/goldens/<scene>.png` by `filmcraft_testkit::golden`:
  **PSNR ≥ 45 dB, max abs ≤ 12, 99th-percentile per-pixel max channel difference ≤ 2** (8-bit
  sRGB levels; `Tolerance::RENDER`). On failure the actual frame and a ×8 difference image go to
  `<workspace>/target/golden-failures/`. Six `vfx_*` scenes cover the M5.11 effects (Corner Pin over Turbulent
  Displace, Ultra Key, Wonder Glow + Glint, Spacer + Stroke + Long Shadow over Gradient, VR Rotate Sphere,
  Channel Mix + Color Emboss + Rounded Crop); per-effect unit tests live in `crates/render/src/vfx/tests.rs`
  (identity at neutral parameters, determinism, range, a known value each, temporal/track effects through
  the compositor). `scenes_are_distinct` guards against blank or duplicate
  scenes.
- **Video transitions** (`crates/render/src/transitions/tests.rs`) are checked for every transition
  (84 modern, 21 Legacy, 7 hidden Obsolete): p = 0 is exactly the outgoing frame and p = 1 exactly
  the incoming one, near both ends the output is close to that frame (continuity), the midpoint differs
  from both, the output is finite and deterministic (also on 1×1 / odd sizes and NaN progress), every
  parameter visibly changes the picture, and every direction differs. Golden *fingerprints* (4×4 mean
  RGB grid at p = 0.25/0.5/0.75, ±4 levels) live in `src/transitions/goldens.txt`; re-bless with
  `FILMCRAFT_BLESS=1 cargo test -p filmcraft-render golden_fingerprints` and review the diff.
  `FILMCRAFT_TRANSITION_SHEETS=<dir> cargo test -p filmcraft-render contact_sheets -- --ignored`
  writes one contact-sheet PNG per folder for eyeballing (never commit them).
- **Blessing:** `FILMCRAFT_BLESS=1 cargo test -p filmcraft-golden` rewrites the references (and
  writes a `.attribution` sidecar for any new one; it prints the `ATTRIBUTION.md` row to add).
  References are original work rendered by FilmCraft, must stay under 50 KB (enforced on bless),
  and every one needs its sidecar and index row (`cargo xtask assets`). Look at a re-blessed PNG
  before committing it.
- The same scenes run through `plan_frame` + `GpuCompositor` when a GPU adapter exists
  (`gpu_matches_cpu_on_golden_scenes`, same p99 ≤ 6 / mean < 1.5 criterion; skipped otherwise).
  This found a real bug: the GPU upload cache was keyed by pixel-buffer address without keeping
  the buffer alive, so a new frame allocated at a freed frame's address was drawn with the stale
  texture (fixed; regression test `upload_cache_keeps_buffers_alive`).

## 4. UI: control channel and screenshots

### Scripted UI tests (headless)

`crates/ui-egui/tests/scripted.rs` runs the real `FilmcraftApp` under
[`egui_kittest`](https://docs.rs/egui_kittest) (`build_eframe`): no window, no GPU, no OS event loop.
A small `Driver` opens the demo project, sends requests through the same control channel agents use
(`ControlRequest` → `control::handle`, exactly as the TCP server does) and steps egui frames until
each reply arrives. Synthetic input queued by `ui.click` / `ui.drag` / `ui.key` is moved into the
next frame by the app's own `raw_input_hook`, so clicks by automation id work headless. Covered:

| Test | Asserts |
|---|---|
| `demo_project_opens_headless_and_registers_widgets` | active sequence, > 50 registered widgets, panel and tool ids, 6 clips on V1 (`sequence.inspect`) |
| `razor_then_undo_through_the_control_channel` | `timeline.razor` adds a clip, the timeline can locate it on screen, `edit.undo` via `ui.menu.invoke`, redo |
| `insert_from_source_ripples_the_sequence` | `source.open` + marks + `source.insert` lengthens the sequence |
| `apply_effect_appears_in_effect_controls` | `effects.apply` adds Gaussian Blur in the model and `effectControls.effect.gaussian_blur` appears in the panel; undo removes it |
| `playback_toggle_and_stop` | `playback.toggle` / `ui.playback stop` and `ui.inspect` playback state |
| `clicking_a_tool_button_by_automation_id` | `ui.click {id: "tools.Razor"}` changes the tool (real egui input path) |
| `unknown_methods_and_commands_fail_cleanly` | errors come back as `{"ok": false}` |

`crates/ui-egui/tests/mixer_ui.rs` drives the Audio Track Mixer, Audio Clip Mixer, timeline track
keyframes and the Audio Gain dialog by automation id and with multi-frame pointer drags (press, move
over several frames, release). With `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` it also renders the window
offscreen through wgpu (`Harness::render`) and writes `mixer-*.png`; this works without a visible
window (for example on a locked screen, where `ui.screenshot` cannot capture).
`crates/ui-egui/tests/essential_sound_ui.rs` does the same for the Essential Sound panel (type buttons,
switches, a slider drag as one undo step, section bypass, Auto-Match, ducking, Browse presets;
`essential-sound-*.png`).
`crates/ui-egui/tests/clip_audio_ui.rs` covers the audio of video clips (#223): the linked audio's
Volume, Channel Volume and Panner in Effect Controls and Properties, and the Volume line on audio
clips in the Timeline (a drag is one undo step, Pen-tool keyframes, keyframe drags, a click or
right-click on the line still reaches the clip; `FILMCRAFT_UI_SHOTS=<dir>` writes `volume-*.png`).

Essential Sound engine tests (`crates/engine/src/essential_sound_tests.rs`) build projects from
generated speech-like and tonal WAVs: Auto-Match lands within ±0.5 LU of the target (measured: 0.000 LU,
with and without a repair/clarity chain) and measures a mono clip as one channel (3.01 LU below the
same signal as dual mono, within 0.1 LU of a one-channel meter over the file); ducking on a dialogue +
music project gives keyframes within 60 ms of the expected times (measured 20 ms) and −15.00 dB in the
mix; each repair stage improves its metric through the render path (hum −39 dB, rumble −22 dB, noise
floor −13 dB, sibilance −19 dB, reverb tail −10 dB); the mix is bit-identical however requests are cut
and the WAV export equals it.
`perf_full_dialogue_chain_realtime_factor` (ignored; run with `--release`) prints the realtime factor of
all nine Dialogue effects on one clip (22× on one core).

`crates/ui-egui/tests/export_ui.rs` drives Export mode: the settings column and Summary, editing a
setting turning the preset into Custom, the Preset Manager (search, favourite star, save, apply,
delete), the queue panel (Send to Queue, Up/Down, Cancel, Start, Retry, Clear) with real encodes,
the Export button and the header's Quick Export popup (`export-*.png` snapshots).

`crates/ui-egui/tests/color_ui.rs` drives colour features the same way (`color-*.png` snapshots):
Lumetri Input/Look LUT menus and section switches, the Interpret Footage ▸ Color Management and
Sequence Color Management dialogs, and the HDR scopes of a PQ sequence.

Colour science tests: `filmcraft-color` (curve round trips and published reference values per
camera log curve, BT.709/BT.2087 matrices, BT.2390 EETF, gamut mapping, LUT tetrahedral vs a
brute-force barycentric reference, `.cube`/`.3dl` round trips), `filmcraft-gpu`
(`gpu_lut_matches_cpu_tetrahedral`: WGSL vs CPU, max |Δ| ≈ 2e-7), `render::colorman`,
`render::color_match`, engine `color_tests.rs`, and `export::tests::hdr_exports_signal_pq_and_hlg`
(PQ/HLG exports read back by our demuxer and by ffprobe: `color_transfer`, `color_primaries`,
`color_space`, mastering display and content light side data).

Not covered headless: `ui.screenshot` (needs a real viewport), the wgpu monitor path (the harness
runs the CPU texture path), audio output, and wall-clock playback advance (kittest frames do not
advance real time). Playing sound is covered without a device: `play_ahead` tests drive the
mixed-ahead ring with a wall-clock device (order, underruns stay in time, mixer panic, stop), and
`cargo run --release -p filmcraft-ui-egui --example bench_audio -- <project.fcproj>` plays a real
project's sequences through a simulated 48 kHz device and counts dropouts, mixing in the callback
(`direct`, as before) against mixing ahead (`ahead`). Use `Driver` for new UI regressions: `d.exec(command, params)`,
`d.ok(method, params)`, `d.frames(n)`.

### Interactive checks

For visual work, drive the real app:

```sh
cargo run --release -p filmcraft -- --control 9876
```

Then script it over the control channel or MCP: run commands, click by automation id,
`ui.inspect` / `ui.elements` to assert state, and `ui.screenshot {"path": "…"}` to capture the window
or a single panel (`{"panel": "Timeline"}`). Look at every screenshot. Examples are in
[agents.md](agents.md). The headless path (`filmcraft-cli run script.jsonl`, MCP `--demo`) covers
engine behaviour without a window.

## 5. Performance

| Benchmark | Command |
|---|---|
| H.264 decode, 1080p, threads sweep | `cargo test --release -p filmcraft-h264 --test perf -- --ignored --nocapture` |
| HEVC decode, 1080p / 2160p | `cargo test --release -p filmcraft-hevc --test perf -- --ignored --nocapture` |
| ProRes decode/encode | `cargo test --release -p filmcraft-prores --test perf -- --ignored --nocapture` |
| Encoder speed / PSNR / bitrate | `cargo run --release -p filmcraft-h264enc --example h264enc_synth -- …` |
| Decode a real file through the media stack | `cargo run --release -p filmcraft-cli -- bench-decode file.mp4 --frames 120` |
| Coding-tool coverage of the fixtures | `cargo test --release -p filmcraft-h264 --test conformance coverage_report -- --ignored --nocapture` (same for `hevc`) |
| **Whole-app suite**: decode fps per codec, playback, scrubbing, timeline UI, export, project save/open, peak RSS | `cargo xtask bench` ([performance.md](performance.md)) |

`cargo xtask bench` (the example `crates/ui-egui/examples/bench/`) runs six sections, each in its
own process under `/usr/bin/time` so peak RSS is per section: `decode` (every frame of 1080p and
2160p H.264, HEVC, VP9, AV1 and ProRes through the media stack), `playback` (the scenarios below),
`scrub` (random seeks and playhead drags: time until the exact frame is on screen), `timeline`
(the real app under `egui_kittest` with a 1000-clip / 20-track sequence: update, tessellation and
wgpu render ms per frame), `export` (H.264 + AAC and ProRes) and `project` (save / open 5000 clips).
Options: `--sections decode,scrub`, `--only <substring>` (fixture or scenario), `--repeat N`,
`--quick`, `--cpu`, `--hw auto|off` (Settings ▸ Playback ▸ Hardware decoding; default `auto`, as the app), `--label NAME` (output `target/bench/bench-<label>.{json,md}`),
`--section NAME --json FILE` (one section in-process, e.g. under a profiler). Fixtures are made
with ffmpeg in `target/fixtures/playback/` (or `$FILMCRAFT_FIXTURES/playback`). The suite's
`playback` section plays `h264-1080`, `stack3` and `stack3-blend` at Full, `h264-2160` at Full, 1/2 and 1/4 and with
draft decoding (Settings ▸ Playback ▸ Draft decoding) at 1/2 and 1/4, and `hevc-2160`, `vp9-2160`
and `av1-2160` at Full, 1/2 and 1/2 draft; its `draft` column counts the frames decoded in draft
mode.

The perf tests check bit-exactness before they time anything. Results go in the crate README's
performance table, with machine and thread count. Headline numbers go in
[ROADMAP.md](../ROADMAP.md). Measure on an idle machine: parallel agent builds distort timings.

### Playback benchmark

`cargo xtask bench-playback` (the example `crates/ui-egui/examples/bench_playback.rs`) plays
sequences headlessly through the Program monitor's own frame scheduler: the `FrameServer` worker
pool, `schedule_playback` (prefetch order and stale-job dropping), its caches, and the
`PlaybackMeter` that counts shown/dropped frames in the app. On the GPU path it also composites
each plan with `filmcraft-gpu`, as the monitor does on the UI thread.

```sh
cargo xtask bench-playback                                   # every scenario, GPU path, Full and Half
cargo xtask bench-playback --scenario stack3 --res full --cpu
cargo xtask bench-playback --json target/bench-playback.json # machine-readable results
```

| Scenario | What plays |
|---|---|
| `h264-1080` | one 1080p23.976 H.264 clip (testsrc2 + grain, ~45 Mbit/s, 250-frame GOP) |
| `stack3` | three 1080p H.264 clips on V1–V3; V2/V3 scaled, positioned, rotated, 70–85 % opacity |
| `stack3-blend` | `stack3` with V2 in Screen and V3 in Overlay (blend modes on the GPU) |
| `h264-2160` | one 2160p23.976 H.264 clip |
| `demo` | the built-in demo project (procedural footage, transitions, effects) |
| `after-preview` | 1080p H.264 + Lumetri/Sharpen/Levels/Tint: a live play, Render Effects In to Out, then two plays of the green segment |
| `seek-storm` | 40 jumps to random frames 150 ms apart (scrubbing), time until the exact frame shows |

Options: `--res full,half,quarter`, `--cpu` (CPU compositor + texture conversion instead of the
GPU path), `--seconds`, `--refresh` (display Hz), `--workers`, `--repeat`, `--json <file>`.
Fixtures are made with ffmpeg in `target/fixtures/playback/` (set `FILMCRAFT_FIXTURES` to share one
set between worktrees).

Columns: **shown/drop** as counted in the app (a frame is shown when its exact picture was on
screen at a refresh while it was due; frames passed over without a refresh count as dropped);
**ontime** = due frames whose job finished before they were due; **lat** = queue→ready per job,
**svc** = worker time per job; **cpu/j**, **src/j**, **srcC/j** = worker thread CPU, source fetch
(decode) wall and thread CPU per job; **ui** = time on the UI thread to present a frame (GPU upload
and draw, or texture conversion); **cpu ms/f** = process CPU per frame and **cores** = the cores that
needs at the sequence frame rate; **seeks/dec** = decoder restarts and samples decoded; **waste** =
jobs for frames that were never due.

Wall-clock columns (shown/drop, ontime, latencies) depend on machine load, so each row prints the
load average. CPU columns (thread and process CPU time) and the structural counters (seeks,
samples decoded, wasted jobs) do not, and are what to compare between runs on a busy machine.

Results on an M4 Pro (14 cores), GPU path, 8 s plays, median of 2 alternating runs of the M4.6
baseline (commit `5170376`) and the result of M4.6, on a machine shared with parallel agent
builds (**load average 86–207**, so wall-clock columns are pessimistic; CPU ms/frame is not):

| Scenario | shown/dropped before | after | CPU ms/frame before → after | decoder seeks |
|---|---|---|---|---|
| h264-1080 Full / Half | 106/86, 118/74 | **192/0, 192/0** | 80 → 46, 95 → 46 | 3–4 → 1 |
| stack3 (3 × 1080p) Full / Half | 185/7, 174/18 | **192/0, 192/0** | 136 → 109, 117 → 111 | 4 → 3 (one per source) |
| h264-2160 Full / Half | 0/192, 0/192 | 5/187, 19/173 | 110 → 135, 170 → 145 | 4–5 → 1–2 |
| demo Full / Half | 63/129, 124/68 | 97/95, 152/40 | 140 → 80, 93 → 40 | |
| after-preview, 1st play after render (Full) | 80/112 | **190/2** | 119 → 26 | 104 → 0 |
| after-preview, 2nd play (Full) | 145/47 | **192/0** | 76 → 26 | 132 → 0 |
| after-preview, live effects (Half) | 18/174 | 69/123 (frames skipped evenly) | 109 → 95 | |
| seek-storm (40 jumps, 150 ms each) | 0/40 shown | 6/40 | 827 → 666 per jump | |

At load ~25–60 the same final build plays h264-1080, stack3 and h264-2160 at Full with 0 dropped
in 3 of 3 runs (192/0) and render previews 192/0. The 4K fixture needs ~4 cores of decode per
real-time second (≈160 ms CPU per frame at 170 Mbit/s); the demo project's procedural footage
~190 ms per Full-resolution frame.

### Audio mixer throughput

`perf_24_tracks_3_effects_realtime_factor` (24 tracks, 72 inserts and a compressed submix on one
core) must run at least 4x realtime in release builds (1x in debug). On Unix it times the test
thread's CPU time, so it runs with the rest of the suite. Hosts without a per-thread CPU clock
(Windows) fall back to wall time, which depends on load, so there it is ignored by default; run it
explicitly with
`cargo test --release -p filmcraft-render --lib perf_24_tracks_3_effects_realtime_factor -- --ignored --nocapture`
and record the machine with the result.

## SSD audio-parameter engine parity candidate

Eight synthetic engine cases exercise the existing `filmcraft_set_parameter` native operation against the shared human `effects.setParam` for the six graphical audio-effect editor definitions. They compare complete project/editor collateral, second-duplicate targeting, authored finishing definitions, exact source-clock mapping across static/retime/reverse/hold modes, replay receipts, guarded Undo/Redo and format roundtrip. Boolean controls and rejected/stale audio requests cover type/clamp refusal and preserved human Redo. These are authored-state engine fixtures using generated media declarations; they do not decode media, drive widgets, listen to audio, establish every parameter/compound gesture, qualify track inserts/Essential Sound, change the installed app, or finish all14-family/product acceptance. Results remain pending compilation and execution.


### Exact Poema mixer insert structure differentials

The bounded native track-family unit group includes actual human `mixer.addInsert` and `mixer.removeInsert` comparisons for audio, submix and master strips. It covers first/middle/last insertion, native registry defaults, retained neighboring animation, complete project/editor collateral, one-step history, replay, guarded Undo/Redo, and format roundtrip. Hostile cases preserve native state and human Redo on invalid slot/clamping, absent/implicit targets, unknown or wrong-kind definitions, native capacity, empty stacks, locks, stale digests/tickets/scopes, and mixed/nonexact batches. Tests use generated demo declarations, never decode or audition real media. These checks do not qualify an installed desktop or replace full application acceptance.


### Shared native strip/send services

Human `mixer.setStrip`, `mixer.addSend`, `mixer.setSend`, and `mixer.removeSend` prepare owned native changes and apply them through shared pure sequence services. Preparation has no project/editor/history side effects; the command retains its existing one-step history label and return data. The existing human optional/default/clamping behavior remains; a future exact agent wrapper must validate scope, locks, field ranges and observed targets before calling these services. Send index conversion uses checked native usize conversion.

`mixer::settings::tests` covers native field values, clamping/default behavior, the master's actual persistence boundary, independent cloned-sequence application, send static values versus retained automation, full send lane renumbering, complete Undo/Redo and format roundtrip, and graph/capacity/index refusal preserving the original human history branch. Existing track, parameter, mixer, hostile-input, and finishing groups remain regression requirements. This refactor does not implement live recorder/editor-state tools or their preference/device/output ownership.


### Exact shared mixer routing bridge

The native track group adds six mixer-routing cases: actual human audio/submix/master settings, complete output graph collateral, send creation/settings preserving native automation, removal with lane remapping, hostile values/ignored master fields/aliases/graph refusals preserving Redo, and stale/locked/full/nonexact/foreign request refusal. They use the same shared native services and human command IDs, compare full project/editor state, and retain receipt replay, one-step Undo/Redo and format roundtrip checks. They do not decode real footage, audition, start a live recorder or change preferences/devices.

Shared idle mixer automation services (development draft) delegate the original human control, keyframe set/delete/move, clear and point-write commands to sequence-only functions. Eight generated regressions cover mode/static/base distinctions, full key payload retention and native neighbor clamps, last-key value, playhead clear semantics, sample-rate boundaries/thinning, missing-key rejection without consuming human Redo, and unchanged live recorder dispatch. This does not add an agent lane projection or live recorder ownership; those remain required. Qualification is pending.

Native authored mixer automation projection (development draft) exposes bounded lane inventory and separate key pages under original document tickets and whole-track digest. It preserves stored/default/orphan storage identities and full key interpolation/influence fields, including lanes too large for the old complete-insert projection. Existing finishing validation is shared. Eight synthetic tests cover read-only audio/submix/master state, stitched keys, a 5000-key lane, shadowed/alias/default storage, hostile bounds/types, stale and replaced documents, and refusal of source/write/live-recorder authority. Composer handles and exact mutations remain pending; this is not full application qualification.

Exact mixer automation authoring draft adds six gestures to original agent.track, delegating to the shared human idle services. Explicit observed clock, complete inventory identity, current strip lock, actual Float/animatable widget definitions and exact native values/neighbor bounds are checked before publishing. Receipts count inserted/removed/changed keys, stored/static value changes, lane removal and explicit point thinning. Eight generated native cases compare complete human project/editor state, receipt replay, native Undo/Redo and format roundtrip, including actual current DSP menu Float widget definitions and separate typed Bool/Choice insert controls. Host authoring tools and full application qualification remain pending.

Authored automation guard draft also verifies the resulting strip remains inside complete native lane/key/value read bounds before the human command can publish. Original-session live mixer/clock ownership is checked both before staging and immediately at publication; staging has no live preview connection. Two regressions cover an oversized point write and a human touch starting after staging without consuming/releasing that human gesture. Host authoring remains withheld until qualification.


## Shared idle Clip Mixer service draft

Human `clipMixer.set` delegates to the same pure clip-control preparation/application service. The original source_time_at clock (including holds independent of Hold Filters), native end clamps, animation/keyframe policy, parameter/value clamps and first matching effect remain human behavior. Human begin/continuation keeps its original merge label/key; preparation happens before resetting a drag boundary and failed commit restores it. Eight generated regressions cover source clocks, payloads, complete history/serialization, malformed targets and post-preparation failure. Native compilation and qualification are still required. No new agent adapter or desktop acceptance is claimed.


## Exact idle Clip Mixer alias draft

The original `agent.setParameter` accepts a strict optional `control:"clip_mixer"` discriminator. Its explicit native capability distinguishes this source_time_at control from the unchanged effects.setParam/effect_time_at entry. Only the first present native volume.level/panner.balance on an enabled unlocked current audio clip qualifies; no masks, range clamps or later duplicate fallback. The shared original clip service builds expected state, and the original human clipMixer.set begins one gesture before the existing whole-project audit and parameter receipt. Real Session live controls/Recorder are checked at original staging and publication. Eight generated alias regressions are pending qualification; no desktop or host-composer acceptance is claimed yet.


Original session settings service (candidate): `mixer::editor::tests` compares
all five clip modes and every native transition default with the original
human handlers. The default-transition command keeps its journal=true flag and
its normal outer command lifecycle; clip mode keeps journal=false. The service
owns neither Project nor Undo/Redo, and tests preserve current live/recording
owners, unrelated editor state, aliases, failed-input Redo and saved formats.
The guarded composer state-only adapter and normal reopen durability remain
pending. No normal desktop, real-media or full-qualification claim is made.


Shared opaque editor proof (draft): `agent_bridge::editor_proof::tests` retains
native positive/negative i64 clocks, original private byte serialization,
clipboard hashing without exporting its payload, exact schema/after-state
validation and UTF-8 byte bounds. The original eight Root navigation and
version-four checkpoint tests remain unchanged and must pass alongside the
shared editor-state and all earlier mixer cases. No new model hand, restored
state, source permission or project history is created by this validator.


Original editor-settings read (draft): `agent_bridge::editor_settings::tests`
checks the actual native mode/default values through existing explicit Root or
qualified sequence read purposes. Pages preserve absent versus explicitly
stored modes; orphan/foreign targets grant no handles. Full editor/clipboard
context is checked even when project revision is unchanged. Current live owners
are preserved for a still-current ticket; original fresh-snapshot/mutation idle
guards remain unchanged. This metadata read does not change settings, renew
source witnesses, mint tickets, grant media access or create history. Setting
mutation, its retained native receipt, the composer adapter and normal app
qualification remain pending.

The settings read requires `beforeEditorSha256` from the same observed native
read. Root reads already expose that digest; sequence snapshots now expose the
same additional metadata field under the new `editorSettingsRead` capability.
The digest includes private clipboard hashes without exporting their content.
The existing five document-projection/source-validation regressions are also
required: the new metadata field must not renew media or editing authority.

### Guarded original editor settings (component qualification pending)

`agent.configureEditor` takes an exact independent Root metadata ticket, the complete
project/editor-and-clipboard basis, and one typed `clip_mixer_mode` or canonical
`default_transition` change. The original human command runs the shared service;
its original journal behavior, project revision and Undo ownership are preserved.
Defaults support a Root with no sequence; modes require the actual owning audio
track. The bounded Root transition catalog reads the original effect registry.
A current pre-recording ticket may change the same controls while original live
owners run; fresh snapshot and all original mutation idle guards remain intact.
Changing a default during trim playback refuses explicitly because the journaled
human command may otherwise settle a timeline edit. This compound case remains
open for complete tool parity.

Editor-setting receipts require native history capsule **version 5**, separate
from the unchanged Save journal format. Formats 1–4 remain supported for their
original receipts. V5 retains mixed Root navigation and genuine project-history
receipts, validates the exact shared human state transition using bounded opaque
proofs and native i64 clocks, and never replays settings or adopts Undo ownership.
Downgrading V5 settings into an older format must refuse before restoration.
The composer adapter and actual cross-language setting qualification remain pending.


## Shared native trim settlement

The native pre-command trim settlement service is shared with the original human dispatcher. An immutable preparation retains exact dynamic/loop owner identity, private clocks, project Arc, edit points, shift, revision, saved state and the complete existing Undo/Redo graph. A cancelled and restarted identical trim is a different owner. The preparation describes the original zero-offset restoration and nonzero history effects without claiming an agent owns the human trim. It preserves the original five live-command exceptions and trim-loop behavior. Targeted tests cover actual human default dispatch, original history limits, native Undo/Redo, exact original Arc restoration, restarted owners, private clock changes and stale history; the original trim, trim-operation and prior selected native regressions remain required.

This is the shared native service prerequisite. The compound guarded editor-setting receipt, ticket capture, host persistence/readback and actual combined composer acceptance are still required before active-trim default changes can be exposed. Existing guarded settings continue to refuse that unsupported compound case. No normal app adoption or full qualification is implied.


## Live editor read purpose

The explicit `agent.readEditorControl` read observes current controls and retains a prepared trim owner on the original native ticket. It does not relax `fresh_snapshot` or the original idle requirement for project/sequence/media planning. A finite editor-only hand allows the existing settings/catalog metadata reads, receipt lookup and read cancellation; project, source, timeline, history and persistence operations cannot borrow this purpose, even after the live gesture stops. Ordinary, library, restored and restore-operation tickets receive no prepared live owner. The native descriptor is bounded and carries sourceValidation=not-performed, mediaEvidence=false and writeAuthority=false; it advertises no compound writer.

The native cases cover genuine empty Roots, actual regular/ripple/roll/loop gestures, recording/preview preservation, original idle refusals, finite-hand refusal, identical restarts, token/path/scope/legacy/cancellation checks, original256-ticket eviction and read-only lifecycle projections. The compound write, opaque outcome/persistence and original host integration remain required for B3.


### Native compound editor history foundation (Native66)

The shared original trim service captures bounded historical descriptions and strong
project snapshots without publishing a setting, stopping a gesture, or granting Undo
ownership. Its projection uses the original commit-label decision and original native
history push. It preserves the dynamic-before project and discarded Redo even when
the human Undo limit is zero; zero-offset restoration retains distinct Arc identities
when project bytes match. Historical decoding has no route back to a live prepared
owner. The future compound receipt/checkpoint adapter must preserve the injective
local-to-capsule snapshot mapping and validate its independent scope/editor proofs.

Generated native tests compare the projection with actual human default commands
for all three trim kinds, zero/nonzero offsets, loop-only and idle states, and Undo
limits 0, 1, and 128. They also exercise exact i64 encoding, missing/forged outcomes,
malformed input, identity collapse and complete snapshot bounds. This foundation
does not itself implement compound wire publication, capsule6, the host adapter,
or full product qualification.


### Explicit live-control change and capsule6 candidate (Native69)

The original Ticket store now distinguishes the existing read-only live-control
read from an explicit version2 change preparation. The typed compound command
precomputes bounded exact editor/history testimony before one independent Stop
fence. Transition defaults use the shared original trim settlement and original
human command; clip modes use that same human service while preserving the live
trim, recording and preview owners. A human trim commit is disclosed separately
from agent timeline commits and owned Undo, including the native zero-Undo case.

Historical evidence1 retains its prior bytes and strict rules. Explicit evidence2
describes preservation for the non-journaled clip-mode command. Both are descriptive
and cannot reconstruct a live owner. Compound receipts require capsule6 with an
injective local-to-capsule snapshot map and complete strong snapshot closure. Old
formats1..5 remain distinct. Cold restoration validates recipes and exact opaque
proofs, retains descriptive outcomes, and preserves the current editor rather
than replaying a setting or a trim. Unknown versions, mixed purposes, downgraded
evidence and detached or collapsed snapshot mappings must refuse atomically.

The candidate's generated tests cover all original selected regressions plus
human parity, live owners, Stop, stale private state, receipt reconciliation,
actual hot/cold native-file restore, mixed older receipts and owned Undo, exact
i64 testimony and forged recovery data. Native transport, original host/composer
port and full product qualification require their separate acceptance evidence.


The subsequent Native70 draft adds a private native history digest to the explicit
change-purpose read and compound receipt. Targeted settings/catalog reads on this
purpose revalidate its complete retained owner, preventing a history-only change
from paying readback against stale metadata. The original read-only version1
projection remains unchanged. The history descriptor keeps labels, merge keys and
project identity relations inside an opaque digest, with explicit Undo/Redo counts
and the actual native limit. A zero-limit commit remains explicitly disclosed even
when retained before/after digests match. This draft supersedes disposable Native69
protocol2/capsule6 fixtures; neither candidate has been adopted into the normal app.

## Original human clock following preparation

The selected clock cases compare all three trim kinds and native history limits
against the original human default command at publication time. They also cover
loop/restarted/ended owners, clamped shuttle motion, clock-derived selection,
no-op and real-tick rejection of foreign project/revision state, exact native
history identity, skipped clipboard data, preferences and unrelated editor
changes. Existing strict control/recovery cases remain part of the regression
set. This component is separate from the pending live wire purpose, current
composer integration and continuous GUI-clock acceptance.

Following editor-control metadata is an explicit native purpose: `clockPolicy: "original_human_trim"` on `agent.prepareEditorControl` returns read version3. Version2 settings/catalog metadata requests bind the ORIGINAL editor digest and return separate original/current control documents through the original catalogs. Existing read1/prepare2 and unversioned metadata stay strict. The live version3 purpose advertises only its finite shared setting publisher; it cannot borrow an older write or any project/source/history operation. Original causal-owner validation and independent Stop remain required. Tests cover actual original trim ticks and separate original/publication bases.

The following-publication component consumes explicit version3 ORIGINAL digests, validates current causal owner evidence and uses the original compound setting publisher with one independent Stop fence. Its bounded receipt separates the original read, actual publication read, native publication receipt2 and original opaque editor/history evidence. Retained roots include both observations. Legacy version2 stays strict. The original SDK/controller/composer port has separate generated-protocol acceptance; the exact actual native/composer pair still requires qualification before normal adoption.

Following history closure uses capsule7 with separate explicit publication/original snapshot maps and native opaque original revision/saved-revision evidence. The shared bounded control-document encoder reconstructs both historical reads; validated native trim recipes compare exact baseline, points, shift and history identity, and mode preservation remains distinct from DEFAULT forecasts. The same original/publication historical validator runs before the native publication fence and after capsule loading. Restore keeps historical outcomes only: no live Following, settlement, source witness or new Undo grant. Existing capsule encodings remain selected for legacy-only graphs; capsule7 can carry mixed legacy receipts without relabeling them. New writes remain unadvertised pending warm outcome readback and original composer integration. Tests broaden the former provisional-refusal case to actual inclusion and add10 generated cold/hot/changed-human/mode/loop/equal-byte-identity/forgery/mixed-legacy/purpose cases. This is bounded native component work; full qualification and actual-model/GUI acceptance remain open.

Following outcome readback retains a private post-setting binding on the original committed Ticket, precomputed before the same original publication fence from its actual in-memory Prepared and human editor/history forecast. Native `agent.readEditorControlOutcome` binds complete receipt hash and exact Root, returns documentary read1 current controls/history/target without creating any read ticket, and distinguishes same clock lineage, same current basis, changed since setting, historical-only or exact idle current. Reopened receipts never restore the binding. Cold exact idle matching requires actual retained after-project Arc, full editor/private clipboard and history identities; a matching target value alone never qualifies. Writes remain unadvertised until original SDK/controller/composer integration. Twelve generated tests cover all3 trims/all5 modes, history limits, loops, later human/epoch changes, old-purpose rejection, Stop/no replay, no writer/source/Undo promotion, cold historical/idle matches and preserved recording/preview. GUI/real media/provider/cadence and full qualification are held.

Live following advertisement is recorded in opaque originalBasis inner version2 with `following_write:true`. Historical inner version1 omits the flag and reconstructs unadvertised read3 documents exactly; a strict canonical decoder rejects inconsistent versions, flags and read capabilities. Both use the unchanged outer opaque envelope and capsule7 dual snapshot maps. Legacy-only capsule1-6 encodings remain unchanged. Restored records never reconstruct private Following, settlement, observation or post-setting binding. Source-format compatibility tests retain native graph identity; actual older-producer interchange remains a separate acceptance gate.


## Shared original sequence view state services

Close Sequence, Close Other Timeline Panels and Move Sequence Tab now use shared
state transitions beside the existing Open in Timeline state service. Human
parameter parsing, enablement, pre-command trim settlement, journal flags, native
events, missing-tab behavior and end clamping remain at their original boundaries.
Generated regressions cover state collateral, exact native clocks, idle command
equivalence, hostile inputs, live trim/loop history and nullable active views.
This service refactor grants no new agent purpose or navigation authority; native
qualification and the finite original-composer view integration remain required.


## Original sequence-view preparation

The separate native view read retains complete sequence membership and exact tab order on the original Root Ticket. Its original/current observation follows only the same actual human trim clock through the shared native clock checks. Defaults and views keep distinct finite constructors. Neither query publishes a view, grants source/timeline/settings/history access, restores a live owner, or adds a second queue/store/composer. Generated regressions cover empty and foreign-active views, all original trim kinds and loops, private-owner restarts, exact original digests, hostile fields, Stop, eviction and complete-inventory bounds. Native qualification, finite view publication/history, original SDK/Root controller/composer and full acceptance remain required.


## Shared native sequence-view publication draft

Four typed view operations use the original human state services, shared native pre-command trim/history evidence, and one compound Stop/publication service. Version1 view reads stay readonly; an explicit version2 change preparation is required. Receipts distinguish original-read and actual-publication bases and own no human trim Undo. Root-only capsule8 adds separate view maps while absent fields preserve earlier encodings. Restored records carry immutable outcomes without a live Following or Prepared. Generated paired-human and cold-companion checks cover clocks, events, selections, nullable historical views, strict purpose separation, Stop, rehashed contradictions and retained human history. Companion restoration preserves the fresh editor; full current-editor/application restoration remains the separate unfinished C2-C4 requirement. New behavior is draft until bounded native and actual original-composer qualification; ordinary application adoption, providers and full acceptance remain held.
