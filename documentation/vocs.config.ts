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
		/*{ text: "Blog", link: "/blog" },*/
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
	ogImageUrl:
		"https://vocs.dev/api/og?logo=boundless-documentation.vercel.app/logo-dark.png&title=%title&description=%description",
});
