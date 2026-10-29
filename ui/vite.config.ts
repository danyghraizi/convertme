import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';

const BUILD_ID = new Date().toISOString().slice(0, 16).replace('T', ' ');

export default defineConfig({
  plugins: [solid()],
  define: {
    // Stamped into the bundle: a window showing an older id than ui-dist is running a
    // stale process, which is otherwise indistinguishable from a design that was ignored.
    __BUILD_ID__: JSON.stringify(BUILD_ID),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: 'es2021',
    outDir: '../crates/convertme-gui/ui-dist',
    emptyOutDir: true,
    sourcemap: false,
  },
});
