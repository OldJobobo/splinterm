---
title: Splinterm documentation
description: Start using and understanding Splinterm's persistent terminal environment.
template: splash
hero:
  tagline: Keep the shell. Replace the window.
  actions:
    - text: Start the quickstart
      link: /docs/quickstart/
      icon: right-arrow
      variant: primary
    - text: Check current status
      link: /docs/status/
      icon: information
      variant: secondary
---

import { Card, CardGrid } from '@astrojs/starlight/components';

Splinterm is a native Wayland terminal backed by a headless daemon. This documentation begins with ordinary human workflows and keeps implementation history in a separate development section.

<CardGrid>
  <Card title="Install and begin" icon="rocket">
    Check the validated environment, install the private prerelease, and open your first terminal.

    [Read the quickstart →](/docs/quickstart/)
  </Card>
  <Card title="Understand persistence" icon="seti:folder">
    Learn how Lairs, Dojos, Splints, tabs, and windows fit together.

    [Explore the concepts →](/docs/concepts/)
  </Card>
  <Card title="Configure Splinterm" icon="setting">
    Set fonts, sizing, shell behavior, scrollback, pane chrome, and theme overrides.

    [Open configuration →](/docs/configure/configuration/)
  </Card>
  <Card title="Develop and integrate" icon="puzzle">
    Find contributor checks, architecture, automation contracts, and specialist references.

    [Enter development docs →](/docs/development/)
  </Card>
</CardGrid>

:::caution[Private prerelease]
Splinterm is not a supported public release. The current environment and installation path are intentionally narrow. Read [current status](/docs/status/) before relying on it.
:::
