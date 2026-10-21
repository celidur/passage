import {defineConfig} from 'vite';

// `npm run dev` serves the dashboard with hot reload and forwards API calls to a local `passage serve`.
export default defineConfig({
  server: {
    proxy: {'/api': 'http://127.0.0.1:8080'},
  },
});
