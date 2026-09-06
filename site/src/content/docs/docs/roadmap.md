---
title: Roadmap
description: Shipped foundations and intended improvements for Splinterm's persistent workspace.
---

Splinterm's roadmap uses **Now / Next / Later / Explore** horizons instead of release dates. These horizons describe intended product outcomes. They are not delivery dates, implementation order, compatibility guarantees, or promises that every listed idea will ship.

[Current status](/docs/status/) identifies the shipped release and its support boundary. The [visual roadmap](/roadmap/) summarizes the same horizons. Development-branch plans are not evidence that a feature is available in current packages. Maintainer dependency order, implementation plans, and delivery gates are tracked separately from the public product repository; accepted decisions needed to understand shipped behavior are promoted into public ADRs and documentation.

## Shipped foundations

The first stable release already includes persistent Lairs and explicit restore; saved-Lair controls; optional Window-owned lifetimes and tab-organization promotion; native Omarchy theme and font-family following; bounded local-file drop path insertion; native SSH profiles; versioned packages; and policy-scoped automation.

These capabilities retain their documented limits. In particular, 0.1 has no live daemon-upgrade handoff, terminal-history persistence across daemon restarts, or reboot-transparent process survival. See [Upgrade and rollback](/docs/packaging/) and [Sessions and persistence](/docs/sessions/).

## Now: make daily work easier

Improve the human journey on the validated x86_64 Omarchy/Arch environment without renaming Lairs, Dojos, or Splints.

Intended improvements include:

- a clear first five minutes: open a terminal, split a Dojo, leave, and resume;
- recognizable work in tabs and pickers, with lifecycle consequences visible before destructive actions;
- easier discovery of controls and familiar tmux workflows;
- safer upgrades, clearer recovery diagnostics, and less confusing consent;
- better connection feedback and recovery for people using existing SSH profiles; and
- repeatable daily-driver checks and performance evidence tied to exact builds.

Compatible live upgrade handoff is future work, not a reason to skip saving work before a 0.1 upgrade. Clipboard-image saving also remains a future direction rather than part of the shipped local-file drop feature.

This horizon succeeds when a new user can install Splinterm, organize work, close its Window, return safely, and predict destructive actions without maintainer assistance.

## Next: define a supported 1.0 contract

Stable 0.1 is scoped to the documented target. A future 1.0 contract would need wider and longer-lived commitments, not simply more features.

Before a 1.0 claim, the project intends to:

- declare supported platforms, compatibility windows, release channels, and breaking-change policy;
- publish tested upgrade, rollback, reset, and recovery procedures;
- stabilize human workflows, configuration, machine schemas, and package contracts;
- establish issue reporting, security reporting, and realistic support expectations; and
- make resource limits, diagnostics, and failure behavior ordinary product knowledge.

Version 1.0 is a support contract, not a reward for accumulating features.

## Later: connect the persistent workspace

After the primary product is dependable, local, remote, headless, and authorized tool access should become intentional ways into the same work rather than separate terminal worlds.

Candidate outcomes include:

- a more cohesive experience across existing local, remote, and headless entry points;
- stable integration kits and reference journeys for tools and MCP hosts;
- visibly distinct human and automated activity inside shared topology; and
- portable workspace definitions that do not execute untrusted shell source.

This horizon does not imply a public daemon listener, cloud account, hosted control plane, synchronized secrets, or collaborative simultaneous typing.

## Explore: broader Linux support

The following are research directions rather than commitments:

- reproducible Nix and Home Manager workflows;
- additional Wayland compositors backed by compatibility matrices;
- additional distribution artifacts with coherent service and upgrade behavior;
- a carefully bounded extension model; and
- selective compatibility work driven by real applications.

An expansion should proceed only when it serves a real blocked user, can be continuously validated, has an honest support boundary, and justifies the primary-product work it delays.

## Deliberate boundaries

Splinterm does not currently promise reboot-transparent process survival, arbitrary `foot.ini` compatibility, unrestricted automation, collaborative typing, a hosted control plane, or broad Linux support without continuous validation.

The primary human workflow remains the product anchor. Automation expands Splinterm; it does not redefine it as an "AI terminal."

## Evidence and feedback

Roadmap decisions use release validation, issue patterns, documentation feedback, explicit user research, and privacy-preserving aggregate website analytics. The terminal application itself does not embed product telemetry, and website analytics cannot prove that a terminal workflow succeeded.
