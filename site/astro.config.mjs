import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import release from './src/data/release.json' with { type: 'json' };

export default defineConfig({
  site: 'https://splinterm.com',
  output: 'static',
  integrations: [
    starlight({
      title: 'Splinterm',
      description: 'User and developer documentation for Splinterm.',
      favicon: '/favicon.svg',
      logo: {
        src: './src/assets/splinterm-glyph.svg',
        alt: 'Splinterm',
      },
      customCss: ['./src/styles/starlight.css'],
      lastUpdated: false,
      head: [{ tag: 'meta', attrs: { name: 'splinterm-release', content: release.version } }],
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Documentation home', slug: 'docs' },
            { label: 'Current status', slug: 'docs/status' },
            { label: 'Roadmap', slug: 'docs/roadmap' },
            { label: 'Installation', slug: 'docs/install' },
            { label: 'Upgrade and rollback', slug: 'docs/packaging' },
            { label: 'Quickstart', slug: 'docs/quickstart' },
            { label: 'CLI reference', slug: 'docs/cli' },
            { label: 'Core concepts', slug: 'docs/concepts' },
            { label: 'Why native Wayland?', slug: 'docs/wayland' },
          ],
        },
        {
          label: 'Use Splinterm',
          items: [
            { label: 'Sessions and persistence', slug: 'docs/sessions' },
            { label: 'Configuration and keymaps', slug: 'docs/configure/configuration' },
            { label: 'Dojo presets', slug: 'docs/presets' },
            { label: 'Remote access', slug: 'docs/remote' },
            { label: 'Troubleshooting', slug: 'docs/troubleshooting' },
          ],
        },
        {
          label: 'Automation and integrations',
          items: [
            { label: 'Automation and permissions', slug: 'docs/automation' },
            { label: 'MCP adapter', slug: 'docs/mcp' },
          ],
        },
        {
          label: 'Development',
          items: [{ label: 'Contributor guide', slug: 'docs/development' }],
        },
      ],
    }),
  ],
});
