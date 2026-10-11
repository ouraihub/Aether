import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

// Tauri loads ../ui/dist (tauri.conf.json frontendDist); run `pnpm -F example-assistant-ui build` first.
export default defineConfig({ plugins: [tailwindcss(), svelte()], build: { outDir: 'dist', emptyOutDir: true } })
