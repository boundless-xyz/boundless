import { execSync } from "node:child_process";
import { defineConfig } from "vocs";

function getLatestTag(): string {
	if (process.env.VERCEL) {
		console.log("Running on Vercel");
		const ref = process.env.VERCEL_GIT_COMMIT_REF || "";
		console.log("VERCEL_GIT_COMMIT_REF:", ref);

		if (ref.startsWith("refs/tags/")) {
			const version = ref.replace("refs/tags/", "").replace("v", "");
			console.log("Found tag reference, using version:", version);
			return version;
		}

		const version = ref.replace("v", "") || "latest";
		console.log("Using ref directly as version:", version);
		return version;
	}

	try {
		console.log("Running in local environment");
		const version = execSync("git describe --tags --abbrev=0")
			.toString()
			.trim()
			.replace("v", "");
		console.log("Found local git tag:", version);
		return version;
	} catch (error) {
		console.warn(
			"Failed to fetch git tag, falling back to default version",
			error,
		);
		return "latest";
	}
}

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
			text: getLatestTag(),
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
			light: "#474444", // Forest - primary accent
			dark: "#AED8C4", // Leaf - lighter accent for dark mode
		},
		variables: {
			color: {
				backgroundDark: {
					light: "#EFECE3", // Sand
					dark: "#1e1d1f",
				},
				background: {
					light: "#FFFFFF",
					dark: "#232225",
				},
			},
			content: {
				width: "calc(90ch + (var(--vocs-content_horizontalPadding) * 2))",
			},
		},
	},
	iconUrl: {
		light: "/favicon.png",
		dark: "/favicon-dark.png",
	},
	// banner: "Read the [Boundless Blog Article](https://risczero.com/blog/boundless-the-verifiable-compute-layer)",
	editLink: {
		pattern:
			"https://github.com/boundless-xyz/boundless/edit/main/documentation/site/pages/:path",
		text: "Edit on GitHub",
	},
	ogImageUrl:
		"https://vocs.dev/api/og?logo=https://boundless-documentation.vercel.app/logo-dark.png&title=%title&description=%description",
});
