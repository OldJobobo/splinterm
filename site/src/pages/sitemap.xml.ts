import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';

const escapeXml = (value: string) =>
  value.replace(/[<>&'\"]/g, (character) => {
    const entities: Record<string, string> = {
      '<': '&lt;',
      '>': '&gt;',
      '&': '&amp;',
      "'": '&apos;',
      '"': '&quot;',
    };

    return entities[character];
  });

export const GET: APIRoute = async ({ site }) => {
  if (!site) {
    throw new Error('The Astro site URL is required to generate sitemap.xml.');
  }

  const docs = await getCollection('docs');
  const urls = [
    new URL('/', site),
    ...docs.map((entry) => new URL(`/${entry.id.replace(/\/index$/, '')}/`, site)),
  ];

  const body = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
    ...urls
      .sort((left, right) => left.href.localeCompare(right.href))
      .map((url) => `  <url><loc>${escapeXml(url.href)}</loc></url>`),
    '</urlset>',
    '',
  ].join('\n');

  return new Response(body, {
    headers: { 'Content-Type': 'application/xml; charset=utf-8' },
  });
};
