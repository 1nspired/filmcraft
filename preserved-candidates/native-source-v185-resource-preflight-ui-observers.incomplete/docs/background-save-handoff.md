# Original bounded Save planning handoff

The original shared planning worker now retains its genuine Encoded result
alongside the unchanged metadata description. Read replies clone only the
description. A private native continuation removes and tombstones the actual
delivered job, moves its Vec into the same original pending Save owner, and
keeps that same job alive there. No encoded bytes or continuation can be
reconstructed from JSON, plan hashes or a recovery description.

The original two-worker/four-result/256-tombstone bounds and 45s operation,
250ms delivery limits are unchanged. Handoff rechecks native Root and complete
Save context before and after original autosave/preview quiescence. The same
original input, destination/adoption policy and serializer must match.
Foreign payloads, stale context, expired/cancelled/consumed ids and destination
requests without actual encoded output cannot prepare a Save.

The pending owner retains the original job/Stop lifetime. Its original Root
guard is checked before dispatch; its live Stop/deadline is checked during
retained file streaming and physical effect callbacks without demanding the
pre-adoption Root after the original Save adopts its destination. A stopped
pre-dispatch handoff remains cancellable. Every post-dispatch result, including
Ok, remains pending and retains original workers and physical receipts.

No new public/native command, model hand, host protocol field or app adoption
is introduced. Complete current Root/file/parent/overwrite/storage grants,
all-document budgets and directory custody, auxiliary/aggregate reconciliation,
capsule/restart/manager/history/shared-composer integration and full original
all14-family acceptance remain required. Real payload memory/cadence, physical
cross-volume and installed/runtime/model/GUI acceptance remain unproven. The
unchanged four-result cache bounds result count, not aggregate encoded bytes;
large-document memory acceptance is still required before public adoption.
