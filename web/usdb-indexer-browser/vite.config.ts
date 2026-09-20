import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const controlPlaneTarget = process.env.USDB_CONTROL_PLANE_TARGET ?? 'http://127.0.0.1:28140'

export default defineConfig({
  base: './',
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 5176,
    proxy: {
      '/api': {
        target: controlPlaneTarget,
        changeOrigin: false,
      },
    },
  },
  preview: {
    host: '127.0.0.1',
    port: 4176,
  },
})
