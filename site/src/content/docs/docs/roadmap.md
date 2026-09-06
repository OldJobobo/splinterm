---
title: Roadmap
description: What we’re working on, what comes next, and what is still an idea.
---

We group plans into **Now / Next / Later / Explore** rather than assigning release dates. Plans can change, and an item here does not mean it is available in the current package.

See [Current status](/docs/status/) for what you can use today, or the [visual roadmap](/roadmap/) for a shorter overview.

<span id="shipped-foundations"></span>

## Already available

The first stable release includes persistent Lairs, saved layouts, explicit restore, optional work that ends with its window, Omarchy theme and font-family following, local-file drop support, SSH access, and automation with permissions.

There are important limits: restarting the background service ends running commands and loses terminal history. Version 0.1 cannot keep those commands alive through an upgrade or reboot. Read [Upgrade and rollback](/docs/packaging/) and [Sessions and persistence](/docs/sessions/) before relying on saved work.

<span id="now-make-daily-work-easier"></span>

## Now: make the everyday easier

Focus on everyday use on x86_64 Omarchy/Arch:

- a short path from installation to opening, splitting, and returning to a Dojo;
- names you can recognize in tabs and pickers, with clear warnings before ending work;
- easy-to-find shortcuts, including familiar tmux-style controls;
- clearer upgrade instructions, error messages, and permission prompts;
- better feedback and recovery when SSH connections fail; and
- repeatable checks for everyday use and performance, tied to the builds tested.

The aim is simple: you can organize work, return to it, and know what will end a running command without asking the maintainer.

Keeping commands alive through compatible background-service upgrades is future work. Saving clipboard images is also a future idea, not part of the current local-file drop feature.

<span id="next-define-a-supported-10-contract"></span>

## Next: prepare for 1.0 support

Stable 0.1 supports the documented setup; it is not a long-term-support promise. Before 1.0, we want users to know what will keep working between versions and how long it will be supported.

That means:

- clear supported platforms, support periods, release channels, and rules for breaking changes;
- tested upgrades, downgrades, resets, and recovery steps;
- reliable settings, commands, tool interfaces, and packages;
- straightforward bug and security reporting; and
- documented resource limits and what happens when something fails.

More features alone will not make Splinterm ready for 1.0.

<span id="later-connect-the-persistent-workspace"></span>

## Later: make remote work feel familiar

Working on this machine, over SSH, without a graphical desktop, or through an authorized tool should feel more consistent.

Ideas include:

- familiar controls across local and remote work;
- better examples and support for tool and MCP integrations;
- a clear difference between your actions and a tool’s actions; and
- shareable workspace descriptions that are data, not executable scripts.

This does not mean cloud accounts, syncing secrets, a public daemon port, or several people typing into the same pane at once.

<span id="explore-broader-linux-support"></span>

## Explore: more Linux setups

We are considering Nix and Home Manager, other Wayland desktops, packages for additional distributions, extensions with clear limits, and compatibility fixes for applications people use.

Before promising support, there needs to be a real need, a way to test the setup regularly, and time to maintain it. These remain ideas, not commitments.

<span id="deliberate-boundaries"></span>

## What we are not promising

- Keeping commands running through a reboot.
- Support for every `foot.ini` setting.
- Tools acting without permission.
- Several people typing into one pane at once.
- A cloud service for managing your terminals.
- Support for Linux setups we cannot regularly test.

Splinterm is a terminal for people first. Automation is optional help, not its whole identity.

<span id="evidence-and-feedback"></span>

## What guides the work

We use release tests, bug reports, documentation feedback, user research, and privacy-preserving aggregate website analytics. The terminal application does not collect product telemetry, and website visits cannot tell us whether a terminal workflow succeeded.
