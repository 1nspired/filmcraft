# Preserved preview rename foundation

Original PreviewStore::move_to still owns rename/copy/remove branching and keeps
its ordinary default behavior. The new format retained::relocation primitive is
for the first rename branch only; no engine/provider/Save/composer adoption exists
in this component, and no copy/remove fallback, cleanup, replay or reverse move
is performed by this primitive.

Staging streams the full regular source through a fixed 64 KiB buffer into one
independent, synced archive in a private source-side sibling operation directory.
It checks exact source bytes, identity and metadata, canonical original source
and destination directory handles, destination absence, complete archive bytes
and immutable intent. No media-sized Vec or implicit current-directory fallback
is used. Budget one extra complete preview on the SOURCE volume plus bounded
metadata; source/destination reserve and namespace custody are caller obligations.
No media/sample/model data is downloaded or copied by the source implementation.

A single live StagedRename (not Clone or Deserialize) durably records dispatch,
then invokes the safe rustix NOREPLACE kernel move using original directory
handles. A successful same-filesystem move preserves the original file inode,
permissions and mtime. Existing or final-gap destinations are never overwritten.
Unexpected moved source data, later source recreation or output changes remain
where they actually are; the independent original archive is retained. Parent
and operation directories are synced. Immutable receipts are create-new and never
overwrite foreign records. Stop or uncertain receipt/sync observations cannot
become a safe automatic retry. Old open-file writers cannot alter the archive.

Fresh inspection streams complete bytes and fences both parent namespaces,
archive and immutable metadata. It reports current physical observations plus
historical records, with no native, overwrite, replay, startup or Undo authority.
A process exit after the kernel move may leave dispatch without a receipt; fresh
inspection can describe that move, never reconstruct its live dispatch owner.

Cross-volume physical acceptance, preserving streamed copy/source retirement,
all original fallback/ignored-error branches, destination directory creation and
full finite catalog/byte budgets remain required. Integrate them under the actual
original pending Session worker owner, current Root/file/storage authority and
aggregate durable receipt/capsule recovery before a composer Save hand or worker
release. Per-file archive/receipts alone are not complete Save/restart authority.
APFS tests are bounded synthetic filesystem checks, not full product, power-loss,
Linux/Windows/other-filesystem or real-footage qualification. Full original14-family
Poema assistant/composer/perception/NLE scope remains unchanged.
