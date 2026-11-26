# Pending native Save worker owner

The original Session now owns its actual autosave SavePause, preview WriterPause,
exact prepared original Save body, before/after native basis, actual retained
physical document receipts, and observed service return. The live Key cannot be
cloned or deserialized and is bound to one native owner, not an operation string.

Only the one original Save service is dispatched. Dispatch installs a Busy gate
before physical callouts and retains the owner on errors, caught panics, Stop and
successful returns alike. Physical receipts stay in the same owner. Competing
commands, direct edit/Undo/Redo, public preferences, persistence startup and job
pumping are fenced. The display projection carries no operation key or pauses.
Unexpected direct public-field replacement is refused by fresh original context
and exact worker pointer checks; this code does not overwrite that later state.

Before dispatch, cancellation queues the original final persistence state and
original Resume marker. The same acknowledgement receiver survives lost/Stop
observations. Retrying observation never queues a second marker. The native gate
and preview pause remain until that acknowledgement is confirmed. Original scoped
SavePause::finish behavior remains unchanged.

Session destruction stops Persistence before dropping pending pause owners.
Explicit shutdown during pending Save stops the original worker without Flush,
Clean, recovery cleanup or automatic release; pending recovery/files remain.
Call-return capture has a finite budget; exhaustion refuses further physical I/O
and is explicitly incomplete, never a qualified full effect receipt.

This is private Rust integration, not a public Save RPC or app adoption. There is
intentionally no post-dispatch completion/release until the actual current Root,
filesystem/directory/overwrite/storage authority, complete preserving auxiliary
provider, native result and durable aggregate receipt are qualified. Root ticket
and manager admission, preview preservation under every original fallback branch,
recovery/capsule closure, history/conversation adoption and ordinary composer Send
remain required. Historical receipts, callback results or copied JSON cannot mint
current authority. All four original Save intents remain in the full scope.
