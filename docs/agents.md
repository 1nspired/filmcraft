# Agents

FilmCraft is built to be driven by AI agents, and largely built by them. Part 1 covers using
FilmCraft from an agent. Part 2 covers developing FilmCraft as an agent.

# Part 1: driving FilmCraft

## 1. MCP server

`filmcraft-cli mcp` serves MCP on stdio. It has two modes:

| Mode | Command | What it drives |
|---|---|---|
| Headless | `filmcraft-cli mcp --demo` or `--project p.fcproj` (neither = empty project) | an in-process engine session, no window |
| Bridge | `filmcraft-cli mcp --bridge 127.0.0.1:9876` | the running desktop app started with `filmcraft --control 9876` |

Headless mode also takes `--data-dir DIR` (user presets). Any other option is rejected (exit 2):
there are no folder-confinement flags such as `--automation-read-root`, so the server can read
and write wherever the user can.

**Claude Code.** The repository's `.mcp.json` registers both servers (`filmcraft` = bridge,
`filmcraft-headless` = demo). Both point at `target/release/filmcraft-cli`, so build it first:

```sh
cargo build --release -p filmcraft-cli -p filmcraft
./target/release/filmcraft --control 9876 &      # only for the bridge server
claude                                           # approve the project MCP servers when asked
```

To register the server by hand, or for another MCP client, use the absolute binary path:

```sh
claude mcp add filmcraft-headless -- /abs/path/filmcraft/target/release/filmcraft-cli mcp --demo
```

```json
{ "mcpServers": { "filmcraft": { "command": "/abs/path/filmcraft-cli", "args": ["mcp", "--bridge", "127.0.0.1:9876"] } } }
```

### From an installed release

The release packages ship `filmcraft-cli` alongside the desktop app, so no build is needed:

| Install | CLI |
|---|---|
| Windows (MSI) | `C:\Program Files\FilmCraft\filmcraft-cli.exe` by default (wherever you installed it otherwise), not on `PATH` |
| Linux (deb, rpm) | `/usr/bin/filmcraft-cli` |
| macOS | the separate `filmcraft-cli-<version>-macos-<arch>.zip` release asset (the `.app` holds only the desktop app) |

```sh
# Windows, default install folder
claude mcp add filmcraft -- "C:\Program Files\FilmCraft\filmcraft-cli.exe" mcp
# Linux, or macOS with the CLI unzipped onto PATH
claude mcp add filmcraft -- filmcraft-cli mcp
```

### Tools

| Tool | Modes | Purpose |
|---|---|---|
| `command_list` | both | every command: id, label, menu, shortcut, params, enabled now (`filter`, `enabled_only`) |
| `command_run` | both | run a command `{id, params}`; edits are undoable |
| `command_batch` | both | run several commands in order `{steps: [{id, params}], stop_on_error}` → `{completed, failed, results}` |
| `doc_inspect` | both | the project tree and the active sequence in one call (`project_inspect` + `sequence_inspect`) |
| `render_preview` | both | same as `render_frame` |
| `project_inspect` | both | bins and items with ids, types, durations; active sequence |
| `sequence_inspect` | both | the active sequence: tracks, clips (`start` / `end` and `sourceIn` / `sourceOut` in ticks, frames, `speed`, `reverse`: the clip plays its `sourceIn` to `sourceOut` stretch backward, `gainDb`), effects, transitions, markers (name and `comment`), playhead, selection |
| `media_import` | both | import files by absolute path (`paths`, an array; optional `bin`, `image_sequence`) |
| `render_frame` | both | PNG of the program frame at `seconds` (headless renders; bridge screenshots the Program monitor). Read-only: the playhead and selection are left as they were |
| `ui_inspect` | bridge | UI state: tool, workspace, panels, zoom, playback, fps |
| `ui_elements` | bridge | on-screen interactive elements with id, label and rect (`prefix` filter) |
| `ui_click` | bridge | click by `id` or `x`,`y`; `button`, `count`, `modifiers` |
| `ui_drag` | bridge | press–move–release between points or element ids |
| `ui_key` | bridge | key or shortcut: `Space`, `Cmd+K`, `Shift+Delete` |
| `ui_type` | bridge | type text into the focused field |
| `ui_screenshot` | bridge | PNG of the window or one `panel` |
| `ui_control` | bridge | call any control-channel method directly (`ui.set`, `ui.scroll`, `ui.timeline.locate`, …) |

Typical loop: `project_inspect` / `sequence_inspect` → get ids → `command_run` → `render_frame` or
`ui_screenshot` → look at the result → `edit.undo` if it's wrong.

### Conventions

