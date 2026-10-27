import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

// During `npm run dev`, API calls go to a running `swarmdash serve --port 7070`.
export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: { '/api': 'http://127.0.0.1:7070' },
  },
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 600,
  },
})
