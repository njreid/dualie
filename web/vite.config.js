import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import path from 'path';

export default defineConfig({
  plugins: [svelte()],

  // Production build goes straight into the Rust embed path
  build: {
    outDir: path.resolve('..', 'daemon', 'src', 'web', 'static'),
    emptyOutDir: true,
  },

  server: {
    port: 5173,
    // Proxy API calls to the running daemon in dev mode
    proxy: {
      '/api': {
        target: 'http://localhost:7474',
        changeOrigin: true,
      },
    },
  },

  test: {
    environment: 'jsdom',
  },
});
