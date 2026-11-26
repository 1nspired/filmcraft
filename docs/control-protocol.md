# Control protocol & MCP

## Original reaction reference inputs and handoff

`agent.reactionReferenceInputs` reads current original source-event coordinates.
Arguments are a current `token`, explicit workspace `scope`, `originalTimeline`,
and 1–512 `queries`. Each query has a distinct 64-hex `key`, bound original
`assetId`, distinct original `componentIds`, and nonnegative `sourceTicks`.
The optional `projection` mode requires separate completed source validation.

The reader uses the existing native clip clock rather than old sequence times.
Rows report `mapped`, `ambiguous`, `outside_current_source`, `component_absent`
or `unsupported_clock`; complete current matches retain clip/identity, track,
record ticks and the native observed source tick. Held clocks have no unique
event position. Static forward/reverse and fractional speeds follow native
rounding. The result binds scope, token, revision, sequence, original timeline,
request and full-project hashes. It is read-only, not semantic verification.
Queries cap at two MiB, 8,192 component references and 4,096 total matches.

`agent.importReactionReferences` is a private form inside one exact `agent.apply`
batch, not a standalone command or model tool. It requires current project and
marker hashes, original plan/input hashes, and 1–512 complete cues. Every cue
retains its source query, chosen current clip/identity and measured record tick,
original name and documentary reference comment, and an explicit native color.
The publisher remeasures the source event and refuses ambiguous or changed
coordinates and an already referenced original cue. Shared human marker
services add the complete batch in one native Undo step. Stop and late failure
discard staging. Only the marker collection and ID allocator may change;
picture, audio, captions, processing and editor context remain unchanged.
The receipt retains original/native marker identities, positions, exact text
hashes and before/after project/marker hashes. Parameters cap below four MiB.

## Current picture fitting inputs

`agent.pictureInputs` is a private read with a current read `token`, explicit
workspace `scope`, a `dialogueTrack` video track ID, `pictureTrack` video track ID
or explicit null, and 1–64 `bites`. Each bite contains a distinct 64-hex `key`,
an original `assetId`, and half-open `sourceInTicks`/`sourceOutTicks`.

The read intersects those windows with current tracked native instances using
`TrackItem::source_time_at`. It preserves fractional starts, static speed,
reverse and frame holds. It returns current pieces, linked original audio
geometry/channels, complete target picture-track content and a full-project
basis digest. Overlapping bites are unioned for totals. It never restores a
saved logical passage, changes a cut, seeks, decodes or changes native history.
Animated remapping/remix need a separate renderer-input contract and refuse.

`linkedAudioTicks` measures linked authored geometry, including explicitly
reported mute/enable/mixer state; it does not measure audible speech. Both
`semanticVerification` and `signalVerification` are false, as is `writeAuthority`.
The optional native `projection:true` form also requires final source validation;
the Poema client accepts the fully validated form and rechecks its original
snapshot after delivery. Missing/ambiguous originals, unsafe clocks, stale reads
and incomplete output refuse. The complete reply is bounded to 2 MiB and 4096
matched picture/audio pieces; nothing is silently truncated.

This is input machinery for Poema's shared original picture planner. It does not
expose a model tool or publish picture overlays. Original-owner choice/analysis
qualification, fitting transport, publication, output review and history remain
separate requirements.

## Exact marker edits and removal

`engine.execute` dispatches `markers.edit` with one positive `marker` ID and
optional `target:"program"|"source"` (default Program). Supplied fields are
`time:ticks` or `frame:integer`, `duration:ticks` or `durationFrames:integer`,
`name`, `comment`, and `color`. The ID and marker kind are retained. Times are
nonnegative exact native integers; frame conversion uses the sequence rate or
the open source's rate. Unknown fields and duplicate representations refuse.

`markers.remove {target?,marker}` deletes only that ID in the chosen collection,
including when several markers share the same start. Missing, foreign or
duplicate IDs refuse. Neither command seeks or selects; each successful gesture
is one native Undo step. Source range relocation preserves duration and refuses
an out-of-bounds range. Existing Source duration-only editing may clamp at its
end; callers requiring exact duration must verify the resulting authored state.

These are native human/control commands, not authorization for a model to send
raw commands. A composer integration still needs scoped reads/targets, guarded
publication, receipts, exact readback and owned history recovery. Native marker
comments/presentation do not establish Poema cue evidence or resolve human notes.

## Current document and source qualification

`documentProjection:true` advertises two private host commands.
`agent.projectDocument {token:freshRandomString,basis:priorReadToken,mediaScope}`
reads current native geometry/context without opening files. The explicit scope
must be covered by native expected generations from the prior ticket, with the
same saved path, source declarations and original bindings. Expired bases or
uncovered/changed declarations return a closed `needs_qualification` reply with
no ticket; a host may then request a fresh qualified snapshot. Other refusals
must not silently downgrade.

A successful document has the normal canonical native shape plus
`documentProjection:{expectedGenerations:true,sourceValidation:"not-performed",writeAuthority:false}`.
Every file row is `unverified`, and library qualification is false. This creates
fresh document/context coordinates, not fresh footage. Expected generations
remain private native witnesses, and publication/perception guards still check
the actual originals. No decoder cache or observed-media cache is refreshed by
this projection. Unknown sources cannot acquire evidence or insert handles.