The server follows a few conventions that make it predictable for agents. They are not
FilmCraft-specific: the sibling craft apps' MCP servers can follow the same ones, so an agent that has
driven one app can drive the others.

- **Core tools.** `command_list`, `command_run`, `command_batch`, `doc_inspect` and `render_preview`
  work in both modes. The FilmCraft tools above stay as they are.
- **Titles and annotations.** Every tool has a `title` and the four MCP hints (`readOnlyHint`,
  `destructiveHint`, `idempotentHint`, `openWorldHint`), so a client can let the read-only tools
  (lists, inspects, renders, screenshots) run without asking and ask before edits. When you add a
  tool, set them in its `#[tool(...)]` attribute; a test checks every listed tool.
- **Strict arguments.** An unknown tool argument is a JSON-RPC `-32602` error that names it and the
  accepted ones (`unknown argument "filtr" for command_list; expected: enabled_only, filter`), not a
  silently ignored key. `command_run.params` goes to the command unchanged.
- **Errors.** A failing command is an `isError` result with the engine's message. A line that is not
  JSON gets a `-32700` parse error (id `null`) and the server keeps serving. A command that panics is
  reported as `internal error: …`, and the session stays usable.
- **Resources.** `filmcraft://document` (the project tree and active sequence, as `doc_inspect`) and
  `filmcraft://commands` (the command catalog, as `command_list`), both `application/json`. For MCP
  2026-07-28 clients such as current Claude Code, list and read results carry the `ttlMs` and
  `cacheScope` hints that revision requires.

### Long exports: progress and cancellation

`command_run {"id": "file.exportMedia", "params": {…, "wait": true}}` in headless mode blocks until
the file is written. It uses the MCP progress and cancellation utilities, so a client can show
progress and stop it:

- The export runs as the same background job `jobs.list` shows. The session is locked only for short
  polls, so other requests (`ping`, `doc_inspect`, `jobs.list`, …) are answered while it encodes.
- `file.exportMedia` with `wait: true`, an export job in `jobs.list` and each `export.queue.list`
  item carry `offlineMedia`: the offline items the export drew the placeholder for (see §2b), `[]`
  when none. The export itself still succeeds; deciding what to do about it is up to the caller.
- Every job in `jobs.list`, and every encoding item of `export.queue.list`, carries `etaSeconds`: the
  time left at the job's speed over the last 15 seconds, `null` until a second of progress has been
  measured (a loudness pass, a seek) and once it is done. The app shows it as `37% · 2:05 left` in the
  status bar, the Export queue and the Progress panel.
- With `"_meta": {"progressToken": T}` on the `tools/call`, the server sends
  `notifications/progress` `{progressToken: T, progress: <frames done>, total: <frames>, message:
  <job status>}` at most every 100 ms while frames advance. Without a token it sends none.
- `notifications/cancelled` for the request stops the encode at the next batch (`jobs.cancel`),
  deletes the partial output (the movie, or the frames and caption sidecar written so far) and sends
  no response, as the MCP cancellation utility asks.
