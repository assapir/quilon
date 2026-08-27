/**
 * Rewrite the docs' relative `*.md` links to site routes.
 *
 * The pages under docs/ are rendered twice — by GitHub, where a link has to
 * name a file, and by Starlight, where the same page is a directory route
 * (`docs/types/text.md` is `/types/text/`, a folder's README.md is the folder's
 * index). Without this the file links ship verbatim into the built HTML and
 * 404 on the site, so the source keeps the GitHub-correct form and the routes
 * are derived here.
 *
 * Purely lexical, no docs root needed: a route sits one level deeper than its
 * file's directory unless the file is a README (which IS its directory), so a
 * non-README page's links need one extra `../` to climb back out.
 */
export function rewriteDocsLinks() {
  return (tree, file) => {
    const prefix = /README\.md$/.test(file.path ?? '') ? '' : '../';
    const walk = (node) => {
      if (node.type === 'link') {
        const [path, ...rest] = node.url.split('#');
        if (/\.md$/.test(path) && !/^[a-z][a-z0-9+.-]*:|^\//i.test(path)) {
          const route = path.replace(/README\.md$/, '').replace(/\.md$/, '/').toLowerCase();
          node.url = `${prefix}${route}${rest.length ? `#${rest.join('#')}` : ''}` || './';
        }
      }
      node.children?.forEach(walk);
    };
    walk(tree);
  };
}
