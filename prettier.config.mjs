/** @type {import('prettier').Config} */
const config = {
  printWidth: 100,
  singleQuote: true,
  plugins: ['prettier-plugin-tailwindcss'],
  tailwindStylesheet: './src/styles/globals.css',
  tailwindFunctions: ['cn', 'cva'],
};

export default config;
