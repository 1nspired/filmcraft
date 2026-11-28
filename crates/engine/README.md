# FilmCraft engine

Session::execute is the shared UI/control/MCP command door. Project edits use the native clone/check and snapshot history; preview stages remain private until a guarded commit. Native geometry uses integer Tick values.

For a sequence explicitly using ProgramExtentPolicy::Poema, direct timeline.trim protects actually changed identity-bearing targets as human inside its existing checked sequence clone. Agent staging is explicitly distinguished by BridgeState.staging and never gains an automatic human pin. The annotation changes no source intervals, carriers or unrelated fields. Ordinary native sequences, identity-free/no-op targets and locked partners retain their prior behavior. A ripple moves following items without claiming they were explicitly human-trimmed. Undo/Redo restores protection with the original edit.

The bounded native tests use independent complete Project expectations, actual command journal and owned/human history, linked/locked targets, harness program duration, unchanged defaults, no-op/invalid input, agent preview/concurrent read and real-human invalidation. The paired host proof compares complete independent Poema human/agent references, Resolve snapshots and actual frame/PCM output at24/rational rates. Exact source revision and final suite results belong in the linked PRs after acceptance runs.

This covers timeline.trim only. Other direct native human tools and long out-of-program tail navigation require their separate integration proofs. No new command, widget, schema, asset or codec.

`poema.setTrackCurve` writes/clears bounded additive stereo-track gain/pan keys, checked provenance and Poema field/recipe intent in one native clone/check/Undo. Explicit imported provenance survives; generic human keyframe edits refresh the basis and author in the same transaction. Staged raw edits cannot silently reuse a stale basis. The existing timeline Pen/drag/delete tools edit the new lanes when present; ordinary menus are unchanged. Native Clear Track Keyframes bakes a held additive value into the knob, refusing gain beyond the fader's range; Poema curve clear removes the offset while keeping the knob.

Poema project lifecycle also has bounded in-memory primitives: `poema.createSequence`
creates the explicit Poema empty format (picture/audio/empty captions, native audio
laws and program extent) or copies a current native sequence item. It preserves
foreground/editor state unless activation is requested. `poema.setSequenceMetadata`
accepts an explicit target without changing the editor; omission retains its old
active-scope behavior. `poema.setSequenceRecord` stores bounded editorial catalog
properties and optionally a native item name, with identities checked against the
native aliases. Name/version/geometry cannot be stored as numeric shadows; held
versions refuse pending their separate persistent representation.

`poema.deleteSequence` refuses the last/protected/nested sequence and requires an
explicit valid fallback for an active deletion. All project changes use the native
clone/check and project-snapshot Undo. Undo does not restore arbitrary editor view
state; it keeps valid native selections/tabs with `fix_state`, as before. Agent
project transactions can compose these primitives into one checked preview/commit;
scoped previews still reject any project/catalog change outside their sequence.
Unmanaged commands/defaults and all asset/license notices are unchanged. Native
engine regressions cover these primitives, hostile arguments/allocator overflow,
source/carrier/metadata preservation, foreground, journal and Undo/Redo. Host public
lifecycle parity and exact binary qualification are reported in the paired PR;
the primitive alone does not enable assembly, held drafts or historical restores.

Sequence display names remain native `ProjectItem.name`. A distinct saved legacy document title, when declared by the editorial field set, lives in the checked native sequence record (`document_title`). Both are presentation data, not geometry; cloning and renaming preserve the legacy distinction. The record command requires title presence to match the declared fields and bounds its length.
