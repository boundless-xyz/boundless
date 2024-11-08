import tailwindConfig from "@risc0/ui/config/tailwind.config.base";
import deepmerge from "deepmerge";

const config = deepmerge(tailwindConfig, {
	theme: {
		extend: {
			fontFamily: {
				sans: ["var(--font-noto-sans)", "system-ui"],
				mono: ["var(--font-ubuntu-mono)"],
			},
		},
	},
});

config.content = [
	"./node_modules/@risc0/ui/**/*.{ts,tsx}",
	"./site/**/*.{html,md,mdx,tsx,js,jsx}",
];

export default config;
