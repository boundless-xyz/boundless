const tailwindConfig = require("@risc0/ui/config/tailwind.config.base");
const deepmerge = require("deepmerge");

const config = deepmerge(tailwindConfig, {
  theme: {
    extend: {
      fontFamily: {
        serif: ["var(--font-self-modern)", "system-ui"],
        sans: ["var(--font-replica)", "system-ui"],
        mono: ["var(--font-ubuntu-mono)", "monospace"],
      },
    },
  },
});

config.content = ["./node_modules/@risc0/ui/**/*.{ts,tsx}", "./site/**/*.{html,md,mdx,tsx,js,jsx}"];

module.exports = config;
