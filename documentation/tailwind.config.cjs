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
            filter: "drop-shadow(0 20px 50px rgba(0,0,0,0.25))",
          },
          "50%": {
            transform: "translateY(-20px)",
            filter: "drop-shadow(0 40px 70px rgba(0,0,0,0.15))",
          },
        },
        flicker: {
          "0%, 100%": { filter: "brightness(100%)" },
          "10%": { filter: "brightness(120%)" },
          "20%": { filter: "brightness(100%)" },
          "30%": { filter: "brightness(110%)" },
          "40%": { filter: "brightness(100%)" },
          "50%": { filter: "brightness(115%)" },
          "60%": { filter: "brightness(95%)" },
          "70%": { filter: "brightness(105%)" },
          "80%": { filter: "brightness(105%)" },
          "90%": { filter: "brightness(125%)" },
        },
      },
      animation: {
        float: "float 9s cubic-bezier(0.445, 0.05, 0.55, 0.95) infinite",
        flicker: "flicker 20s ease-in-out infinite",
        flickerAndFloat: "float 7s cubic-bezier(0.445, 0.05, 0.55, 0.95) infinite, flicker 15s ease-in-out infinite",
      },
    },
  },
});

config.content = ["./node_modules/@risc0/ui/**/*.{ts,tsx}", "./site/**/*.{html,md,mdx,tsx,js,jsx}"];

module.exports = config;
