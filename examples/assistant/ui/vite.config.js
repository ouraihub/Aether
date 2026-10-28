import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

// Tauri loads ../ui/dist (tauri.conf.json frontendDist); run `pnpm -F example-assistant-ui build` first.
export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    proxy: Object.fromEntries(
      ['/api', '/mcp', '/oauth', '/.well-known', '/health'].map((path) => [
        path,
        {
          target: 'http://127.0.0.1:8787',
          changeOrigin: true,
        },
      ]),
    ),
  },
  build: { outDir: 'dist', emptyOutDir: true },
})