For media-backed work, start a dedicated `agent.validateSnapshot` job for that
token and obtain its complete native delivery. Then call
`agent.qualifiedDocument {token,validation:job}` within the existing 250 ms
delivery window. It requires the exact unused document and full current editor
context, consumes that one native delivery and returns canonical native rows
with source qualification. The host must not relabel its earlier JSON as fresh
evidence. Apply/candidate/inspection jobs, stale context, incomplete/expired or
already consumed deliveries refuse. No new edit ticket, project/history change
or bypass of final publication checks is introduced.

## History availability metadata

`historyAvailabilityProjection:true` advertises the host's ability to display
the ownership flags from `agent.readHistory {projection:true,token,commit,undo,scope}`
without a separate media sweep. The native COW/revision/path/sequence, full
editor context, explicit owning scope and actual history-head checks remain.
The reply's `requiresSourceValidation:true`, `writeAuthority:false` and
`execution:"not-applied"` markers must be retained. It reports ownership only;
executed Undo/Redo still performs its existing complete source checks before
publication. Source-qualified history testimony still requires the separate
background validation and final metadata recheck. No model tool, edit ticket,
history adoption or project mutation is added by this availability contract.

## Private history persistence basis

`historyCheckpointMetadataBasis:true` advertises the host-only
`agent.readHistoryCheckpointBasis {token:freshRandomString,scope:{projectId,anchorSequence}}`.
It captures the native COW project/history identities, revision, saved path and
editor context without opening footage. The closed `native_history_checkpoint_basis`
reply declares `sourceValidation:"not-performed"` and `writeAuthority:false`.
Its token is separate from the read-ticket cache; it grants no image, source,
timeline-edit or restore authority. Sixteen weak bases are retained, with no
extra project snapshots or persisted source proof.

Only `agent.saveHistoryCheckpoint` accepts that token with `metadataBasis:true`.
It checks the exact current native basis, validates the existing saved-project
path and exclusively publishes the same bounded immutable history companion.
The native worker, independent Stop fence and compatible host's bounded wait
apply. A successful save consumes the attempt; a lost reply cannot be replayed.
Saving an offline project is valid and does not certify its media. Restoration,
inspection and actual edits still require their existing fresh source checks.
Older fully qualified checkpoint saves remain supported. The original project
file and native history are unchanged by either save path.

## Explicit native source dependencies

`agent.snapshot` optionally accepts `mediaScope:{sequences:[itemId],sources:[itemId],assets:[{projectId,assetId}]}`
in both synchronous and background modes. Missing arrays default empty. The
active sequence and open bound original are always included. Nested sequences,
subclips and disabled/muted track references are traversed with strict cycle,
identity and inventory bounds. Omitting the object retains complete bound-original
library verification; invalid/null explicit scope never triggers that fallback.
Saved condense members and immutable physical-member bases are also dependencies
of a selected sequence, including originals absent from its visible pieces.

`scopedSourceVerification:true` confirms the new contract. Snapshot `mediaScope`
contains `kind:dependencies|library`, sorted selected original-file `items` (null
for library) and `libraryVerified`. The latter describes complete **bound-original
file** coverage only. Each source declares `fileVerification:verified|unverified|unavailable|generated`.
Unselected sources remain unverified discovery metadata. No private file stamp or
hash is exposed or serialized into the project.

