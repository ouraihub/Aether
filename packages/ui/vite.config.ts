import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { svelteTesting } from '@testing-library/svelte/vite'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [tailwindcss(), svelte(), svelteTesting()],
  test: { environment: 'jsdom', include: ['src/**/*.test.ts'], setupFiles: ['./vitest.setup.ts'] },
})
