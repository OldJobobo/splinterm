---
title: Core concepts
description: Understand Splinterm's persistent topology and its disposable graphical views.
---

Splinterm separates terminal process lifetime from graphical client lifetime. Its vocabulary makes that ownership explicit.

## Persistent topology

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

A named persistent session or project. One Lair contains zero or more Dojos.

### Dojo

One persistent terminal layout within a Lair. A Dojo owns a binary pane-layout tree whose leaves are Splints.

### Splint

An individual terminal pane. It has a stable ID, terminal state, launch metadata, and a process lifecycle.

## Disposable presentation

### Window

A native Wayland toplevel managed by the compositor. A window displays one or more Dojos but does not own their process lifetime.

### Tab

A window-local reference to one daemon-owned Dojo. Tabs and their order disappear with the window. Closing a tab detaches the view; it does not close the Dojo.

## Lifecycle words

- **Attach:** observe an existing running Dojo or Splint through a client.
- **Detach:** remove a graphical view without terminating the underlying process.
- **Incarnation:** one process lifetime inside a stable Splint identity.
- **Restore:** explicitly start an exited Splint using saved launch metadata.
- **Controller:** the one client currently allowed to send input or resize a Splint.

The distinction between persistent topology and disposable presentation is important for both people and automation. Structured clients can mutate topology, but those operations do not imply compositor-native window control.
