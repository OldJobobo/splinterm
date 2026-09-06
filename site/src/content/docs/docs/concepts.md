---
title: Core concepts
description: Meet Lairs, Dojos, and Splints, and learn what happens when you close a window.
---

A **Lair** holds your workspace. A **Dojo** arranges its terminal panes. Each pane is a **Splint**.

For example, you might keep your editor and tests in one Dojo, with your services in another:

## Your workspace

```text
Your work
└── Lair: project atlas
    ├── Dojo: editor
    │   ├── Splint: shell
    │   └── Splint: tests
    └── Dojo: services
        └── Splint: server
```

### Lair

A workspace for a project or session, containing zero or more Dojos. By default, its work keeps running when you close the window. You can also configure ordinary unnamed Lairs to end with their window.

### Dojo

A layout of Splints inside a Lair, shown through a tab. Put an editor beside your tests, or keep a few service logs together.

A Dojo is not the tab itself. A persistent Dojo still exists after you close its tab, so you can open that layout again later.

### Splint

One terminal pane and its process. It has a stable ID and remembers how its process was launched. A process can exit while the Splint remains available for an explicit restore.

<span id="disposable-presentation"></span>

## Windows and tabs

### Window

The part of Splinterm you see on your Wayland desktop. A window displays one or more Dojos and handles your keyboard, mouse, clipboard, and display scale.

The background service, `splinterd`, runs the shells. Closing a window leaves persistent Lairs running. If a Lair still belongs to that window instead, closing it ends the Lair’s processes.

### Tab

A view of one Dojo inside a window. Each window has its own tabs and tab order. Closing a tab removes that view; closing the final tab also closes the window, so its lifetime rules apply.

## Persistent or Window-owned?

**Persistent** means work can keep running after its window closes. **Window-owned** means work ends when its owning window closes, unless you make it persistent first.

The default is `persistent-by-default=yes`. Set it to `no` to make ordinary unnamed graphical Lairs Window-owned. By default, creating another Dojo or explicitly naming/renaming a Dojo makes the whole Lair persistent. The docs call that change **promotion**.

An XDG launch with a command starts client-bound regardless of the default lifetime. That means it ends when its initial command exits or its owning window disconnects, unless promoted. The same tab-organization promotion applies when enabled.

Learn the exact [lifetime settings](/docs/configure/configuration/#terminal-lifetime) and [closing, reopening, and restore behavior](/docs/sessions/).

:::caution
Persistence does not keep commands running through a background-service restart or reboot. Saved layouts and launch details are not saved application state or terminal history.
:::

<span id="lifecycle-words"></span>

## Words you’ll see in the reference docs

- **Attach:** open a view of an existing running Dojo or Splint.
- **Detach:** remove a view without ending its processes.
- **Restore:** explicitly start an exited Splint again using its saved launch details. This starts a new process, not a continuation of the old one.
- **Incarnation:** one run of a process inside a Splint. Restoring it starts a new incarnation with the same Splint ID.
- **Controller:** the one client currently allowed to send input or resize a Splint.

### Topology

The background service’s complete record of Lairs, Dojos, Splints, names, IDs, layouts, focus hints, and lifecycle state. Internally, each Dojo’s layout is a tree of splits with a Splint at each leaf.

A tool’s permission to change that record does not automatically let it open, focus, move, or resize a desktop window.
