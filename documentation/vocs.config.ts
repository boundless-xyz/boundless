import { defineConfig } from "vocs";

export default defineConfig({
	font: {
		mono: {
			google: "Ubuntu Mono",
		},
	},
	sidebar: [
		{
			text: "Market",
			items: [
				{
					text: "Introduction",
					link: "/market/introduction",
				},
				{
					text: "Boundless Market RFC",
					link: "/market/boundless-market-rfc",
				},
				{
					text: "Market Matching Design",
					link: "/market/market-matching-design",
				},
				{
					text: "Local Development",
					link: "/market/local-development",
				},
				{
					text: "Public Deployments",
					link: "/market/public-deployments",
				},
			],
		},
		{
			text: "Requestor Manual",
			items: [
				{
					text: "Introduction",
					link: "/requestor-manual/introduction",
				},
				{
					text: "Broadcasting Requests",
					link: "/requestor-manual/broadcasting-requests",
				},
			],
		},
		{
			text: "Prover Manual",
			items: [
				{
					text: "Introduction",
					link: "/prover-manual/introduction",
				},
				{
					text: "Bento",
					items: [
						{
							text: "Introduction",
							link: "/prover-manual/bento/introduction",
						},
						{
							text: "Running",
							link: "/prover-manual/bento/running",
						},
						{
							text: "Performance Tuning",
							link: "/prover-manual/bento/performance-tuning",
						},
					],
				},
				{
					text: "Broker",
					items: [
						{
							text: "Introduction",
							link: "/prover-manual/broker/introduction",
						},
						{
							text: "Configuration",
							link: "/prover-manual/broker/configuration",
						},
						{
							text: "Operation",
							link: "/prover-manual/broker/operation",
						},
					],
				},
				{
					text: "Monitoring",
					link: "/prover-manual/monitoring",
				},
			],
		},
		{
			text: "Reference",
			link: "/reference",
		},
		{
			text: "Glossary",
			link: "/glossary",
		},
	],
	topNav: [
		{ text: "Indexer", link: "https://boundless-indexer-risczero.vercel.app/" },
		{
			text: "0.2.0",
			items: [
				{
					text: "Releases",
					link: "https://github.com/boundless-xyz/boundless/releases",
				},
			],
		},
	],
	socials: [
		{
			icon: "github",
			link: "https://github.com/boundless-xyz",
		},
		{
			icon: "x",
			link: "https://x.com/boundless_xyz",
		},
	],
	rootDir: "site",
	title: "Boundless Docs",
	logoUrl: {
		light: "/logo.png",
		dark: "/logo-dark.png",
	},
	theme: {
		accentColor: {
			light: "#537263", // Forest - primary accent
			dark: "#AED8C4", // Leaf - lighter accent for dark mode
		},
		variables: {
			/*color: {
				background: {
					light: "#EFECE3", // Sand
					dark: "#000000", // Black
				},
				text: {
					light: "#000000", // Black
					dark: "#EFECE3", // Sand
				},
				text2: {
					light: "#537263", // Forest
					dark: "#AED8C4", // Leaf
				},
				text3: {
					light: "#D6D3C7", // Tan
					dark: "#537263", // Forest
				},
				backgroundAccent: {
					light: "#537263", // Forest
					dark: "#AED8C4", // Leaf
				},
				backgroundAccentHover: {
					light: "#445e52", // Darker Forest
					dark: "#9bc4b0", // Darker Leaf
				},
				border: {
					light: "#D6D3C7", // Tan
					dark: "#537263", // Forest
				},
				codeBlockBackground: {
					light: "#FFFFFF", // White
					dark: "#1a1a1a", // Dark gray
				},
				codeInlineBackground: {
					light: "#F2E2E0", // Sunrise
					dark: "#D1C6EA", // Lilac
				},
				link: {
					light: "#537263", // Forest
					dark: "#AED8C4", // Leaf
				},
				linkHover: {
					light: "#445e52", // Darker Forest
					dark: "#9bc4b0", // Darker Leaf
				},
				noteBackground: {
					light: "#F2E2E0", // Sunrise
					dark: "#D1C6EA", // Lilac
				},
				noteBorder: {
					light: "#537263", // Forest
					dark: "#AED8C4", // Leaf
				},
			},*/
			/*fontFamily: {
				default: "system-ui, sans-serif",
				mono: "ui-monospace, monospace",
			},*/
			content: {
				width: "calc(90ch + (var(--vocs-content_horizontalPadding) * 2))",
			},
		},
	},
	iconUrl: {
		light: "/favicon.png",
		dark: "/favicon-dark.png",
	},
	// banner: "Head to our new [Discord](https://discord.gg/JUrRkGweXV)!",
	editLink: {
		pattern:
			"https://github.com/boundless-xyz/boundless/edit/main/documentation/site/pages/:path",
		text: "Edit on GitHub",
	},
	ogImageUrl:
		"https://vocs.dev/api/og?logo=https://boundless-documentation.vercel.app/logo-dark.png&title=%title&description=%description",
});
