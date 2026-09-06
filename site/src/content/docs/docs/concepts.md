---
title: Core concepts
description: Understand Splinterm's persistent topology and its disposable graphical views.
---

Think **workspace → layout → pane**: a Lair contains Dojos, and each Dojo arranges Splints. The ninja names describe what you organize, not extra commands you need to learn first.

Work is persistent by default, with optional Window-owned lifetimes described below.

## Your workspace

```text
Topology
└── Lair: project atlas
    ├── Dojo: editor
    │   ├── Splint: shell
    │   └── Splint: tests
    └── Dojo: services
        └── Splint: server
```

### Topology

The complete daemon-owned catalog of Lairs, Dojos, Splints, layout trees, names, stable IDs, focus hints, and lifecycle metadata.

### Lair

A workspace for a project or session, containing zero or more Dojos. Lairs are persistent by default. An ordinary unnamed Lair can instead belong to its Window when configured that way.

### Dojo

A terminal layout inside a Lair. Arrange its Splints for editing, tests, or services, then return to that layout through a tab. A Dojo is not the tab itself: persistent Dojos remain available after their views close.

Internally, the layout is a binary split tree whose leaves are Splints.

### Splint

An individual terminal pane. It has a stable ID, terminal state, launch metadata, and a process lifecycle.

## Disposable presentation

### Window

A native Wayland toplevel managed by the compositor. It receives compositor scaling, input, clipboard, IME, and frame lifecycle events directly. A Window displays one or more Dojos. It does not own the lifetime of persistent Lairs, but closing an owning Window terminates its unpromoted transient Lair. See [Why native Wayland?](/docs/wayland/) for the practical benefits and current limits.

### Tab

A window-local reference to one daemon-owned Dojo. Tabs and their order disappear with the window. Closing a tab detaches the view. Closing the final tab also closes the Window, so Window-owned lifetime rules then apply.

## Persistent or Window-owned?

With the default `persistent-by-default=yes`, closing a Window leaves its work running in `splinterd`. With `no`, ordinary unnamed graphical Lairs end with their owning Window unless promoted. By default, creating another Dojo or explicitly naming/renaming a Dojo permanently promotes that Lair. Command-bearing XDG launches start client-bound regardless of the default lifetime, but the same tab-organization promotion applies when enabled.

Persistence does not mean survival of daemon restarts or reboots. Learn the exact [lifetime settings](/docs/configure/configuration/#terminal-lifetime) and [close, reopen, and restore behavior](/docs/sessions/).

## Lifecycle words

- **Attach:** observe an existing running Dojo or Splint through a client.
- **Detach:** remove a graphical view without terminating the underlying process.
- **Incarnation:** one process lifetime inside a stable Splint identity.
- **Restore:** explicitly start an exited Splint using saved launch metadata.
- **Controller:** the one client currently allowed to send input or resize a Splint.

The distinction between persistent topology and disposable presentation is important for both people and automation. Structured clients can mutate topology, but those operations do not imply compositor-native window control.
