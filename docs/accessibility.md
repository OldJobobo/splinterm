# Navigation accessibility

The optional Lair Explorer has a Linux AT-SPI semantic tree alongside its custom
rendering. This is **navigation accessibility**, not whole-terminal screen-reader
support. Live assistive-technology and packaged integration acceptance remain
separate from the non-graphical source tests.

When the Explorer is visible in a focused, controller-authorized Window, the
adapter exposes:

- Lair → Dojo → Splint tree items, with explicit hierarchy levels;
- independent selected, current, expanded/collapsed, disabled, and busy states;
  the current Lair and Dojo can both contain the current Splint;
- a search field, visible-row result count, and bounded loading, empty, stale,
  or unavailable status, with polite announcements when status changes;
- distinct focus, expand/collapse, and activate actions, plus search-value edits.

Actions use the same Explorer decisions as local navigation. Restorable Splints
still enter the existing preview/confirmation flow; an accessibility action
cannot confirm restoration, grant control, or send terminal input. Navigation
focus changes stay inside the already-focused Window: they do not raise or focus
a compositor window. Pending navigation blocks further actions. Modal surfaces,
hidden Explorers, observer-only Windows, stale targets, and lost keyboard focus
cannot be bypassed by queued accessibility requests. Modal dialogs and other
navigation surfaces do not yet have semantic coverage.

## Identity and privacy

Item IDs are Window-local, stable across ordinary redraws/reordering, and never
reused after retirement. Removed items, changed endpoint generations, and changed
Splint incarnations cannot retarget an old item ID. Callbacks capture an owner
state epoch; the UI owner checks it against the current typed projection,
revision, parent identity, capabilities, and modal/pending state before dispatch.
An outdated callback may therefore be rejected even if its target is still
visible; assistive clients should query the current state again.

The tree contains only bounded navigation names, lifecycle/status labels, and
the navigation search query. Names/status are limited to 256 characters and
search to 64 characters, with control and bidi-formatting characters excluded.
At most 4096 items are exposed. No terminal cells, scrollback, parser state,
clipboard or image bodies, environment, process bodies, or raw daemon diagnostic
messages are published. The Terminal node is a placeholder, not terminal text.

## Service and geometry limits

Publication uses one cancellable worker per Window, a latest-state slot, and a
64-request action queue with an explicit owner-loop wake. Equivalent semantic
states do not produce repeat announcements. Focus/search requests coalesce;
other requests fail closed when the queue is full. Removed native items are
unregistered, and hierarchy, state, name, and search changes produce AT-SPI events.

Only local Unix D-Bus transports are accepted. The desktop accessibility service
must be enabled. Connection/publication attempts have a 500 ms deadline;
unavailable services retry with a two-second backoff. Publication never waits
for D-Bus on the rendering/event-loop owner, and Window teardown cancels pending
transport work rather than waiting for a bus reply. Missing accessibility
services do not prevent terminal startup. Queuing an update is not proof that an
assistive client received it.

Wayland does not disclose a Window's global screen position. No AT-SPI Component
interface, global geometry, pointer targeting, or terminal-text interface is
claimed. Real screen-reader compatibility and announcement usability still need
separately authorized graphical validation. The synthetic example described in
`accessibility-spike.md` is not evidence of that acceptance.
