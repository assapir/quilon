import { defineRouteMiddleware } from '@astrojs/starlight/route-data';

// Entries are read through the src/content/docs symlink, so Starlight builds
// edit URLs with the symlink's path in them; the real files live in docs/.
export const onRequest = defineRouteMiddleware((context) => {
  const { editUrl } = context.locals.starlightRoute;
  if (editUrl) {
    context.locals.starlightRoute.editUrl = new URL(
      editUrl.href.replace('/edit/main/docs/src/content/docs/', '/edit/main/docs/')
    );
  }
});
