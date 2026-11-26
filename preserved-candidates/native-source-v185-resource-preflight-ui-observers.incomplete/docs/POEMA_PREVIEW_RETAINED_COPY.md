# Preserved preview copy and source retirement

The original engine still owns rename/copy/remove policy. These format primitives
implement explicit copy and original remove effects; they do not automatically
choose fallback, authorize a native operation, or integrate an engine provider.

Copy stages two complete payloads on the DESTINATION volume: one independent
archive and one output slot. Both use the same fixed64KiB streaming helper as the
retained rename archive, with full source/file/parent checks and preserved source
permission bits for the output. Original source stays named and unchanged. Safe
rustix NOREPLACE publishes the slot within the destination volume; a late target
can never be overwritten. Immutable intent/dispatch/receipt records remain private
and create-new; archive/parents/data are synced. A fresh full inspection must bind
the exact source/output/archive/receipt before a private live continuation exists.

Only that live, uncopyable continuation can stage original source retirement.
Serialized/copied or mutated display metadata cannot mint it, and it is consumed
once. Retirement moves the original inode by NOREPLACE into a new private directory
on the SOURCE volume instead of unlinking it. A final-gap foreign source, later
source recreation, old open writer or output replacement is retained or refused;
there is no blind restore, repeated removal, reverse move or cleanup. Stop, panic
outside the primitive, lost observation or process exit never creates a retry
permission. Fresh inspection rechecks both copy and retirement records/full bytes
and grants no native/write/startup/retirement/Undo authority.

Budget TWO new full logical preview payloads on the destination volume plus
bounded metadata. Source retirement adds directory/metadata entries and relocates
the existing original inode; it does not copy another full source payload. If the
rename branch already retained a source-side archive before failing, keep and
budget that archive too: worst-case extra source1+destination2 complete payloads.
These are per-preview cache files, not another complete project/media library.
The exact finite catalog and per-volume identity/reserve/space proof are required
before a managed Save can publish; missing/incorrect/full volumes must stop.

The existing engine's default human behavior is unchanged. Full catalog/directory
creation and byte budgets, original fallback and ignored-error provider branches,
current Root/native/file/storage custody, original bounded background Save plan,
pending Session integration, aggregate durable receipt/post-dispatch worker
release, native capsule recovery, manager/path/history/conversation and composer
Send remain required. No app adoption, credentials, providers, real footage or
full qualification is introduced. Actual cross-volume/read-only/platform and
power-loss/product acceptance remain unproven; current checks are bounded APFS
synthetic filesystem tests. Full original Poema assistant/composer/perception and
all14 FilmCraft tool-family/runtime/history/distribution scope stays intact.
