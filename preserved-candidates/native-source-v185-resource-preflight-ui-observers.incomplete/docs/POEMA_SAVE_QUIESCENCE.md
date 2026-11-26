# Scoped native Save worker coordination

The private project_save::quiescence::with_quiescent helper uses the original
Session and original human Save preparation/publication service. It adds no
native/model write command, filesystem custody, overwrite grant, durable receipt,
manager path adoption or composer tool.

The exact original idle-writer predicate and editable-owner guard run before
coordination. The original persistence worker acknowledges Save only after older
I/O and pending recovery serialization settle. While held it leaves ordinary
messages in their original FIFO, including Clean, Prefs, Flush and explicit
AutoSaveNow. Save has its own exclusive lease and control wake channel; releasing
it cannot clear an independent workspace-update pause. Stop before/after the
acknowledgement and dropping the lease queue its matching release once.

The worker suppresses expired timers until a matching release marker, queued after
the owner's final original Clean/Dirty notification. Queued explicit autosaves
refuse while that state drains. This prevents old pending state overwriting newly
saved bytes. Finish acknowledges the marker; failure still releases the worker.
A caught callback panic synchronizes the resulting state before release and
reports an uncertain file outcome rather than rollback or retry.

Original video and audio preview producers retain an activity lease from before
folder creation through their final rename/catalog/result write. A Save gate
blocks new producers and waits for existing leases, checking Stop/deadline every
10ms. It neither cancels human rendering nor adopts its work. The original job
panic guard and failed/unlaunched closure drop also release those leases. Main
owner preview migration during Save uses the original implementation.

This foundation still needs actual typed file/auxiliary custody, complete
publication budgets and conditional publication/receipt-first recovery, followed
by original manager/conversation/composer adoption and full product acceptance.