- Without `wait` the command returns `{job, path, poll: "jobs.list", cancel: "jobs.cancel"}` at
  once: poll `jobs.list` for the job's `progress`, `etaSeconds`, `finished` and `result`, and stop it
  with `jobs.cancel` (`export.queue.list` lists the Export Queue only, not direct exports, #1544). Other exports (`file.exportFrame`, interchange formats, …) finish quickly and
  ignore the token.
- Bridge mode works the same way: the export runs as an app job (the app stays responsive and shows
  "Exporting… NN%" in its status bar) and the call returns when it is written, however long it
  takes. `filmcraft-cli --bridge … exec file.exportMedia … wait=true` blocks the same way.

## 2. Control channel

`filmcraft --control 9876` (or `FILMCRAFT_CONTROL_PORT=9876`) listens on `127.0.0.1` only. Send one
JSON request per line and get one JSON reply per line. Full method table:
[control-protocol.md](control-protocol.md).

```jsonc
{"id":1,"method":"engine.commands"}
{"id":2,"method":"engine.execute","params":{"command":"file.openDemoProject"}}
{"id":3,"method":"engine.execute","params":{"command":"playhead.set","params":{"seconds":2}}}
{"id":4,"method":"engine.execute","params":{"command":"sequence.addEdit"}}
{"id":5,"method":"engine.execute","params":{"command":"effects.apply","params":{"effect":"Gaussian Blur"}}}
{"id":6,"method":"ui.elements","params":{"prefix":"tools."}}
{"id":7,"method":"ui.click","params":{"id":"tools.Razor"}}
{"id":8,"method":"ui.key","params":{"key":"Cmd+Z"}}
{"id":9,"method":"ui.screenshot","params":{"path":"/tmp/timeline.png","panel":"Timeline"}}
```

Replies look like `{"id":4,"ok":true,"result":{"cuts":2}}` or `{"id":7,"ok":false,"error":"…"}`.
Minimal client:

```python
import json, socket
s = socket.create_connection(("127.0.0.1", 9876)); f = s.makefile("rw")
def call(method, **params):
    f.write(json.dumps({"id": 1, "method": method, "params": params}) + "\n"); f.flush()
    return json.loads(f.readline())
print(call("engine.execute", command="sequence.inspect"))
```

Notes:

- Time is in ticks (254 016 000 000 per second). Commands also accept `seconds`, `frame` or
  `timecode`.
- A track parameter (`track`, `audioTrack`) is a track id or a name such as `"V1"` / `"A2"`.
  Leaving it out picks the command's default track; naming a track the sequence does not have is
  an "invalid parameters" error, never a different track.
  In `timeline.place`, `track` is where the picture goes and `audioTrack` where the sound goes;
  an audio track given as `track` places the sound alone, on that track.
  The clip lands on the frame grid: its start and its end (`time + duration`) each snap to the
  nearest frame.
- `sequence.inspect` reports `durationFrames` as the number of frames export renders: a sequence
  that ends partway through a frame counts that frame.
- `engine.execute` also runs UI-only commands (`tool.razor`, `playback.toggle`,
  `window.workspace.color`, `window.panel.<name>`).
- A command that works on the selection is disabled ("no clips selected", "select a clip in the
  Project panel") until something is selected, unless the call names its targets under a key the
  command documents (`clips` / `clip` for timeline clips, `items` / `item` for project items):
  `clip.replaceFromBin {"clips":[12],"item":4}` runs with nothing selected and leaves the
  selection alone. Only the selection condition is answered by the named targets; every other
  one (an open sequence, a filled clipboard, the right kind of clip) still applies, and ids of
  nothing do not count. `command_list` / `engine.commands` keep reporting the selection's state.
- Element ids come from the previous frame. If an element is missing right after a layout change,
  the app retries on later frames before giving up.
- `timeline.move` takes linked partners along while Linked Selection is on: moving a picture
  clip moves its sound by the same offset (pass `"linked": false` to move only the listed clips).
  It returns `{"moved": [ids]}`, every clip that moved.
- Without a window, `filmcraft-cli run script.jsonl` (lines of `{"id":"…","params":{…}}`) runs the
  same commands headlessly. See §2b for the rest of the CLI.

## 2b. Command-line interface

Every command is also one shell call away. Options go anywhere; output is JSON; exit status is 0 on
success, 1 when a command fails and 2 on a usage error. `filmcraft-cli help`, `filmcraft-cli --help`
and `filmcraft-cli -h` print the reference and exit successfully.
If the reader of stdout closes the pipe early (`filmcraft-cli commands | head`), the CLI drops the rest
of its output but still finishes the work, saves included, and the exit status still reports
failures; any other stdout write error is reported and makes the status 1.

```sh
filmcraft-cli commands razor                     # find ids (add --json for machine output)
filmcraft-cli describe timeline.razor            # one command: menu, shortcut, params, enabled now
filmcraft-cli --demo inspect sequence            # project / sequence as JSON
filmcraft-cli --project p.fcproj --save exec timeline.razor seconds=3.5
filmcraft-cli --project p.fcproj exec effects.apply '{"effect":"Gaussian Blur"}'
filmcraft-cli --project p.fcproj exec markers.exportCsv path=review.csv
filmcraft-cli --project p.fcproj --save import a.mov b.wav
filmcraft-cli --project p.fcproj export out.mp4  # format from the extension; waits for the job
filmcraft-cli --project p.fcproj export out --preset "YouTube 1080p Full HD" --start 0 --end 10
filmcraft-cli export --list-presets prores       # built-in + user presets (--data-dir for another library)
filmcraft-cli --project p.fcproj exec export.queue.add preset="Apple ProRes 422 HQ" path=renders/ start=true wait=true
echo '{"id":"file.newBin","params":{"name":"Selects"}}' | filmcraft-cli --project p.fcproj --save run -
filmcraft-cli --bridge 127.0.0.1:9876 exec window.workspace.color   # the running app
```

`key=value` values are parsed as JSON when they can be (`3.5`, `true`, `[1,2]`), otherwise taken as
strings; dotted keys nest (`color.r=1`). `run` prints one JSON line per command
(`{"line","id","ok","result"|"error"}`) and stops at the first failure unless `--keep-going`.
`--save` writes back to `--project`; `--save-as path` writes elsewhere.

Media that is offline when it is needed (the file is missing, was made offline, or can't be read)
renders as the "media not found" placeholder. `export` and `render` then name each such item on
stderr (`item 8 a.mp4 (missing): /path/a.mp4`) and exit 1 after writing the output, so a moved or
unmounted media folder doesn't pass as a good render. `--allow-offline` accepts the placeholder
frames: the warning stays and the exit status is 0. The export result lists the same items as
`offlineMedia` (`[{item, name, path, status, error}]`, `status` as in `media.status`: `missing`,
`offline` or `unreadable`), and `[]` when every clip it drew was online; check before exporting with
`exec media.status` or `exec media.findMissing`.

`markers.exportCsv {path}` exports all markers in the active sequence, including colours hidden
in the UI and markers outside In/Out, sorted by start then id. The UTF-8 CSV has a header and CRLF
records; quotes, commas and embedded newlines are escaped. Columns are Sequence, Marker ID, Name,
Comment, Kind, Color, Start Timecode, End Timecode, Duration Frames, Start Ticks, Duration Ticks and
Frame Rate (exact numerator/denominator). Color uses the user's display label from Settings ▸
Labels. Timecodes include the sequence's start timecode and
drop-frame setting; End is exclusive, and a point marker's End equals Start. Duration Frames is
rounded down; tick columns preserve sub-frame timing (254016000000 ticks per second).
An empty sequence writes just the header. User text starting with a spreadsheet formula character
(`=`, `+`, `-`, `@`, including after whitespace) gets a leading apostrophe in the exported cell.
The export leaves the project, dirty flag and undo history untouched, works with locked tracks,
and uses the host's file service (atomic
on desktop, a download on web). It exports sequence markers only, not clip/source markers;
reports are limited to 100000 markers and a conservative 16 MiB output budget. Invalid timing or a
failed write reports an error. The menu opens the normal save dialog; cancelling writes nothing.

## 3. Verifying UI work

For any UI change, look at the result:

1. `cargo run --release -p filmcraft -- --control 9876`
2. Drive the feature through the control channel or the MCP bridge: commands, then clicks and drags
   by automation id.
3. Assert on `ui.inspect`, `ui.elements` and `sequence.inspect`.
4. Take `ui.screenshot` (the whole window and the panel you changed) and open the PNG.

Screenshots you commit must follow [AGENTS.md](../AGENTS.md) §1: FilmCraft only, openly licensed
media, sidecar and `ATTRIBUTION.md` entry.

# Part 2: developing FilmCraft

## 4. Orientation, in this order

1. [AGENTS.md](../AGENTS.md): absolute rules (assets, clean-room, licences).
2. [CLAUDE.md](../CLAUDE.md): working instructions and non-negotiables.
3. [ROADMAP.md](../ROADMAP.md) for the summary, then [gaps.md](gaps.md) (the ranked shortfalls) first,
   [target-app-parity.md](target-app-parity.md) and [roadmap.md](roadmap.md) (milestones, current focus).
4. [architecture.md](architecture.md), then the README and tests of the crate you'll touch.
5. [contributing.md](contributing.md) (how to add things, gates) and [testing.md](testing.md).

Maintainers also keep a local planning folder, `plan/`. It is gitignored and not in the repository,
and holds the task-level plan, the status file and behaviour reference notes. If you don't have it,
everything you need to contribute is in the public docs above. Ask a maintainer for a task id.

## 5. Autonomous work loop

1. **Orient.** Pick the next task: the next unchecked task in the maintainer status file, or an open
   [gaps.md](gaps.md) entry. Prefer those gaps (stability, `.prproj`, camera media, measurement,
   hardware, Windows/Linux) over adding more checklist items. Read the relevant
   architecture section and crate README.
2. **Plan tests first.** Write down the acceptance test before writing code.
3. **Implement and test.**
4. **Verify.** Run all gates (`cargo xtask ci`). For UI work, run the app with `--control`, drive it
   and look at the screenshots (§3).
5. **Record.** Update the crate README (behaviour decisions, test results, limitations) and
   the progress docs when a milestone moves (ROADMAP.md, docs/roadmap.md, the relevant parity doc
   and gaps.md, with their timestamps). Commit as `M<n>.<k>: …`, then move on to the next task.

For parallel agents, use one git worktree and one `CARGO_TARGET_DIR` per agent, and keep each crate
with one owner ([contributing.md §5](contributing.md#5-parallel-work-several-agents-or-worktrees)).

## 6. Decision rules

Don't block on choices you can make yourself:

- **Behaviour.** Match the publicly documented and observable behaviour of professional editors
  (help pages, published shortcut lists, using the app). Never inspect application internals
  ([AGENTS.md](../AGENTS.md) §2). If behaviour is undocumented, choose what editors most commonly
  expect and record the choice in the crate README.
- **Ask a human only about:**
  - licences outside the allowlist;
  - spending money;
  - publishing or pushing;
  - personal data;
  - destructive git operations;
  - anything AGENTS.md leaves unclear ("when in doubt, leave it out").

## 7. Definition of Done

A feature is done when:

- [ ] its command(s) are registered with label, menu path, shortcut (if any) and a parameter doc;
- [ ] engine tests cover it, including undo/redo and the disabled case;
- [ ] the UI is wired: menu, panel control and shortcut;
- [ ] every new interactive widget has an automation id;
- [ ] you have driven it through the control channel and reviewed a screenshot;
- [ ] all gates pass (`cargo xtask ci`), and new assets have sidecars;
- [ ] the crate README and the progress docs (ROADMAP.md, docs/gaps.md, the parity docs) are updated where relevant;
- [ ] it is committed with its task id.

### Reporting progress honestly

ROADMAP.md and [target-app-parity.md](target-app-parity.md) keep two numbers: **feature breadth**
(does it exist?) and **ready for real work** (does it hold up?). When you update either:

- say how a number was obtained: *measured* (a test, a diff, a benchmark you ran) or *estimated*;
- count an approximation, a stub or a partly wired setting as such, never as done;
- record known bugs and limitations in the crate README and [gaps.md](gaps.md), not only
  what works;
- benchmarks state the machine, the load average and before/after on the same build;
- don't raise a percentage without evidence a reviewer can rerun.

### Process restart and pending Poema plans

Guarded read tickets and prepared previews are scoped to a randomly namespaced
native Session, not only its revision or a process-local counter. Restarted owners
refuse old tickets even if an identical project recreates the same ids and revision.
A reconnect must read again and build a new plan; it must never replay a pending
edit. This namespace is a concurrency fence, not authentication. Native Undo and
prepared previews remain session memory; this change does not persist them across
restart, implement project-file ownership, or activate a Poema film.

### Poema-managed project-file ownership (Unix)

`poema.owner` reports the opaque Session owner, canonical project path, managed
status and current revision without changing history. It is read-only identity,
not authorization.

A project containing Poema editorial bindings, or a path with a persistent
`.poema-owner.lock` sidecar, requires an exclusive OS file lease before open/save.
The sidecar remains after close; process death releases the lock. Symlink aliases
resolve to the same owner. Hardlinked projects/sidecars, missing/replaced locks and
unsupported platforms refuse. Save As acquires its destination before changing
memory; Save a Copy retains the source lease. Saves use the format's atomic writer.
Queued auto-saves retain the lease until their snapshot is released. Direct edit,
Undo/Redo and command paths refuse if ownership is lost. Ordinary unmarked projects
keep their existing behavior. Schema 17 excludes older native readers that do not
honor this protocol. This is cooperative ownership, not authentication or a fence
against arbitrary external programs. Deployment must also exclude old Poema servers
from opted-in data roots. This piece does not publish a host ownership binding,
restore an Undo stack after restart, or enable any real film.

### Guarded checkpoints and file generations

`poema.owner` also reports the opaque file-lease generation and whether the native
project is dirty. Reopening a released file creates a new generation even in the
same Session; Save As changes it and Save a Copy retains the source generation.
`poema.checkpoint {token, lease}` validates the unchanged native read basis and
file generation, checks/bounds the project, and atomically saves inside one command.
Its reply identifies the exact revision/path/owner/lease and SHA-256 of the bytes
saved. The host must independently check the file digest and semantics. Checkpoint
is persistence, not an edit: no new Undo step or command-journal event. A stale
basis, replaced file owner or redirected path refuses before writing. This does
not publish a host binding, freeze human input, or recover unclean unsaved work.

### Publication freeze

`poema.freezeCheckpoint {token, lease}` performs the same guarded checkpoint and
atomically holds its file-write generation frozen. The reply adds a transient
`freeze` identity. While held, only `poema.owner`, `agent.snapshot`,
`agent.document` and `poema.releaseCheckpoint` may execute. Direct edits,
Undo/Redo/revert, document replacement and saves refuse too. Read-only control
queries and the pushed journal remain available. No project edit, Undo entry or
journal event is created. `poema.owner.frozen` reports the state, not its release
identity. There is no timeout, automatic thaw or edit replay.

`poema.releaseCheckpoint {lease, freeze}` requires the exact original pair and
releases once; foreign, extra and repeated arguments refuse. Both transitions
discard pending agent plans/tickets and owned-Undo claims, without discarding
native history. Obtain a fresh basis afterwards. These local concurrency
identities are not authentication or protection against arbitrary external code.

Foreground saves and freeze/release advance an in-memory persistence generation.
Queued autosaves capture that generation and check it under the same mutex as the
atomic file write, so an old worker cannot overwrite a checkpoint after release.
The recovery ring remains available; a refused stale current-project save is
reported. Unmarked projects have no managed generation and behave as before.
This is a temporary native prerequisite, not a host binding transaction, a
persisted Undo stack, or a public film switch. The controller must retain the
freeze while publication is uncertain and reconcile rather than guessing.

### Explicit clip properties

`poema.patchClip {clip, gainDb?, enabled?}` patches only the supplied properties
on the addressed clip. Supply at least one: gain is finite and in `[-60,12]` dB,
enablement is a boolean. Unknown fields, absent clips and locked tracks refuse.
Speed, source/record geometry, track mute and provenance remain unchanged. The
delivery configurator's defaults are deliberately not used for property edits.

The guarded batch/preview door admits this command and `timeline.setTrack`.
Mixed operations stage and check together, then commit as one native Undo and
one journal event. Host admission and policy are separate: this prerequisite
does not migrate a Poema caller or enable any real film.

### Poema native audition (B4a3 prerequisite)

`agent.monitor {token, updates:[{track:<native audio id>, muted?:bool, solo?:bool}]}`
changes 1–128 explicitly addressed audio audition settings under the same read guard.
`poema.setMonitor {updates}` is its direct native command. Poema-tagged sequences route
pure audio M/S calls from `timeline.setTrack` and `mixer.setStrip` to this same door.
Ordinary FilmCraft projects retain their existing versioned M/S behavior.

The renderer reads the actual native track flags. Bounded per-sequence
`poema.monitor.v1` intent preserves both flags, including explicit false, through
native Undo/Redo and project save/reopen. Audition invalidates stale reads and
publishes one journal event on change, without adding history or clearing Redo.
A valid owned edit-history claim follows the audition revision; an invalidated
claim is never resurrected. Corrupt intent refuses before consuming history.
Mixed/versioned M/S on a monitored sequence refuses until separately proved.
Host human/agent routing is still planned in the next stacked piece; this
prerequisite does not enable a real film or write a Node monitor overlay.

`agent.previewMonitor {token, updates}` evaluates the same audition command on a
private native Session clone and returns `{preview:true,basis,changed,tracks,document}`.
It publishes no live project/history/journal change and writes no project. The
returned document is policy input, not a live ticket or a reusable edit plan.
A later `agent.monitor` must still present the original basis and pass the guard.
Host policy may refuse the clone without sending an audition.

Audition and history restoration validate that persisted monitor intent agrees
with current native flags before consuming input or history. A parseable but
contradictory monitor map refuses atomically; the command does not silently repair it.

### Exact stereo-centre channel sum (B4b1)

`poema.setChannelSum {clip, enabled:bool}` manages only the exact static native
Channel Mixer: left/right output each receive 50% left plus 50% right, with no
inversion. It is admitted to guarded native batches/previews and normal Undo.
A no-op does not add history. Unrelated/animated/duplicate mixer state, mapped
channels/streams, non-audio/locked targets and non-stereo source layouts refuse
without replacing effects. No DSP implementation or project schema is copied.

### Optional clip-local audio fades (B4b3 prerequisite)

`poema.setAudioFades {clip, fadeIn?, fadeOut?, fadeInCurve?, fadeOutCurve?}`
patches an unlocked explicit audio clip through the native edit/preview/Undo
surface. At least one property is required; durations are 0..5 seconds at saved
six-decimal precision, curves are `equal_power` or `linear`. The optional native
`clip_audio_fades` intrinsic owns these physical values. It is never installed
on ordinary clips by default. Static/unique/unmasked envelope state only;
unrelated or animated envelope state refuses. Unspecified properties survive,
including a linear curve at zero duration. Exact no-ops add no history.

Playback and export evaluate the same sample-local envelope. In/out gains
multiply even when they overlap or exceed the clip length, without replacing
other track material. Fade-out starts retain the existing Poema millisecond
rounding of the actual binary64 value; duration/sample conversion is explicit.
Continuation-offset envelopes and source-drift semantics are not admitted by
the host; host routing/import remain separately gated until their proofs.

Changing an existing envelope preserves its native effect-list position. A
change-and-revert preview is a real no-op even beside another audio effect.

A guarded preview also recognises the optional envelope's non-rendering list
position (and its exact positive-zero/equal-power neutral form). It compares
bounded private copies; all other fields and other effects' relative order stay
exact. A semantic no-op commit retains the original project pointer, revision,
history and journal. Neither live project nor staged audio is rewritten by the
comparison; the ordinary renderer is unchanged. Projects with no supported
optional envelope retain the exact existing comparison. `agent.apply` retains
its existing commit semantics; this rule belongs to preview/commitPreview.


### Explicit native audio laws

`poema.setAudioLaw {track, stereoBalance, monoSource}` sets physical native
mixer state on an unlocked audio track. Both fields are required:
`stereoBalance` is `equalPower` or `linear`; `monoSource` is `dualMono` or
`equalPower`. It uses the normal command, guarded preview/batch, clone/check,
Undo and journal path. Unknown fields/enums and video/locked targets refuse.

Defaults preserve existing FilmCraft behavior, including deserializing older
files. Poema candidate imports explicitly select linear stereo balance and
an equal-power mono-source upmix. The latter applies before nonlinear clip
DSP, only for an actual mono source feeding stereo. Static linear balance uses
the existing Poema dead zone and four-decimal coefficients; automated balance
uses continuous linear coefficients. Ordinary native pan and source defaults
are unchanged. These are renderer/mixer fields, not editorial annotations or
host DSP. Schema 18 prevents older readers from silently losing these laws;
the 17-to-18 migration preserves old values/defaults. Existing schema steps,
licenses and notices are unchanged. Host import/readBack and PCM parity are
qualified separately against unchanged Poema rendering.


### Native scalar cubic keyframes (B4c prerequisite)

`poema.setScalarCurve {clip, effect:index, param, points}` sets physical native
Float keyframes on an unlocked clip. `points` is empty (clear) or 2..4096
strictly increasing bounded source-tick times, each `{time,value,inHandle?,
outHandle?}`. Handles are exact finite `[seconds,valueDelta]` vectors. Values
and deltas are bounded to magnitude1e6. No provenance/numeric metadata overlay.
The regular staged/guarded command, clone/check, Undo and journal apply; exact
no-ops do not edit. Unknown fields, wrong targets and malformed input refuse.

New `Cubic` interpolation evaluates these vectors, clamps their time reach to
the segment, and holds edge values. Missing both handles means linear; a missing
one-sided partner uses a flat tangent. Equal endpoint values retain a vertical
hump. Existing influence-only Bezier and every old constructor/default are
unchanged; schema19 appends a default-preserving migration so old readers
cannot silently lose the new fields/mode.

The scalar human graph exposes Cubic and two-axis handle drags through existing
`effects.setKeyframe` with `inHandle`/`outHandle` (or null to clear).
On a first drag of a linear segment, materialize the other linear control in
the SAME native edit, preventing a shape jump or extra Undo. Whole-curve
commands preserve the supplied missing handles exactly instead. Escape discards
the held preview. Existing non-Cubic graph behavior is unchanged.

This is native representation groundwork, not admission of Poema `set_curve`.
Poema curve projection, additive clip/track offset lanes and legacy render
sampling/rounding compatibility still need separate proofs. Existing Native
Volume playback evaluates on its64-sample grid; native interchange continues
to report its existing handle-loss limitation. No promise that Native export
or host curve routing is complete. The Poema/external-adapter code stays
unchanged and unsupported curve state remains planned.


### Guarded explicit moves (B5a1)

The native `timeline.move` command is admitted to guarded apply/preview only
with explicit `linked:false, insert:false, copy:false`, 1..4096 unique
`{clip,track,time}` entries, numeric existing track addresses and nonnegative
bounded integer ticks. Unknown fields, duplicates, copy/insert/new-track modes
and implicit link expansion refuse before staging. The ordinary human command
retains its existing behavior. A guarded move uses that same engine command
inside the private Session; a failed suffix publishes neither an earlier edit
nor a journal event. Preview/commit is one native Undo, preserving source clocks,
carriers and unrelated parked tracks. Other move profiles need their own audit.

### Tagged Poema clip-curve provenance

For a Poema-tagged Volume.level curve, direct human effects.setParam,
effects.toggleAnimation and single-key add/delete/move/set/interpolation edits
stamp human by/pin and physical presence in the SAME native Undo step. Staged
agent operations and ordinary FilmCraft clips retain existing behavior.
`poema.gainCurve.by.v1`, `.present.v1` and `.basis.v1` are nonnumeric editorial
intent and a digest; all actual scalar values/times/handles remain keyframes.

Basis encoding is SHA-256: ASCII domain `poema-clip-gain-curve-v1` plus NUL;
static Float64LE; countUInt32LE; for each key: timeInt64LE, valueFloat64LE,
interpolation byte (Linear0,Hold1,Bezier2,AutoBezier3,ContinuousBezier4,EaseIn5,
EaseOut6,Cubic7), out/in influenceFloat64LE, then in/out vector flagByte and
optional dt/dvFloat64LE. Bound4096. A fixed independent vector and host byte
encoder prove agreement. Unknown tool edits invalidate the basis and host
readBack refuses rather than inferring unchanged agent provenance. Multi-key
moves and other yet-unproved tools require reconciliation until their provenance
path is proved; native Undo restores the original project/metadata atomically.
No schema/default/renderer change. Host curve writes and rich import stay gated.

A nonempty human curve edit pins the clip; clearing its last curve keys preserves
the existing protection, matching Poema set_curve's early clear return. By/presence
and the digest still record the human clear in the same native Undo step.


### Explicit scoped native sequence preview

`agent.previewSequence {token, sequence, label?, operations}` is an explicit
project-owned sequence transaction. `token` remains the current ACTIVE-sequence
read ticket; a ticket read for another sequence still cannot authorize a write.
`sequence` must be an exact unsigned id of an existing native sequence. The same
1..256 audited in-memory command/parameter rules, clone/check, eight-preview bound,
60-second expiry, editor-state and project/revision/owner fences apply.

Only the private clone switches sequence. Its selection is cleared before the
commands run, and its entire interactive EditorState is restored before staging
returns. A whole-project postcondition also refuses changes outside the scoped
sequence item and monotonic native id allocation: shared-media/catalog changes
need a separate project transaction proof. The live editor's tab, playheads, selections, clipboard and preferences
never change during preview or commit. `document` is the full project with the
original active sequence; read the requested sequence directly from that project.
No client project replacement or implicit live sequence activation is accepted.

Use the existing `agent.commitPreview {token,preview}` and discard/owned history
doors. A changed commit is one project Undo entry and one ordered journal event;
watch subscribers receive it without polling or sequence filtering. No-op,
refused, discarded and expired previews publish no edit. The existing active
`agent.preview`/`agent.apply` behavior and read-ticket guards remain unchanged.

### Guarded append of one stereo audio track (B6b2b prerequisite)

The existing `sequence.addTracks` command is admitted inside guarded batches only
with these exact parameters:

```json
{"video":0,"audio":1,"submix":0,"audioAfter":"last","audioType":"stereo"}
```

This appends one stereo audio track using the same native command/clone-and-check
path as a human edit. Missing/default/extra arguments, other counts/types and
middle placement refuse before any valid batch prefix can run. The ordinary
human command retains its existing bounded counts and placement options.

A private active or sequence-scoped preview allocates only on its clone. Commit
is one native project Undo and one pushed edit event; owned Undo/Redo restores the
complete project. A scoped append preserves the foreground tab, selection,
clipboard, playheads, preferences, other sequences and catalog. Discard, stale
editor state or a refused suffix leaves the live allocator/tracks/history intact.

The controller must discover the new native track id from the raw preview, attach
its app routing alias/default fields and configure the applicable native audio
law through commands before exposing the complete projection. The existing
human command may renumber default track names; a compatibility controller must
validate or restore old names inside that same private transaction. This native
prerequisite alone does not migrate a host caller, configure a Poema track, import
media or enable a real film. Existing lifetime/read-ticket/owner/revision and
whole-project scope fences remain in force.


### Optional native program extent (B3b4b prerequisite)

`poema.setExtentPolicy {policy:"poema"|"native"}` is a bounded native sequence
command. The policy is stored in `Sequence.program_extent`, cloned/checked,
serialized in schema20 and restored by native Undo/Redo. It is admitted to the
same active/scoped guarded batches; unknown/missing/extra arguments refuse.
`native` removes the override; ordinary sequences and older files keep the
existing furthest-item/caption duration. No ordinary editor default changes.

`poema` uses the first video track's end or a harness-protected audio item's
end when that spine has picture; without spine picture, sound defines the
program. Free audio/overlay/caption tails do not extend it. All values derive
from actual native items in integer ticks, including after split/trim/Undo.
There is no stored duration overlay or Node timeline. UI transport and export
use the same `Sequence::duration`. This is an internal integration policy,
not a new UI widget or a claim of complete Poema rollout.

Acceptance and full suite results must be reported before the paired host
profile is enabled. Legacy adapters/default-OFF behavior stay unchanged.

### Poema native human trim protection

For a sequence explicitly using the Poema program policy, a direct `timeline.trim` marks actually changed identity-bearing target clips as human protected inside that command's clone/check/Undo. Untouched/locked targets, no-op geometry, ordinary native sequences and clips without identity retain their prior behavior. Agent-staged commands remain agent edits and do not acquire an automatic human pin. Source/record/carrier data is not rewritten by this annotation. Other direct human commands require their own integration proof.
