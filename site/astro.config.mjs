import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://preview.splinterm.com',
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
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Documentation home', slug: 'docs' },
            { label: 'Current status', slug: 'docs/status' },
            { label: 'Installation', slug: 'docs/install' },
            { label: 'Quickstart', slug: 'docs/quickstart' },
            { label: 'Core concepts', slug: 'docs/concepts' },
            { label: 'Why native Wayland?', slug: 'docs/wayland' },
          ],
        },
        {
          label: 'Use Splinterm',
          items: [
            { label: 'Sessions and persistence', slug: 'docs/sessions' },
            { label: 'Configuration', slug: 'docs/configure/configuration' },
            { label: 'Troubleshooting', slug: 'docs/troubleshooting' },
          ],
        },
        {
          label: 'Automation and integrations',
          items: [
            { label: 'Bounded automation', slug: 'docs/automation' },
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
