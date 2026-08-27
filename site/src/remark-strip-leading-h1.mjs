/**
 * Remove a page's first depth-1 heading. The markdown under docs/ keeps its H1
 * so GitHub renders each page with a title; on the site, Starlight already
 * shows the frontmatter title as the heading, and without this the page would
 * carry it twice.
 */
export function stripLeadingH1() {
  return (tree) => {
    const index = tree.children.findIndex(
      (node) => node.type === 'heading' && node.depth === 1
    );
    if (index !== -1) {
      tree.children.splice(index, 1);
    }
  };
}
