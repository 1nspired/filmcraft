# FilmCraft interface

The egui frontend draws panels and dialogs, and dispatches project changes through the engine.
Interactive controls keep stable automation ids regardless of the interface language.

## Guarded native writes

Desktop control setup installs the bounded engine Session worker. Guarded agent
Apply/Undo/Redo/condense publication holds its original request until the actual
Session returns, then replies once. While pending, the frontend paints a dedicated
progress/Stop view rather than running editor panels against the display projection.
Audition stops, active human gestures refuse before transfer, and editing commands,
synthetic editing input and native menu actions are not queued for later execution.
Read-only UI inspection, screenshots and viewport operations remain available;
`ui.inspect.nativeWrite` identifies display-only state and diagnostic progress.

The Stop widget is `poema.nativeWrite.cancel`. Stop/Escape uses existing native
cancellation and does not claim rollback after publication begins. Closing while
busy requests cancellation and waits for the real owner, then follows the dirty
project Save/Don't Save/Cancel policy. Ordinary composer Send needs no extra checkbox.
Worker failure never falls back to a synchronous edit or replays the command.

`native_writer::tests` exercises the real worker/control route, one delayed reply,
busy widgets/inspection, input exclusion, restored history and dirty close behavior.
Completion delivery is held only in those tests to make the busy UI deterministic;
native execution and guards remain real. `FILMCRAFT_UI_SNAPSHOT_DIR` enables a
rendered busy-view screenshot. Engine worker tests additionally cover queued Stop,
changed originals, full-queue restoration, post-commit panic and abandoned recovery.

## Speed Duration dialog values

Speed/Duration initializes Reverse from the clip's native `reverse` flag, not
from the sign of its separately stored speed ratio. The draft retains the full
saved percentage instead of rounding it to two decimal places before the user
edits it. Opening a reversed or precisely retimed clip and accepting unchanged
fields must not silently change those settings. Existing control IDs and engine
dispatch remain unchanged.

Two `clip_dialogs_ui` regressions exercise the actual headless dialog: a reversed
clip at positive speed, Cancel without project/history changes, unchanged OK and
Undo, and a saved 125.555% speed accepted without rounding to 125.56%. These are
generated UI checks, not continuous-motion or real-camera qualification.

## Local Poema prototype: clip notes and protection

Clip > Poema notes & protection (also the clip context menu) opens an ephemeral draft pinned to
the active project snapshot, sequence and clip identity. A linked picture/sound selection is one
choice; unrelated multi-selections are refused. Untracked sequences offer an undoable Enable
sequence tracking action. Apply note changes only `editor_note`, retaining other annotation keys.
Protect linked clips and Remove protection are separate, undoable human actions covering linked
picture/sound even when Linked Selection is off. Protected clips show a `[P]` name badge; humans
can still edit them. Protection constrains `agent.apply`, not the trusted debug/control endpoint.

Any intervening document edit, Undo, sequence switch or project replacement makes the draft stale.
Save/protect/unlock are disabled until Reload selected clip (which discards unsaved text); closing
discards the draft. Text limits are checked as UTF-8 bytes, not just characters. Dialog prose is
experimental English; the menu follows the existing translated-menu catalogs.

Automation: `clip.poemaNotes {clip?}` opens it, `ui.set {poema:{note?,reason?}}` changes text only,
and `ui.inspect.poema` reports the pinned draft/stale flag. Real widget IDs are `poema.enable`,
`poema.note`, `poema.saveNote`, `poema.reason`, `poema.protect`, `poema.unlock`, `poema.reload` and
`poema.close`; protected clip badge IDs are `timeline.poema.protected.<clip>`. Draft automation
cannot change the target or perform a save without the same widget/engine action as the editor.

