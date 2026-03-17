import { defineConfig } from 'astro/config';
import tailwind from '@astrojs/tailwind';

export default defineConfig({
  site: 'https://prism-ide.dev',
  output: 'static',
  integrations: [tailwind()],
});
