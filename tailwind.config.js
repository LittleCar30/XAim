const colors = require('tailwindcss/colors')

// Keep existing component utilities while sharing the light/dark UI palette.
const themedColor = (name, shade) => `rgb(var(--${name}-${shade}) / <alpha-value>)`
const accents = ['sky', 'emerald', 'rose', 'amber', 'cyan', 'fuchsia', 'orange', 'pink', 'violet', 'yellow']

/** @type {import('tailwindcss').Config} */
module.exports = {
  darkMode: 'class',
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      borderRadius: { md: '0.75rem', lg: '1rem', xl: '1.25rem', '2xl': '1.5rem' },
      colors: {
        slate: Object.fromEntries(Object.keys(colors.slate).map((shade) => [shade, themedColor('slate', shade)])),
        ...Object.fromEntries(accents.map((name) => [name, {
          200: themedColor(name, 200),
          300: themedColor(name, 300),
          400: themedColor(name, 400),
          ...(['sky', 'emerald'].includes(name) ? { 500: themedColor(name, 500) } : {}),
        }])),
      },
    },
  },
  plugins: [],
}