Unit regressions are `panels::poema::tests`. The separate Poema lab's `filmcraft_smoke.mjs
--guarded --identities --agent-ui` drives actual widgets, protected/stale screenshots, restricted
scripted-provider edits, native readback and human keyboard Undo on scratch synthetic/camera media.
This does not register the prototype tools in the live Poema agent or change live projects.

## Transition drop previews

Dragging a video or audio transition over a compatible clip shows its actual time span, the
selected cut and an In/Out label. Shared cuts center the span; isolated edges use a one-sided
span. The preview uses the engine's read-only transition planner, including Timeline duration
preferences, and release dispatches `effects.apply` with the same edge. Locked tracks and
incompatible clip types show no valid transition preview. Ordinary effects retain their clip
outline. `timeline.transitionDropPreview` exposes the span and edge to automation.

`transition_drop_ui` covers both edges of short video/audio clips, hover without project edits,
the committed span and undo. Set `FILMCRAFT_UI_SNAPSHOT_DIR` to render screenshots with wgpu.

## Localisation

Edit > Language offers English, Japanese, Spanish, Brazilian Portuguese, Ukrainian, Simplified
Chinese, German, Russian, French, Indonesian, Korean and Italian (all but Ukrainian and Russian cover
the whole interface; those two cover the core menus). The language is stored in the engine's `general.interfaceLanguage` preference,
restored on startup, and also available in Settings > General.
Its default, System Language (`system`), follows the operating system: the first of the user's
preferred languages that the interface has (the host supplies them through
`HostHooks::system_languages`: `sys-locale` on the desktop, `navigator.languages` on the web),
otherwise English.
The `app.language.*` UI commands and `prefs.set` reach it through the control channel.
Japanese requires a craft-fonts build or a suitable installed font.

English source strings are lookup keys in `src/i18n/<code>.tsv`. `tl!` translates literals, `tlf!`
fills translated templates, and `i18n::t` translates names from registries. Placeholder values
(including user filenames containing braces) are inserted literally. Catalog translations are
original work using ordinary language, without proprietary localisation resources.

Spanish, Japanese, Simplified Chinese, French, Brazilian Portuguese and German cover menus, panels, dialogs, settings and registry labels. Searches accept both the
translated label and its English source, including Unicode capitals. Project content, command ids
and preference values retain their original values. Engine errors, CLI and MCP messages remain
English. Ukrainian and Russian currently cover the core menus; they fall back to English elsewhere.

Verification: `cargo test -p filmcraft-ui-egui` checks catalog syntax, duplicate keys, placeholders,
literal/menu/registry coverage and UI behaviour; `cargo xtask ci` runs the workspace gates. Visual
checks use the control channel to switch language and capture the resulting panels.

## Source playback

`source_playback.rs` auditions media independently of Program playback. It reuses MediaSource
decoding, output mapping, the device clock and the desktop play-ahead buffer. Only one monitor
owns the device at a time. Source frame jobs use the existing frame server and Source resolution.
The live project, undo stack and timeline playhead are unchanged. See [monitor documentation](../../docs/monitors.md)
for behavior, commands, limitations and regression tests. Source playback tests use synthetic
demo media and fake audio outputs; no third-party assets are introduced.

Source drag controls snapshot the selected span and dispatch `timeline.place` for video, audio,
or linked video/audio. Full-clip marks are implicit until set by the user. The engine owns edits,
range validation and undo; the UI owns the drag gesture. See `source_drag_ui` and the engine's
`source_placement_tests` for regression coverage.

Source range handles use transient gesture previews and the engine's integer-frame
`source_monitor::adjust_range`; release dispatches one mark edit. Monitor command routing
keeps Source navigation, markers and marked-range playback separate from Program.

## Pane layout controls

Dock transactions and pointer targets use `craft-ui::docking`; bounded split geometry and
keyboard-accessible resize handles use `craft-ui::layout`. FilmCraft adapts its existing saved
dock tree and panel identities to the shared model. Panel tabs or their header grip can move
between tab groups, split at an edge, reorder, or float when released with Cmd/Ctrl. Floating
panels move by their empty header and resize from the bottom-right corner. Timeline sequence
tabs retain their sequence-opening, closing and horizontal reorder behavior; the header grip
moves the Timeline panel. Workspace saves retain floating groups alongside the legacy layout
JSON, and workspaces without floating data still load. Automation IDs and workspace commands
remain the integration boundary.

Tools and dock tabs expose their actual egui widget IDs, paint keyboard focus, and support
Tab/Shift-Tab traversal and Space/Enter activation. Control-injected keys can reach focused
widgets in a background window. The optional `--control-input-only` native launch mode isolates
automation from native widget/menu input; `ui.inspect` reports native and effective focus
separately. Focus and input-ownership regressions cover document/playhead preservation.

Compact monitor layouts separate the current and duration timecodes from zoom, resolution and
settings. Source video/audio drag choices retain their existing placement behavior. Transport
buttons use `craft-ui::layout::fit_toolbar`; commands beyond the available width remain available
through More, including their selected state and Source/Program routing. A scrolling fallback
retains all commands if even More cannot fit. `monitor_geometry` checks the painted readouts,
control bounds, overflow actions and Source drag payloads across monitor widths and themes.

## Local Poema guarded-read settling (2026-10-20)

The desktop control channel defers `agent.snapshot` while the active timeline view is loading,
animating, awaiting an empty-to-first-clip fit, or not yet reflected in saved editor view state.
It uses the existing bounded read-retry mechanism (three seconds), before any ticket is issued.
No write is deferred/replayed and the native full editor-context guard is not weakened. Hidden
timeline panels do not wait for a fit that cannot render. Headless CLI snapshots are unchanged.
An occluded window needing that UI pass uses the existing raise-without-keyboard-focus hook;
a stable background read does not raise it.
This addresses a Canon multi-insert acceptance failure where a correct stale-read rejection
occurred as the empty timeline automatically fitted its first clip. Failure leaves earlier
successful project edits intact; the composer must report that partial state, not imply rollback.

## Effect stack ordering and persistent keyframe targets

Effect Controls row menus offer Move Up / Move Down through the shared native
effects.reorder service. Boundaries and intrinsic neighbors disable the action.
Menu automation IDs include the clip ID and current effect ordinal. Tracked
effect rows, mask selections and keyframe references retain existing persistent
IDs, so reordering duplicate definitions and Undo/Redo preserve the intended
instance. Legacy positional keyframe selections are cleared after reordering
instead of silently targeting a different effect. Saved optional reference IDs
remain backward compatible; resolving a reference never edits the project or
allocates identity. The keyframes_ui integration target exercises the actual
menu and shared history; FILMCRAFT_UI_SHOTS also captures the rendered states.


## Application setup admission

New canonical Poema projects retain a separate native setup hold after native
activation. The first editable frame waits for the exact application setup and
connected-checkpoint readiness record. The UI paints Setting up Poema, registers
`native-application-setup`, rejects synthetic edits and drains menu input rather
than queuing it. Cancel update cannot release this hold. Closing setup does not
wait for an unavailable composer and keeps native files/journals. After initial
admission, a later hold retains the normal composer draft-flush/drain handshake.

A generated headless UI test covers held first frames, no cancel bypass, exact
record release, later human editing and missing-record re-hold. Physical desktop
and full product qualification remain deferred.

Managed Poema New Project now requests a separate canonical application window
through the same embedded composer, with a bounded native nonce and exact binding.
The existing human Session, path and history stay in place. Page reload observes
the same retained launch; an uncertain launch requires explicit menu retry with
that identity. Ready/cancelled replies reset only the launch gate and never close
the parent viewport. Unmanaged FilmCraft keeps its ordinary New Project command.
Physical installed-window acceptance remains pending.


## SSD development: Independent project media picker scope (November 6, 2026)

The existing asynchronous native media picker/import accepts disjoint closed
sequence and independent project/workspace scopes. Native catalog checks the
exact Root identity before selection and again before import; the existing COW
project/revision/session guards, native writer and human import command remain.
Version4discovery advertises independent_project_scope. Existing sequence
requests/receipts and durable archive/release semantics remain readable. Root
receipts retain workspace UUID rather than borrowing a sequence, including when
no timeline exists. They still require fresh source validation and explicitly
claim no original-owner write or perception request.
Generated scope regressions reject mixed/null/dummy anchors and foreign realms.
The generated Root intake harness uses the actual Session/UI control route;
physical OS-dialog, shared-composer full-flow and product qualification remain
separate pending acceptances. No new file chooser or second media writer exists.

Independent source-library reads share the snapshot-view settling barrier and background verifier. The generated intake fixture permits the new private read/validation methods; human picker/import and the native writer remain unchanged. This increment does not enable project-only perception/composer routes or claim physical dialog acceptance.

## Paused recovery candidates

A cold, unpublished native workspace candidate exposes its existing private
`agent.readHistoryCheckpointBasis` before its first editable timeline view.
This is needed to cancel and refresh an uncertain activation without replaying
it. The candidate remains paused; ordinary snapshot/library reads still wait
for the view, and an activated editor or original owner keeps the normal view
guard. `control::snapshot_tests::paused_root_candidate_exposes_only_private_recovery_basis_before_first_view`
uses an actual staged/retired/cold Session and checks that the basis does not
change project, editor state or revision. Physical window qualification is separate.
