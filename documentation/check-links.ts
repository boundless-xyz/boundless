import { readFile } from "node:fs/promises";
import { glob } from "glob";

// Add ignore list configuration
const IGNORED_URL_PREFIXES = new Set([
	"https://github.com/boundless-xyz",
	"https://sepolia.etherscan.io",
]);

async function checkRemoteUrl(url: string): Promise<boolean> {
	// Check if URL starts with any of the ignored prefixes
	if ([...IGNORED_URL_PREFIXES].some((prefix) => url.startsWith(prefix))) {
		return true;
	}

	try {
		const response = await fetch(url);

		return response.status >= 200 && response.status < 300;
	} catch {
		return false;
	}
}

async function findAnchorsInFile(filePath: string): Promise<Set<string>> {
	const content = await readFile(filePath, "utf8");
	const anchors = new Set<string>();

	// Match ATX-style headers (# Header)
	const headerRegex = /^#{1,6}\s+(.+)$/gm;
	// biome-ignore lint/suspicious/noExplicitAny: ignore
	let match: any;

	// biome-ignore lint/suspicious/noAssignInExpressions: ignore
	while ((match = headerRegex.exec(content)) !== null) {
		const headerText = match[1].trim();
		// Convert header to GitHub-style anchor
		const anchor = headerText
			.toLowerCase()
			.replace(/[^\w\- ]/g, "") // Remove special chars
			.replace(/\s+/g, "-"); // Replace spaces with hyphens
		anchors.add(anchor);
	}

	return anchors;
}

async function localPathExists(linkPath: string): Promise<boolean> {
	try {
		// Skip checking image files
		if (linkPath.match(/\.(png|jpg|jpeg|gif|svg|webp)$/i)) {
			return true;
		}

		const files = await glob("site/pages/**/*");

		// For remote links, actually check the URL
		if (linkPath.startsWith("http")) {
			return await checkRemoteUrl(linkPath);
		}

		// Split path and anchor
		const [pathPart = "", anchor] = linkPath.split("#");

		// Remove leading slash, .md extension, and trailing slash from the path part
		const normalizedPath = pathPart.replace(/^\//, "").replace(/\.md$/, "");

		const possiblePaths = [
			`site/pages/${normalizedPath}.md`,
			`site/pages/${normalizedPath}.mdx`,
			`site/pages/${normalizedPath}/index.md`,
			`site/pages/${normalizedPath}/index.mdx`,
		];

		// Find the actual file path if it exists
		const existingPath = possiblePaths.find((p) => files.includes(p));

		if (!existingPath) {
			return false;
		}

		// If there's no anchor, we're done
		if (!anchor) {
			return true;
		}

		// If there is an anchor, verify it exists in the file
		const anchors = await findAnchorsInFile(existingPath);
		return anchors.has(anchor);
	} catch (error) {
		console.error("Error checking path:", error);
		return false;
	}
}

async function checkLinks() {
	const files = await glob("**/*.md", { ignore: ["node_modules/**"] });
	let hasErrors = false;

	for (const file of files) {
		const markdown = await readFile(file, "utf8");
		const fileErrors: string[] = [];

		// Check reference-style links
		const refLinkRegex = /^\[([^\]]+)\]:\s*(\S+)/gm;
		// biome-ignore lint/suspicious/noExplicitAny: ignore
		let match: any;

		// biome-ignore lint/suspicious/noAssignInExpressions: ignore
		while ((match = refLinkRegex.exec(markdown)) !== null) {
			const [, label, url] = match;

			if (url.startsWith("http")) {
				const isValid = await checkRemoteUrl(url);
				if (!isValid) {
					fileErrors.push(
						`Reference link [${label}] is not accessible: ${url}`,
					);
				}
			} else if (url.startsWith("/")) {
				const exists = await localPathExists(url);
				if (!exists) {
					fileErrors.push(
						`Reference link [${label}] points to non-existent path or anchor: ${url}`,
					);
				}
			}
		}

		// Check inline links
		const inlineLinkRegex = /\[([^\]]+)\]\(([^)]+)\)/g;

		// biome-ignore lint/suspicious/noAssignInExpressions: ignore
		while ((match = inlineLinkRegex.exec(markdown)) !== null) {
			const [, label, url] = match;

			if (url.startsWith("http")) {
				const isValid = await checkRemoteUrl(url);
				if (!isValid) {
					fileErrors.push(`Inline link [${label}] is not accessible: ${url}`);
				}
			} else if (url.startsWith("/")) {
				const exists = await localPathExists(url);
				if (!exists) {
					fileErrors.push(
						`Inline link [${label}] points to non-existent path or anchor: ${url}`,
					);
				}
			}
		}

		if (fileErrors.length > 0) {
			hasErrors = true;
			console.error(`\n❌ ${file}:`);
			for (const error of fileErrors) {
				console.error(`  - ${error}`);
			}
		} else {
		}
	}

	if (hasErrors) {
		console.error("\n❌ Some files contain invalid links");
		process.exit(1);
	} else {
	}
}

checkLinks();
