import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";

const SOURCE_DIR = "site/pages/tmp";
const TARGET_DIR = "site/pages/smart-contracts";

function sanitizeFileName(name: string): string {
	const baseName = path.basename(name, ".md");
	const sanitizedBase = baseName.replace(/\./g, "-");
	return `${sanitizedBase}.md`;
}

function fixInternalLinks(content: string): string {
	return content.replace(
		/\[([^\]]+)\]\((\/[^)]+\/[^)]+\.sol\/[^)]+)\.md\)/g,
		(_, linkText, path) => {
			// Extract the final component of the path (e.g., "interface.IRiscZeroSetVerifier")
			const fileName = path.split("/").pop();

			// Sanitize it for our new structure
			const sanitizedName = fileName.replace(/\./g, "-");

			return `[${linkText}](/smart-contracts/${sanitizedName})`;
		},
	);
}

async function getAllMdFiles(dir: string): Promise<string[]> {
	const files: string[] = [];

	async function traverse(currentDir: string) {
		const entries = await readdir(currentDir, { withFileTypes: true });

		for (const entry of entries) {
			const fullPath = path.join(currentDir, entry.name);

			if (entry.isDirectory()) {
				await traverse(fullPath);
			} else if (
				entry.isFile() &&
				entry.name.endsWith(".md") &&
				!["SUMMARY.md", "README.md"].includes(entry.name)
			) {
				files.push(fullPath);
			}
		}
	}

	await traverse(dir);
	return files;
}

async function createIndex(files: string[]): Promise<string> {
	let indexContent = "# Smart Contracts Documentation\n\n";

	for (const file of files) {
		// Extract original name without the path or extension
		const originalName = path.basename(file, ".md");

		// Create sanitized filename for the link
		const sanitizedName = sanitizeFileName(originalName);

		// Create simplified link with sanitized name
		const link = `/smart-contracts/${sanitizedName}`;

		// Use original name for display, sanitized name for link
		indexContent += `- [${originalName}](${link})\n`;
	}

	return indexContent;
}

async function flattenFiles() {
	try {
		await mkdir(TARGET_DIR, { recursive: true });

		const files = await getAllMdFiles(SOURCE_DIR);

		for (const file of files) {
			let content = await readFile(file, "utf-8");

			// Fix internal links before saving
			content = fixInternalLinks(content);

			const originalName = path.basename(file);
			const newFileName = sanitizeFileName(originalName);
			await writeFile(path.join(TARGET_DIR, newFileName), content);
		}

		const indexContent = await createIndex(files);
		await writeFile(path.join(TARGET_DIR, "index.md"), indexContent);

		console.log("Documentation processing complete!");
	} catch (error) {
		console.error("Error processing documentation:", error);
	}
}

flattenFiles();
