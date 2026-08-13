---
title: Roadmap
description: Product direction from the current public alpha toward a supported persistent workspace.
---

Splinterm's roadmap uses **Now / Next / Later / Explore** horizons instead of release dates. These horizons describe intended product outcomes. They are not delivery dates, implementation order, compatibility guarantees, or promises that every listed idea will ship.

[Current status](/docs/status/) remains the authority for what works today. The repository [product roadmap](https://github.com/OldJobobo/splinterm/blob/main/docs/product-roadmap.md) contains the full strategic rationale, while the [engineering roadmap](https://github.com/OldJobobo/splinterm/blob/main/docs/roadmap.md) records dependency order and delivery gates.

## Now: make the public alpha a confident daily driver

The current priority is to make persistence understandable, desktop behavior coherent, and installation trustworthy on the validated x86_64 Omarchy/Arch environment.

Planned outcomes include:

- recognizable named, pinned, disposable, restorable, and expired Lair states;
- clear save, restore, pin, delete, and bounded-retention controls without persisting terminal contents or secrets;
- exact theme fidelity and supported Omarchy desktop integration;
- bounded local-file drop path insertion in Alpha3, with clipboard-image saving retained as later work;
- stronger installation, upgrade, recovery, diagnostics, and automation-consent journeys; and
- a passing beta performance and memory gate, or an explicit product disposition.

This horizon succeeds when a new user can install Splinterm, organize work, close its Window, return safely, and predict destructive actions without maintainer assistance.

## Next: define a supported 1.0 contract

A supported release requires more than implemented features. It requires an explicit and testable relationship with users.

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

- productized native remote profiles, connection diagnostics, and SSH recovery;
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
