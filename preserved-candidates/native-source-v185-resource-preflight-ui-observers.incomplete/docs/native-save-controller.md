# Original native Save controller

Three finite native lifecycle commands retain the actual original Save key:
agent.beginProjectSaveOperation, agent.projectSaveOperationStatus, and
agent.cancelProjectSaveOperation. Each accepts only the distinct original job
and fresh operation IDs. Begin consumes the actual completed planning job
through the existing original handoff and its unchanged 250ms delivery window.
The native Session retains the uncopyable operation key and both original worker
pauses before returning metadata. A lost begin reply observes the same owner.

Only these lifecycle commands and original project Root validation can pass the
existing command fence while that owner is held. Every competing edit, raw Save,
new read and workspace transition remains fenced. A display projection has no
key. Status and saved JSON cannot reconstruct an operation in another Session.

Stop before begin tombstones the original job and operation. Cancellation before
dispatch joins the original Resume acknowledgement with a bounded check.
A failed cancellation restores the same live key. After any physical dispatch,
cancellation refuses and the original pending owner remains held for complete
receipt reconciliation. Its native completion/startup/write flags remain false.
Cancellation bookkeeping retains the existing 256-entry bound.

These commands publish no files and expose no new model tool. They are the native
half of the manager bridge. Actual host live-lease binding, complete filesystem
custody, per-I/O checks, native worker transport and Stop delivery, effect journal
reconciliation, durable recovery and public composer Save remain required.
The original encoder, Save service, job lifetime and file policy are unchanged.
