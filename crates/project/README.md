# FilmCraft project model

Projects, sequences and clips are plain serde data owned by the native engine. Edit geometry uses exact `filmcraft_time::Tick`; clone/check and snapshot Undo live in the engine.

An optional `Sequence.program_extent` policy preserves Poema’s program extent without moving, dropping or shortening stored media. `None` keeps FilmCraft’s existing furthest-item/caption duration. `Poema` measures the first picture track or a harness-protected audio anchor; with no picture spine it measures sound. The duration is computed from current native items, so edits, splits and Undo remain authoritative. There is no separately saved duration number. Schema20 prevents an older reader silently discarding this policy.

Project/engine/format tests cover native defaults, serialization, exact rational/extreme ticks, dynamic source anchors, guarded transactions and Undo. Actual render/suite results are reported only after the paired acceptance gates run. No ordinary editor default or media dependency changes.

`Sequence.content_duration()` retains the original furthest-media/caption extent for native view navigation. Timeline Fit, horizontal clamps, wheel limits and zoom scrollbar use it even for an explicit Poema programme. It is computed, never serialized; programme playback/render duration and its policy are unchanged. A long parked tail stays editable without lengthening the render.
