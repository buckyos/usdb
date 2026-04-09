import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const controlPlaneTarget = process.env.USDB_CONTROL_PLANE_TARGET ?? 'http://127.0.0.1:28140'

export default defineConfig({
  plugins: [react()],
  server: {
    host: '0.0.0.0',
    port: 5174,
    proxy: {
      '/api': {
        target: controlPlaneTarget,
        changeOrigin: true,
      },
      '/explorers': {
        target: controlPlaneTarget,
        changeOrigin: true,
      },
    },
  },
  preview: {
    host: '0.0.0.0',
    port: 4174,
  },
})

