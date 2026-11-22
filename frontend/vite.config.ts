import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:4317',
      '/ws': {
        target: 'ws://127.0.0.1:4317',
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: false,
    target: 'es2022',
    rollupOptions: {
      output: {
        manualChunks(id) {
          const norm = id.replace(/\\/g, '/');
          if (!norm.includes('node_modules')) return;
          if (
            norm.includes('/react/') ||
            norm.includes('/react-dom/') ||
            norm.includes('/scheduler/')
          ) {
            return 'vendor-react';
          }
          if (norm.includes('/@xterm/')) {
            return 'vendor-xterm';
          }
          if (
            norm.includes('/@codemirror/lang-') ||
            (norm.includes('/@lezer/') &&
              !norm.includes('/@lezer/common/') &&
              !norm.includes('/@lezer/highlight/') &&
              !norm.includes('/@lezer/lr/'))
          ) {
            return 'vendor-langs';
          }
          if (
            norm.includes('/@codemirror/') ||
            norm.includes('/@lezer/') ||
            norm.includes('/style-mod/') ||
            norm.includes('/w3c-keyname/') ||
            norm.includes('/crelt/')
          ) {
            return 'vendor-codemirror';
          }
          if (norm.includes('/lucide-react/')) {
            return 'vendor-icons';
          }
          if (norm.includes('/diff/')) {
            return 'vendor-diff';
          }
        },
      },
    },
  },
});