Validation, preparation, retained candidates, output inspection and final native
publication preserve the relevant witness set. An insert, source mark, inactive
sequence change, Undo or Redo cannot acquire an unverified source by copying its
metadata: take an explicitly expanded fresh read. Native admission retains its
separate complete-byte proof. Source selection/navigation remains metadata-only
and does not claim renewed file freshness. See the [engine contract](../crates/engine/README.md#explicit-source-dependencies).
This private API does not automatically enable scoped reads in an older composer.

`agent.readSourceCatalog {scope:{projectId,anchorSequence}}` provides bounded,
I/O-free metadata for a compatible host to select the next read's dependencies.
Its closed version-one `native_source_catalog` reply includes native session,
revision, saved project path/dirty state, active sequence and source rows
`{item,name,binding,file}`. It explicitly declares `requiresSourceValidation:true`
and `writeAuthority:false`; it issues no read/edit token and includes no original
file paths or file proof. Oversized or malformed complete metadata refuses rather
than truncates. Recheck its basis and bindings against the fresh guarded snapshot.
Snapshots advertise this service with `sourceCatalogProjection:true`.

`guardedOperationDeadlineMs:45000` advertises the existing native verification
budget. A compatible host may separately allow up to 47 seconds for the guarded
publication response when `operationCancellation` and `sourceFileGuard` are true.
The native deadline remains 45 seconds. Other control reads keep their own short
timeouts. Stop and uncertain replies retain the independent cancellation fence
and receipt reconciliation; a longer wait never authorizes replay or rollback.

`historyCheckpointOperations:true` additionally confirms that private checkpoint
save and restore use the same bounded Session worker and independent operation
fence. Compatible hosts may use the guarded 47-second response wait for these
two commands as well. Stop before publication prevents the exclusive checkpoint
file or history restoration; late Stop cannot certify rollback. Checkpoint save
consumes that operation attempt, and an uncertain immutable-file outcome must be
inspected rather than repeated. The UI retains its inert display projection
while the worker owns the real Session. Original-file validation is unchanged.

## Exact caption destination and import

Native snapshots advertise `exactCaptionImport` when the scoped transfer port is
available. `agent.readCaptionDestination` accepts a current `token`, explicit
`scope` (`projectId`, `anchorSequence`) and `sequence`. It returns the complete
bounded destination projection, `layoutSha256`, `captionSha256`, nullable
`archiveSha256` and native forward-clock qualification. It can read an inactive
workspace sequence without navigating, editing, saving or claiming fresh speech.

`agent.importCaptions` is private inside a one-operation exact `agent.apply`, not
a standalone registry command or arbitrary file executor. Its parameters are the
same scope, all three before digests, `sourceSha256` and structured exact caption
`data` for the explicit destination. The native human `captions.importData` door
performs the same import and records original UTF-8 provenance. Changed clocks,
scope, layout, cues, archive, duplicates and exhausted IDs refuse before
publication. Receipts identify `caption_import` and the destination; the outer
batch sequence still identifies the unchanged active view. Reconcile lost
replies through the original receipt; never re-import to recover a response.

## Desktop control channel
`filmcraft --control 9876` (or `FILMCRAFT_CONTROL_PORT`) listens on `127.0.0.1:<port>` (loopback only).
One JSON request per line → one JSON reply per line. The app opts out of macOS App Nap
(`apps/filmcraft/src/app_nap.rs`): a hidden window would otherwise drop the whole process to
background priority, and on a busy machine it would stop answering.

**Only requests are read.** Every line must be a JSON object with a string `method` (`id` and `params`
are optional; blank lines are skipped). Anything else gets one error reply
(`{"ok": false, "error": "… closing the connection"}`) and the server **closes the connection**, so
nothing sent after it on that connection runs. That covers text that isn't JSON, a JSON array or number,
an object without `method`, invalid UTF-8, and a line longer than 4 MiB. An HTTP request (for example a
web page's cross-origin `fetch` to `127.0.0.1:<port>`) therefore can't smuggle a command in its body:
its request line is rejected first. At most 16 connections are served at once; further ones get an error
line and are closed. Clients that get an error reply should reconnect. The port has no authentication,
so only enable it while you use it. Transport: `apps/filmcraft/src/control_server.rs`.

```json
{"id": 1, "method": "engine.execute", "params": {"command": "sequence.addEdit", "params": {"seconds": 3}}}
{"id": 1, "ok": true, "result": {"cuts": 2}}
```

Methods (UI handlers in `crates/ui-egui/src/control.rs`; independent cancellation in the desktop transport):

Guarded native agent Apply/Undo/Redo/condense publication runs on one bounded Session
worker. The request receives one reply after the real Session returns. While pending,
the UI shows progress/Stop and `ui.inspect.nativeWrite` marks its display-only state;
other editor commands and synthetic editing input refuse rather than queue. Closing
waits for the owner and then follows the dirty-project quit policy. The existing
`engine.cancelOperation` remains independent of the UI queue.

An error reply may include `outcome:"unknown"` when publication may have happened,
including a forwarded request whose reply timed out or disappeared. Reconcile its
original receipt instead of retrying or assuming rollback. A known worker refusal
uses `outcome:"rejected"`; older ordinary error envelopes remain supported. Progress
is diagnostic and never a current edit ticket or permission to bypass native guards.

| Method | Params | |
|---|---|---|
| `engine.execute` / `ui.menu.invoke` | `{command, params}` | any engine or UI command id |
| `engine.cancelOperation` | `{token}` | session-local pre-publication cancellation of an issued guarded agent ticket; handled independently of the UI queue |
| `engine.commands` / `ui.menu.list` | – | command registry / menu tree |
| `ui.inspect` | – | UI state (tool, workspace, dock, timeline view, playback, fps…) |
| `ui.elements` | `{prefix?}` | every on-screen interactive element: id, label, rect |
| `ui.set` | `{tool, workspace, mode, theme, focused, playbackRes, timeline:{pps,scroll,fit}, program:{…}, source:{…}, clipDialog:{param: value}, menuDialog:{param: value}, panels:{scopes, timecode, reference, events, progress}, export:{…}, keyframeSelection:[…]}` | `keyframeSelection` replaces the Effect Controls keyframe selection (`[{clip, effect, param, mask?, time}]`, `time` in media ticks; read back as `ui.inspect` → `ui.keyframe_selection`; a click on a lane keyframe selects just that one); `program` / `source` merge fields into the monitor view state (`res`, `paused_res`, `high_quality`, `display`, `zoom`, `pan`, `show_rulers`, `show_guides`, `lock_guides`, `snap`, `guides`, `compare_ref`, …; see [monitors.md](monitors.md)); `clipDialog` sets fields of the open Edit / Clip / File dialog (Paste Attributes, Make Subclip, Frame Hold Options, …); `menuDialog` those of the M3.11 dialogs (Find, Create Search Bin, Project Settings, Scene Edit Detection, Simplify Sequence, Automate to Sequence…; see `panels::menu_dialogs`); `panels` merges into the panel settings (`ui.inspect` → `ui.panels`; see `panels::panel_state`): Lumetri Scopes `{shown: ["vectorscopeYuv"|"vectorscopeHls"|"histogram"|"parade"|"waveform"], waveformType, paradeType, colorSpace, brightness, scale, clamp, targets}`, Timecode `{rows: [{source, mode, display}], showName}`, Reference Monitor `{ganged, time, display: "composite"|"scopes", scopes}`, Events `{level, selected}`, Progress `{showFinished}` `export` deep-merges into Export mode state (`preset` — applied first —, `settings` (ExportSettings JSON), `fileName`, `location`, `range`, `customStart`, `customEnd`, `openSections`, `manager` (Preset Manager: `query`, `favoritesOnly`, `selected`, `saveName`), `quickOpen`, `quickPath`, `quickPreset`); `program` / `source` merge fields into the monitor view state (`res`, `paused_res`, `high_quality`, `display`, `zoom`, `pan`, `show_rulers`, `show_guides`, `lock_guides`, `snap`, `guides`, `compare_ref`, …; see [monitors.md](monitors.md)); `clipDialog` sets fields of the open Edit / Clip / File dialog (Paste Attributes, Make Subclip, Frame Hold Options, …); `menuDialog` those of the M3.11 dialogs (Find, Create Search Bin, Project Settings, Scene Edit Detection, Simplify Sequence, Automate to Sequence…; see `panels::menu_dialogs`); `projectPanel` merges into the Project panel's view state (`bin` shown in place, `tabs` (bins in a tab or window: `{bin, floating, view, iconSize, nav}`), `activeTab`, `selectedBin`, `rename: {item|bin, text}`, `dialog` (`{"metadataDisplay": {columns, filter}}`, `{"savePresetAs": {name}}`, `{"managePresets": {selected, name}}`, `{"freeformOptions": {options}}`, `{"saveArrangement": {name, bin}}`)); `mediaBrowser` into the Media Browser's (`expanded`, `pathEdit`, `editColumns`, `treeWidth`). View settings, columns and presets are engine commands (`project.view.set`, `project.columns.set`, `project.viewPreset.*`, `mediaBrowser.settings`) |
| `ui.panel.show` / `ui.panel.close` | `{panel}` | |
| `ui.click` / `ui.move` | `{id}` or `{x,y}`, `button`, `count`, `modifiers` | synthetic pointer input |
| `ui.drag` | `{from, to, steps, modifiers}` | press–move–release |
| `ui.scroll` | `{id|x,y, dx, dy, modifiers}` | wheel / trackpad |
| `ui.key` / `ui.type` | `{key}` (`Cmd+K`, `Space`…; off macOS `Ctrl+K` is the same key) / `{text}` | keyboard |
| `ui.timeline.hit` / `ui.timeline.locate` | `{x,y,modifiers?}` / `{clip, edge?}` | timeline hit-testing. `hit` reports a clip's `edge` (`In`/`Out`) within 7 px of it (a third of a narrow clip); just left of a cut that is the left clip's Out, from the cut rightwards the right clip's In. `kind` is the trim a press of the current tool with `modifiers` starts there (`trim`, `ripple`, `roll`, `rateStretch`, `remix`), `volume` on an audio clip's Volume line (Selection and Pen tools), or null |
| `ui.playback` | `{action: play|stop|toggle, speed?}` | |
| `ui.screenshot` | `{path?, panel?}` | PNG of the window or one panel; fails after 10 s when no frame is presented (window hidden, display asleep) |
| `ui.resize`, `app.quit` | `{width,height}` / `{}` | resize or close the native window; `app.quit` quits without the save prompt (unsaved changes stay in the recovery journal), while the command `app.quit` asks first |
| `ui.focus` | `{}` / `{id}` | empty parameters activate the native window; a widget ID requests egui focus without OS activation |
| `perf.stats` | – | performance counters (also the command `perf.stats`, so `engine.execute` and MCP `command_run` reach it; headless sessions return the engine part). `decode`: GOP-cache requests / hits / `cacheHitRate`, decoder seeks, samples decoded and skipped while catching up, `draftFrames` (decoded in draft mode), `h264Threads` (frame-threading workers per H.264 decoder), `liveDecoders` (sources holding a decoder now), `cacheMB` / `cacheBudgetMB` (decoded frames all sources hold, and the shared budget), `planePoolMB` / `planesReused` (idle buffers of the frame pool, the planes of evicted frames and the compositor's float images, which a standalone export frees when it ends; and buffers taken from it instead of allocated), decoder ms (total and per sample), `framesDecoded`, `hardware` (Settings ▸ Playback ▸ Hardware decoding: `enabled`, pictures from hardware decoders `frames` vs `softwareFrames`, `sessions`, `declined` streams, mid-stream `fallbacks`); `playback`: playing, shown / dropped frames and `dropRate` of the current or last play, resolution, `draftDecode` (Settings ▸ Playback ▸ Draft decoding), `audio` (desktop: sound is mixed 200 ms ahead of the device; device `callbacks`, `underruns` and `missingFrames` played as silence because the mixer fell behind, `minLeadMs` the least sound left buffered); `frames`: frame-worker jobs, cancelled, `requestHitRate`, `decodeMs` / `renderMs` (mean, p50, p95 of the last 256 jobs), queue, render-cost estimate, cache use; `monitor`: the `frame` and `revision` of the picture the Program monitor shows against `playheadFrame` and `projectRevision` (how far a drag or a scrub is ahead of the picture); `ui`: fps and frame ms; `process`: CPU seconds; `media`, `jobs`. Counters are cumulative: diff two readings to measure an interval ([performance.md](performance.md)) |

**Timeline thumbnails.** `ui.set {timeline: {thumbnails}}` sets the Timeline wrench menu's Show Video
Thumbnails: `off`, `head`, `headAndTail` or `continuous`.

**Selection or explicit targets.** A command that acts on the selection is "not available right now" when
nothing is selected. When `params` name the targets under a key the command documents (`clips` /
`clip`: timeline clips of the active sequence, `items` / `item`: project items), enablement is
checked against those instead and the selection is left as it is; the command's other conditions
still apply. `engine.commands` and the menus always show the selection's enablement.

**Focus.** Driving the app never steals the user's keyboard. Started with `--control`, the app opens
without activating itself. UI requests and screenshots that need a rendered frame bring the window
forward without making it key (macOS `orderFrontRegardless`), so the user's typing keeps going to
the app they are in. Only an explicit `ui.focus {}` activates FilmCraft. Injected keys work on
focused widgets even when the native window is in the background.

`ui.focus {id}` targets an enabled, visible widget registered with a real egui response. It rejects
unknown IDs, non-widget regions and hidden tool aliases. The reply acknowledges the request;
`ui.inspect.keyboardFocus` reports the actual widget ID, matching element IDs, label and active
focus. This is separate from `ui.focused`, which identifies the active panel. Tools, dock tabs,
tab menus and divider handles expose widget IDs through `ui.elements`. Tab/Shift-Tab move widget
focus; Space/Enter activate focused buttons and tabs, with visible focus outlines.

For unattended validation, start with `--control <port> --control-input-only`. This opt-in mode
accepts widget input through control requests and excludes native keyboard, pointer, file-drop
and menu commands. Window close/quit and screenshots remain available. Ordinary launches and
`--control` alone keep native input enabled. `ui.inspect.input` distinguishes `mode`, the observed
`nativeWindowFocused`, and egui's `effectiveWindowFocused`; effective focus does not activate the
OS window or prove physical-keyboard delivery.

Element ids are stable, e.g. `timeline.clip.<id>`, `timeline.track.V1.lock`, `tools.Razor`,
`project.item.<id>`, `effects.item.gaussian_blur`, `panel.tab.Timeline`, `program.transport.playback.toggle`.

Audio mixing: `mixer.*` commands (strips `"A1"`, `"S1"`, `"Mix"` or ids; lanes `volume`, `pan`,
`mute`, `send.<i>.level`, `fx.<slot>.<param>`): `mixer.inspect`, `mixer.setStrip` (volume, pan, mute,
solo, record arm, solo safe, mode Off/Read/Latch/Touch/Write, output, input map, channels),
`mixer.setValue`, `mixer.touch` / `mixer.release` (a fader gesture; recorded during an automation pass),
`mixer.recordStart` / `mixer.recordStop` (playback runs them), `mixer.addSubmix`, `mixer.deleteSubmix`,
`mixer.addInsert` / `removeInsert` / `setInsert`, `mixer.addSend` / `setSend` / `removeSend`,
`mixer.setKeyframe` / `deleteKeyframe` / `moveKeyframe` / `clearLane`, `mixer.writeAutomation`;
`clipMixer.set`; `clip.audioGain {clips?, mode: set|adjust|normalizeMax|normalizeAll, db}`, `clip.audioPeak`;
`effects.setDefaultTransition`. UI ids: `mixer.<A1|S1|Mix>.<fader|value|pan|panValue|mode|mute|solo|
record|soloSafe|output|input|fx.<n>|send.<n>|meter|name>` (popup entries below them, e.g.
`mixer.A1.mode.Touch`, `mixer.A1.fx.0.studio_reverb`), `mixer.showEffects`, `mixer.transport.<cmd>`,
`clipMixer.A1.<fader|pan|mute|solo|keyframe|value>`, `timeline.track.A1.keyframes[.<lane>|.clip]`,
`timeline.track.A1.lane[.kf.<n>]`, `audioGain.<set|adjust|normalizeMax|normalizeAll|ok|cancel|peak>`.

Essential Sound: `essentialSound.inspect`, `essentialSound.setType {type: dialogue|music|sfx|ambience}`,
`essentialSound.clearType`, `essentialSound.set {key, value}` or `{values: {key: value}}` (keys such as
`repair.noise.on`, `repair.noise.amount` (0–10), `repair.humHz`, `clarity.eqPreset`, `clarity.enhanceTone`,
`creative.reverbPreset`, `ducking.against`, `ducking.reduceDb`, `ducking.fadeS`, `pan.value`,
`volume.levelDb`, `mute`, section switches `<section>.enabled`), `essentialSound.applyPreset {preset, type?}`,
`essentialSound.savePreset {name}`, `essentialSound.deletePreset {name}`, `essentialSound.autoMatch {target?}`,
`essentialSound.generateDucking`; all take `clips` (default: the selected audio clips). UI ids:
`essentialSound.tab.<Browse|Edit>`, `essentialSound.type.<Dialogue|Music|SFX|Ambience>`,
`essentialSound.clearType`, `essentialSound.preset[.save|.delete|.name|.ok]`,
`essentialSound.section.<Name>[.toggle]`, a row per setting key (`essentialSound.repair.noise.on`,
`essentialSound.repair.noise.amount`, `essentialSound.repair.humHz.<i>`), `essentialSound.autoMatch`,
`essentialSound.ducking.against.<Type>`, `essentialSound.generateDucking`, `essentialSound.volume.on`,
`essentialSound.volume.levelDb`, `essentialSound.mute`, `essentialSound.browse.<Type>.<preset>`.

Project files, auto-save, crash recovery and preferences commands (`file.recover`, `prefs.set`, …) and their
automation ids are listed in [project-files.md](project-files.md). That file also lists the media
management commands and dialog ids: offline media and relinking (`media.findMissing`,
`media.relink`, `media.autoRelink`, `media.search`, `media.makeOffline`, `media.status`, `linkMedia.*`),
proxies (`media.createProxies`, `media.attachProxies`, `media.toggleProxies`, `proxies.*`), ingest
(`project.ingestSettings`) and the Project Manager (`file.projectManager`, `pm.*`).

**Moving clips**: `timeline.move {moves: [{clip, track, time}], insert?, linked?}` → `{moved: [clip ids]}`.
With Linked Selection on (the default; `linked` overrides the toggle for one call) the linked
partners of a listed clip that are not listed themselves move by the same time offset and stay on
their own tracks, so picture and sound stay in sync. Like a listed clip, a partner overwrites
what is at its destination (or inserts, with `insert`). A partner on a locked track stays where it
is, and so does one whose position would not change (the listed clip only changed track). `moved`
lists every clip that moved, partners included. No clip is moved before the sequence
start: if one would be, all clips of the call land later by the same amount (their spacing is
kept). A `time` beyond the representable range is an "invalid parameters" error. The timeline
panel's drag passes `linked: false`, because it already lists exactly the clips it moves. With
`copy: true` (Option-drag in the timeline) the clips stay where they are and copies land at the
destinations instead (linked partners are copied too, unless `linked: false`); the result lists
the new clips as `copied`, they are selected, and copies of linked clips are linked to each other.

**Trim mode** (`crates/engine/src/trim.rs`): `trim.selectEditPoint` / `trim.selectNearest` enter trim
mode (the Program monitor becomes the Trim Monitor, ids `trimMonitor.*`); `trim.monitor` returns what
it shows. Dynamic trimming takes an explicit clock (seconds) so it is deterministic: `trim.shuttle
{direction, slow?, clock}` (J/L), `trim.tick {clock}`, `trim.shuttleStop {clock?}` (K, one undo step),
`trim.cancelDynamic` (Esc), `trim.playAround {clock, loop?}` (Space / Shift+K).

**Replace With Clip**: `clip.replaceFromSource`, `clip.replaceFromSourceMatchFrame` and
`clip.replaceFromBin {clips?, item?, keepSourceIn?}` swap the media of the clips and return
`{clips: [ids], item, short: [{clip, shortByFrames}]}`. `short` lists the replaced clips that now
run past the end of their media (the renderer holds the last frame there) with the number of
sequence frames that have no media, at most the clip's length; it is empty when the new media
covers every clip. The edit is the same either way. From Bin starts each clip at the new item's In point; with
`keepSourceIn: true` each clip keeps its own source In instead. A subclip that restricts trims has
no media outside its range, so there a kept In is moved to the subclip's first or last frame, and
the result's `sourceInClamped: [ids]` (present with `keepSourceIn`) names the clips it was moved for.

**Colour** (`crates/engine/src/color.rs`): `sequence.colorSettings {workingSpace: rec709|rec2100-pq|
rec2100-hlg, wideGamut, autoToneMap}`, `clip.interpretFootage {items?, colorSpace?: auto|<id>, pixelAspect?: [num, den]|file}`,
`color.spaces`, `media.colorInfo {item}`; LUTs: `lut.import {path, name?}`, `lut.list`,
`lut.remove {id}`, `lut.export {lut, path, format?}`, `lumetri.setInputLut` / `lumetri.setLook
{clip?, lut: lib:<id>|builtin:<id>|"", path?}`, `lumetri.setSection {clip?, section, on?}`,
`lumetri.applyMatch {clip?, referenceTime|referenceFrame|referenceTimecode, faceDetection?}`. Without
params the menu entries open dialogs (ids `colorDialog.space.<id>`, `colorDialog.working.<id>`,
`colorDialog.wideGamut`, `colorDialog.autoToneMap`, `colorDialog.ok|cancel`). Lumetri panel ids:
`lumetri.input_lut`, `lumetri.look_lut`, `lumetri.switch.<section>`, `lumetri.section.<section>`,
`lumetri.match.*`; HDR scopes: `scopes.hdrWaveform`. `file.exportMedia {sdr: true}` exports an HDR
sequence as tone-mapped SDR.

**Keyboard shortcuts** (`crates/engine/src/shortcuts.rs`): `shortcuts.list {query?, panel?}`,
`shortcuts.get`, `shortcuts.set {command, keys, panel?, add?, keepConflicts?}`, `shortcuts.clear`,
`shortcuts.undo` / `shortcuts.redo`, `shortcuts.conflicts {platform?}`, `shortcuts.forKey {key}`,
`shortcuts.resolve {keys, panel?}`, `shortcuts.presets`, `shortcuts.loadPreset` / `savePreset` /
`deletePreset {name}`, `shortcuts.export` / `import {path}`, `shortcuts.audit`. Keys use `Cmd` (⌘ /
Ctrl), `Ctrl` (macOS ⌃), `Alt`, `Shift`. The dialog (Edit ▸ Keyboard Shortcuts…, ⌥⌘K) uses ids
`shortcuts.*` (`shortcuts.key.K`, `shortcuts.cell.<command>`, `shortcuts.ok`, …).

## MCP
`filmcraft-cli mcp` serves MCP on stdio: headless (in-process session; `--demo` / `--project p.fcproj`)
or `--bridge 127.0.0.1:9876` to drive the running app. Tools: `command_list`, `command_run`,
`command_batch`, `doc_inspect`, `render_preview`, `project_inspect`, `sequence_inspect`, `media_import`,
`render_frame`, `ui_inspect`, `ui_elements`, `ui_click`, `ui_drag`, `ui_key`, `ui_type`, `ui_screenshot`,
`ui_control`; resources `filmcraft://document` and `filmcraft://commands`. `.mcp.json` registers both.
Annotations, argument checking, errors and export progress / cancellation:
[agents.md § Conventions](agents.md#conventions).

Time is in ticks (254 016 000 000 per second); commands also accept `seconds`, `frame` or `timecode`.

## Panel docking commands

`ui.menu.invoke` and `engine.execute` route these frontend commands:

| Command | Parameters | Result |
| --- | --- | --- |
| `window.floatPanel` | `{panel?, rect?: [x,y,width,height]}` | Move an existing panel into a floating group. Omitted panel uses the focused panel; omitted rect uses a centered 420×320 group. |
| `window.dockPanel` | `{panel?, anchor?, zone?: "center"\|"left"\|"right"\|"top"\|"bottom", before?}` | Move into the anchor's tab group or a side split; `before` names an existing destination tab for precise ordering. Floating destinations accept tabs only. Without `anchor`, a side `zone` docks the panel along that edge of the whole workspace, at a quarter of its width or height. |
| `window.closePanel` | `{panel?}` | Close a docked or floating panel without closing its project documents. |

Invalid source/target identities and non-finite or out-of-range geometry return an error with
the layout unchanged. `ui.inspect.ui.floating` reports floating groups and their logical-point
rectangles. `panel.drag.<Panel>` locates each panel's header grip; `panel.float.<index>.move`
and `.resize` locate floating geometry handles. Named workspace saves include these groups.

`window.dockAction` accepts `{action}` using the shared docking action's JSON form. It
covers tab moves and ordering, split and floating geometry, activation, and closing.
For example, `{"action":{"ResizeSplit":{"path":[],"size":{"FixedFirst":360}}}}`
resizes the root split; path entries use `false` for the first child and `true` for
the second. `{"action":{"MoveFloating":{"panel":"Info","rect":[45,65,420,320]}}}`
repositions and resizes the floating group containing Info. Invalid actions leave the
layout unchanged. Pointer gestures dispatch through this same catalog command.

### Exact track removal and ordering

`sequence.removeTracks {tracks:[id|"V1"|"A1"|"S1"|"C1",...]}` removes exactly
the listed tracks as one native Undo step. The bounded nonempty list may remove
all tracks of a kind; duplicate, missing and ambiguous identities refuse before
any mutation. Surviving linked clips on other tracks stay intact. Selections,
inspector references and source-patch destinations are repaired. Deleting a
submix reroutes its inputs to Mix and removes sends with their automation lanes
reindexed by the existing mixer service. Legacy `sequence.deleteTracks` retains
its keep-one-video/audio selector policy before using this same removal logic.

`sequence.reorderTrack {track:id|reference,after:id|reference|null}` moves a track
after another of the same collection; null means first. Video, audio, submix
and caption collections stay separate. Track IDs, contents, source/sequence
times, links, channels, routing, automation, effects and cue metadata stay with
the track. Default video/audio/submix names follow their new ordinal; custom
names remain. Cross-kind, self, missing and no-op placements refuse without
clearing either native history branch.

Track creation reserves above the full bounded native ID inventory, including
legacy IDs beyond `next_id`. `timeline.setTrack` accepts only its documented
fields, validates booleans/text/labels and finite numbers before the edit, and
uses the native -96 to +15 dB fader range. Its existing -100 to +100 pan clamp
remains. These native commands do not themselves grant model execution.

### Guarded authored tracks

`agent.readTracks {token,scope:{projectId,anchorSequence},offset,limit,projection?}`
requires the same scoped native ticket and active adopted sequence. Up to 32
headers/page expose authored static settings, routing/sends and a read-only
master (`id:null`). The returned digest covers every track payload and master,
including hidden/unpaged effects, automation, clips and cues. Inventory and page
bounds are 4096 tracks and 512 KiB. Projection reads require final original-file
validation; headers do not certify evaluated automation or picture/sound.

Private `agent.track {scope,beforeSha256,edit}` is interpreted inside one exact
`agent.apply`, never a stand-alone registry command. Edits are `add` (explicit
counts/channel formats and same-kind `*_after` IDs, null for first), `remove`
(exact ID set), `reorder` (one ID and same-kind `after` or null) or `set`
(per-kind typed changes). Caption creation retains its caption command. Shared
human services own the actual mutations; unexpected project/editor collateral,
clamping, implicit unlock, invalid submix route order, locks/protection and stale
state refuse without publication. Master/routing/automation authoring and
cross-track movement are excluded.

Receipts include complete before/after track hashes, affected IDs, human command
and removal collateral counts. Poema's model adapter uses opaque same-plan
handles only for headers actually read. Complete track-state readback follows
commit and owned Undo/Redo; saved reply recovery never repeats a native write.

### Authored clip effect stacks

agent.readEffectCatalog accepts token, scope, offset, limit and optional projection.
agent.readEffectStack adds the persistent clip identity. Each reader returns up
to 32 headers, the exact revision/sequence and a continuation offset. Catalog
headers describe actual definitions and addability. Stack headers distinguish
duplicate instances by persistent ID, and a complete SHA-256 digest covers the
unpaged authored parameters, masks, animation, flags and identities. Reads do not
allocate IDs, open resources or evaluate output. Projected documentary reads
retain their source-validation debt and cannot renew an original-file witness.

Private agent.editEffectStack is interpreted inside one exact agent.apply. Its
arguments are clip, identity, beforeStackSha256, scope and edit. The edit is one
of add(definition), remove(effectId), set_enabled(effectId,enabled), or
reorder(effectId,beforeEffectId). A null reorder destination means the end of
the current movable segment, immediately before its next fixed-effect barrier.
Add requires a supported exact definition with safe literal defaults. Video
effects insert before the first intrinsic; audio effects append to the explicit
audio clip. There is no implicit linked-audio retargeting. Text/resource
admission, graphic layers, transitions and track/master stacks use other
contracts. Locked/protected/ambiguous targets, stale stacks, no-ops, malformed
fields and intrinsic removal/reordering refuse without publication.

Human effects.apply/remove/toggleEnabled/reorder services perform the mutation.
effects.reorder accepts clip, from and to as exact current zero-based positions;
intrinsics cannot move or be crossed. The service moves the whole instance and
retains its persistent ID, parameters, masks and animation. Bounds are 4096
instances and 16 MiB of serialized authored stack. The typed adapter compares
complete project collateral and publishes one normal native Undo step. Its
receipt includes before/after stack digests/counts and the changed instance ID.

Poema requires fresh timeline then full-stack digest readback after commit and
owned Undo/Redo. Verified cold-history restoration rebases the current ownership
revision; it never changes a historical receipt. Selecting another sequence at
the same revision invalidates completion without requiring the old clip on the
new sequence. Returning to the owned sequence permits fresh verification.
Documentary digest agreement is distinct from Program picture or mixed sound.

### Shared native move and copy validation

`timeline.move` accepts 1..4096 complete distinct `{clip,track,time}` rows plus
optional boolean `insert`, `linked` and `copy`. Malformed rows/options, unknown
fields, mismatched video/audio destination kinds, overlapping requested
destinations, no-op moves and unsupported end times reject atomically. No row
is silently skipped. Omitted flags and native negative-group/locked-partner
policies retain their existing behavior.

The shared typed MovementPlan expires when the project, active sequence or an
implicit Linked Selection dependency changes. Copy/new-track/boundary IDs reserve
checked space above the complete bounded project inventory, so imported IDs and
exhausted counters cannot collide or panic. Existing native edit algebra owns
insert/overwrite, transitions and caption timing, with one native history step.
The human service does not grant raw model execution. The exact adapter below
supplies a separate typed composer contract.

### Guarded exact arrangement

`agent.arrange` is a private `agent.apply` operation, not a registry command.
Use one exact operation with `{scope,beforeSha256,beforeCaptionSha256,copy,mode,
linkedPolicy,placements:[{clip,track,startTicks}]}`. Scope is
`{projectId,anchorSequence}`; mode is gap, overwrite or insert, linkedPolicy is
preserve or explicit, and copy is boolean. Existing same-kind destinations and
current tracked bound originals are required. Placements are 1..200 distinct
clips with exact safe nonnegative tick starts and positive supported durations.

Preserve requires all current linked partners with equal offsets, including
unchanged sound. Explicit permits independent split edits with native sync-lock
collateral. Gap refuses collisions; overwrite reports covered media; insert
ripples the aggregate destination span. Moves leave source gaps. Sequence cues
stay fixed except supported native insert timing; source words are not carried.
Locked/protected source copies are allowed only while originals remain unchanged.
Every write retains the ordinary source/COW/protection/cancellation fences.

The arrange_edit result contains before/after track and caption hashes, the
resolved placed numeric and persistent identities/parents, exact destination
ticks, full collateral counts and humanCommand timeline.move. The model family
uses opaque handles only from actually read same-plan headers. It validates
receipts and compares fresh native hashes and actual placement/lineage before
verification; owned Undo/Redo and saved uncertain-reply recovery retain that debt.
Lookup of a canonical receipt never replays movement or adopts another history
step. Native state verification and sampled output are separate from editorial
or complete delivery certification.
