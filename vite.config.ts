import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
export default defineConfig({ plugins: [react()], base: './', build: { outDir: 'dist/ui', rollupOptions: { input: { desktop: 'index.html', phone: 'phone.html' } } } });
