# ADR 0005: daemon-launched trusted consent client

- **Status:** Accepted
- **Date:** 2026-07-18

## Context

Phase 7 must authorize terminal observation and control without allowing
terminal content or the requesting protocol client to spoof the consent
surface. Letting a requester declare itself graphical or resolve its own prompt
would not establish a trustworthy boundary. `splinterd` must remain headless,
and persistent policy requires a separate future product decision.

## Decision

For requests that lack an existing grant, `splinterd` launches a separate
`splinterm consent` Wayland client. The daemon remains headless: all Wayland,
input, rendering, and UI objects belong to that disposable client.

The daemon and consent client communicate over a private inherited file
descriptor. One-use capability material is generated from OS randomness and is
never placed in argv, environment variables, logs, or terminal content. All
other inherited descriptors are closed or close-on-exec. The exchange has
bounded framing, a short deadline, and grant-once/deny outcomes only.

A pending request and resulting grant are bound to:

- peer UID, PID, and executable identity;
- Splint ID and process incarnation;
- explicitly requested scopes;
- one daemon lifetime; and
- one bounded expiry/revocation lifecycle.

Scopes distinguish observe, scrollback, input, resize, clipboard read,
clipboard write, and terminate. Clipboard scopes authorize metadata/capability
use only; clipboard bodies remain client-local and must never enter daemon audit
records.

The trusted consent window identifies the requesting executable and scopes and
provides grant-once and deny actions. It is rendered as application UI, never as
terminal cells. Active grants and controller ownership remain visibly indicated
in the normal Splinterm window. Revocation closes affected subscriptions,
releases controllers, and rejects later operations.

Audit records are bounded metadata: timestamp/order, peer identity, Splint
identity, scopes, decision, revocation, and reason. They never contain terminal
bodies, clipboard bodies, or input bytes.

The matching sibling `splinterm` executable is the trusted first-party UI and
may implicitly receive only observe, input, and resize authority for its normal
window lifecycle. This identity check uses the same executable device/inode
binding as trusted status/revocation UI. It does not cover scrollback,
clipboard, or terminate scopes and does not authorize another executable,
automation client, or copied binary. Those requests continue through explicit
consent. This avoids prompting every time the disposable first-party window is
reopened without creating a persistent third-party allow rule.

Development bypass remains explicit, visually labeled, and unsuitable for
supported automation. No third-party grant is persisted across daemon restart.

## Consequences

- Initial authorization can work even when no normal graphical terminal is
  already attached.
- Requesters cannot approve themselves through the public protocol.
- Consent availability depends on locating and launching the matching
  `splinterm` executable; launch failure safely denies the request.
- Packaging must preserve executable identity and the daemon-to-client launch
  relationship.
- Persistent allow rules remain deliberately out of scope until separately
  approved.
