import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// Static SPA build. Output in dist/ is plain HTML/JS/CSS and can be hosted
// anywhere; it talks to the backend only through VITE_API_BASE_URL.
export default defineConfig({
  plugins: [react()],
});
