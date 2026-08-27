// @ts-check
import { readFileSync } from 'node:fs';
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { stripLeadingH1 } from './src/remark-strip-leading-h1.mjs';
import { quilonDark, quilonLight } from './src/themes/quilon-code.mjs';

// The one Quilon grammar: the VS Code extension's TextMate file, read in place
// so the editor and the site can never drift apart. It rides in Astro's own
// shikiConfig, which Expressive Code picks up. Keep code-rendering config in
// THIS file: the content layer caches rendered pages and only re-renders when
// astro.config.mjs changes, so config living elsewhere (an ec.config.mjs) can
// appear to silently no-op until the .astro cache is cleared.
const quilonGrammar = {
  ...JSON.parse(
    readFileSync(new URL('../editors/vscode/syntaxes/quilon.tmLanguage.json', import.meta.url), 'utf8')
  ),
  // The grammar names itself "Quilon" (its display name); Shiki matches fences
  // by this field, and the fences are tagged lowercase.
  name: 'quilon',
};

export default defineConfig({
  site: 'https://docs.quilon.run',
  markdown: {
    shikiConfig: {
      langs: [quilonGrammar],
      langAlias: { qn: 'quilon' },
    },
    // Every docs page keeps its H1 for GitHub; Starlight renders the
    // frontmatter title as the page heading, so the in-content H1 is dropped
    // at build time rather than shown twice.
    remarkPlugins: [stripLeadingH1],
  },
  integrations: [
    starlight({
      title: 'Quilon',
      favicon: '/favicon.png',
      customCss: ['./src/styles/quilon.css'],
      expressiveCode: {
        themes: [quilonDark, quilonLight],
      },
      social: [
        { icon: 'rocket', label: 'quilon.run', href: 'https://quilon.run/' },
        { icon: 'github', label: 'GitHub', href: 'https://github.com/assapir/quilon' },
      ],
      editLink: { baseUrl: 'https://github.com/assapir/quilon/edit/main/docs/' },
      routeMiddleware: './src/route-data.mjs',
      components: {
        Sidebar: './src/components/Sidebar.astro',
      },
      sidebar: [
        { label: 'Language reference', link: '/' },
        { label: 'Types', collapsed: true, items: [{ autogenerate: { directory: 'types', collapsed: true } }] },
        { label: 'Collections', collapsed: true, items: [{ autogenerate: { directory: 'collections', collapsed: true } }] },
        { label: 'Variables', link: '/variables/' },
        { label: 'Mutation', link: '/mutation/' },
        { label: 'Functions', collapsed: true, items: [{ autogenerate: { directory: 'functions', collapsed: true } }] },
        { label: 'Expressions', collapsed: true, items: [{ autogenerate: { directory: 'expressions', collapsed: true } }] },
        { label: 'Modules', collapsed: true, items: [{ autogenerate: { directory: 'modules', collapsed: true } }] },
        { label: 'Corelib', collapsed: true, items: [{ autogenerate: { directory: 'corelib', collapsed: true } }] },
        { label: 'Concurrency', collapsed: true, items: [{ autogenerate: { directory: 'concurrency', collapsed: true } }] },
        { label: 'Memory', link: '/memory/' },
        { label: 'Tooling', collapsed: true, items: [{ autogenerate: { directory: 'tooling', collapsed: true } }] },
        { label: 'Status', collapsed: true, items: [{ autogenerate: { directory: 'status', collapsed: true } }] },
        { label: 'Roadmap', link: '/roadmap/' },
      ],
    }),
  ],
});
