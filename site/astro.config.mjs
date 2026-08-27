// @ts-check
import { readFileSync } from 'node:fs';
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { stripLeadingH1 } from './src/remark-strip-leading-h1.mjs';

// The one Quilon grammar: the VS Code extension's TextMate file, read in place
// so the editor and the site can never drift apart. It rides in Astro's own
// shikiConfig, which Expressive Code picks up and which survives the content
// pipeline's process boundary (an ec.config.mjs does not).
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
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/assapir/quilon' }],
      editLink: { baseUrl: 'https://github.com/assapir/quilon/edit/main/docs/' },
      sidebar: [
        { label: 'Language reference', link: '/' },
        { label: 'Types', items: [{ autogenerate: { directory: 'types' } }] },
        { label: 'Collections', items: [{ autogenerate: { directory: 'collections' } }] },
        { label: 'Variables', link: '/variables/' },
        { label: 'Mutation', link: '/mutation/' },
        { label: 'Functions', items: [{ autogenerate: { directory: 'functions' } }] },
        { label: 'Expressions', items: [{ autogenerate: { directory: 'expressions' } }] },
        { label: 'Modules', items: [{ autogenerate: { directory: 'modules' } }] },
        { label: 'Corelib', items: [{ autogenerate: { directory: 'corelib' } }] },
        { label: 'Concurrency', items: [{ autogenerate: { directory: 'concurrency' } }] },
        { label: 'Memory', link: '/memory/' },
        { label: 'Tooling', items: [{ autogenerate: { directory: 'tooling' } }] },
        { label: 'Status', items: [{ autogenerate: { directory: 'status' } }] },
        { label: 'Roadmap', link: '/roadmap/' },
      ],
    }),
  ],
});
