import { resolve } from 'node:path';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig, type Plugin } from 'vite';

// The mocked core writes this string to the page, so any bundle that holds the mock holds it too.
const MOCK_MARKER = 'RESSWAP_MOCK_CORE';

// Mode `mock` reads VITE_MOCK_CORE=1 from .env.mock and builds into dist-mock. Every other build
// goes to dist, which the app bundles, so it fails when an output file carries the mocked core
// (REQ-UI-17).
function refuseMockInBuild(): Plugin {
  return {
    name: 'refuse-mock-in-build',
    apply: 'build',
    generateBundle(_options, bundle) {
      for (const [fileName, output] of Object.entries(bundle)) {
        const text =
          output.type === 'chunk'
            ? output.code
            : typeof output.source === 'string'
              ? output.source
              : '';
        if (text.includes(MOCK_MARKER)) {
          this.error(`${fileName} holds the mocked core; only a mock build may (REQ-UI-17).`);
        }
      }
    },
  };
}

export default defineConfig(({ mode }) => {
  const isMock = mode === 'mock';
  return {
    plugins: [react(), tailwindcss(), ...(isMock ? [] : [refuseMockInBuild()])],
    resolve: {
      alias: {
        '@': resolve(import.meta.dirname, './src'),
      },
    },
    clearScreen: false,
    build: {
      outDir: isMock ? 'dist-mock' : 'dist',
    },
    server: {
      port: 1420,
      strictPort: true,
      watch: {
        ignored: ['**/src-tauri/**'],
      },
    },
    // `vite preview --mode mock` serves dist-mock on the loopback address only, and fails when the
    // port is taken instead of moving to another one. e2e:mock passes its fixed port.
    preview: {
      host: '127.0.0.1',
      strictPort: true,
    },
  };
});
