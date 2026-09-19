import react, { reactCompilerPreset } from '@vitejs/plugin-react'
import babel from '@rolldown/plugin-babel'
import { defineConfig } from 'vite'

const backendTarget = process.env.TRACKER_BACKEND_URL ?? 'http://127.0.0.1:9123'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
    babel({ presets: [reactCompilerPreset()] })
  ],
  server: {
    proxy: {
      '/health': backendTarget,
      '/api': backendTarget,
    },
  },
})
