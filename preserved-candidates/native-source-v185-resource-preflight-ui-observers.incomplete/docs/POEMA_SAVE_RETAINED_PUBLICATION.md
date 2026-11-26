# Retained atomic file publication foundation

This private native foundation supports safe staged file effects for the original
Save service. It is not a composer tool or a complete Save/Save As transaction.
The human serializer/publication service remains shared. Original atomic_write
and the retained stage share complete-byte writes and file fsync. Ordinary human
atomic_write keeps its existing replacement and best-effort directory-sync policy.

A single-use StagedWrite owns actual open parent/operation directory handles,
complete current destination and payload observations, an independent archived
payload inode and an exchange slot. Its new private operation directory receives
a durable immutable intent, then a durable dispatch marker before target I/O.
An absent target uses kernel no-replace; an existing target uses kernel exchange.
No ordinary rename fallback exists. Exchange keeps the ACTUAL displaced entry,
including a replacement in the final check-to-kernel gap. It never automatically
swaps back or deletes a possible later human file. Open-file writers remain
visible on the displaced inode, while the payload archive has a separate inode.

Directory sync is checked before a successful retained receipt. A failure after
kernel publication, missing durable receipt, Stop or late output change is
uncertain. A displaced file differing from the exact prior observation is an
explicit retained conflict. Immutable receipts never overwrite foreign records.
A failed or dropped stage leaves its artifacts. Fresh inspection reads all bytes,
then fences earlier files and metadata; it describes current physical state and
historical receipts with no dispatch/retry/cleanup, native Undo or startup rights.
Copied metadata cannot reconstruct the live Rust staging owner. Recovered files
still need the original native/current-manager ownership and restoration policy.

The verified APFS pilot uses safe rustix1.1.5 filesystem wrappers, already in the
pinned dependency graph, with no new product unsafe code. Exchange publication is
currently compiled on Apple/Linux. Other platforms and unsupported filesystems
need a separately verified preserving provider before this is a portable Save
feature; they must not silently use ordinary replacement. Neither Linux product
acceptance nor power-loss durability is established by the Mac process-exit test.

Composition must still hold the genuine native autosave/preview leases across
actual file/auxiliary publication and unknown-outcome reconciliation; supply
current SSD identity, full space budgets (including TWO project-payload copies,
retained old bytes and metadata), exact Root/path/overwrite authority; preserve
schema backup/preferences/preview behavior; and bind native saved/history/path
and manager/conversation adoption. The existing scoped callback helper alone is
not an unknown-outcome policy. Complete ordinary composer Send integration and
full human-tool/runtime/model/installed/physical qualification remain required.
