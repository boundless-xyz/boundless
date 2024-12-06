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
      keyframes: {
        float: {
          "0%, 100%": {
            transform: "translateY(0)",
          },
          "50%": {
            transform: "translateY(-20px)",
          },
        },
        flicker: {
          "0%, 100%": { filter: "brightness(1) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "10%": { filter: "brightness(1.05) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "20%": { filter: "brightness(1) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "30%": { filter: "brightness(1.1) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "40%": { filter: "brightness(1) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "50%": { filter: "brightness(1.15) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "60%": { filter: "brightness(0.95) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "70%": { filter: "brightness(1.05) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "80%": { filter: "brightness(1.05) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
          "90%": { filter: "brightness(1.2) drop-shadow(0 20px 50px rgba(0,0,0,0.25))" },
        },
      },
      animation: {
        flickerAndFloat: "flicker 15s ease-in-out infinite, float 7s cubic-bezier(0.445, 0.05, 0.55, 0.95) infinite",
      },
    },
  },
});

config.content = ["./node_modules/@risc0/ui/**/*.{ts,tsx}", "./site/**/*.{html,md,mdx,tsx,js,jsx}"];

module.exports = config;
