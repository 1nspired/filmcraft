# Original Save file effects

The original human Save service remains the only Save policy and serializer.
`Prepared::publish` uses `effects::Original`; `publish_with` runs the same body
with a trusted native file-effects implementation. Preferences and preview moves
also expose their original operations through this interface. This is a private
Rust composition boundary, not an RPC, model tool, permission or startup grant.

Events report individual call returns. A successful return does not prove current
bytes, ownership, native completion or rollback. Original preferences and preview
migration still ignore the same auxiliary errors; preview rename/copy/remove order
and unconditional final catalog update are preserved. Independent frozen original
implementations test these policies, including failures and existing destinations.

On supported native filesystems, `RetainedDocuments` consumes a live Prepared Save
and admits only its exact project/schema-backup/preferences role, literal path and
complete original payload. Per-role single-use retained publication keeps displaced
files and immutable intent/dispatch/receipts. Fresh complete inspection must confirm
the exact durable receipt before the original service can see a document success.
The caller can recheck documents after auxiliary work without replaying publication.
A historical saved baseline never authorizes overwriting later changed current bytes.

This receiver does not provide preview custody, current directory/overwrite authority,
manager adoption, a public composer Save hand, or a long-lived native operation owner.
It delegates preview physical effects explicitly. The composing controller must hold
original autosave and preview pauses across uncertain file/native outcomes, provide
preserving auxiliary-file effects and qualify every ignored auxiliary result. The
scoped quiescence helper alone is insufficient because it releases on scope exit.
No receipt or copied JSON may reconstruct dispatch ownership or authorize startup.
Ordinary human Save remains unchanged; the candidate has not been adopted by the app.
