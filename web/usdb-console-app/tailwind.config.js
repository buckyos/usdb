/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      fontFamily: {
        sans: ['Plus Jakarta Sans', 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'sans-serif'],
        display: ['Sora', 'Plus Jakarta Sans', 'sans-serif'],
      },
      boxShadow: {
        panel: 'var(--cp-panel-shadow)',
        window: 'var(--cp-window-shadow)',
      },
    },
  },
  plugins: [],
}

