import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";

const SOURCE_DIR = "site/pages/tmp";
const TARGET_DIR = "site/pages/smart-contracts";

function sanitizeFileName(name: string) {
  const baseName = path.basename(name, ".md");
  const sanitizedBase = baseName.replace(/\./g, "-");
  return `${sanitizedBase}.md`;
}

function fixInternalLinks(content: string) {
  return content.replace(/\[([^\]]+)\]\((\/[^)]+\/[^)]+\.sol\/[^)]+)\.md\)/g, (_, linkText, path) => {
    // Extract the final component of the path (e.g., "interface.IRiscZeroSetVerifier")
    const fileName = path.split("/").pop();

    // Sanitize it for our new structure
    const sanitizedName = fileName.replace(/\./g, "-");

    return `[${linkText}](/smart-contracts/${sanitizedName})`;
  });
}

async function getAllMdFiles(dir: string) {
  const files: string[] = [];

  async function traverse(currentDir: string) {
    const entries = await readdir(currentDir, { withFileTypes: true });

    for (const entry of entries) {
      const fullPath = path.join(currentDir, entry.name);

      if (entry.isDirectory()) {
        await traverse(fullPath);
      } else if (entry.isFile() && entry.name.endsWith(".md") && !["SUMMARY.md", "README.md"].includes(entry.name)) {
        files.push(fullPath);
      }
    }
  }

  await traverse(dir);
  return files;
}

function createIndex(files: string[]) {
  let indexContent = `# Smart Contracts Documentation

Welcome to the smart contracts documentation for Boundless. This section contains detailed documentation for all our smart contracts, including [interfaces](#interfaces), [libraries](#libraries), and [core contracts](#core-contracts).

:::note

Our smart contracts are built using Solidity and are organized into several key components:

- **Interfaces**: Contract interfaces that define the external API
- **Libraries**: Reusable code libraries
- **Core Contracts**: Main contracts that implement the core business logic

:::

`;

  // Group files by type (interface, library, contract)
  const interfaces: string[] = [];
  const libraries: string[] = [];
  const contracts: string[] = [];

  for (const file of files) {
    const originalName = path.basename(file, ".md");
    const sanitizedName = sanitizeFileName(originalName).replace(".md", "");
    const link = `/smart-contracts/${sanitizedName}`;
    const entry = `- [${originalName}](${link})`;

    if (originalName.startsWith("interface.") || originalName.startsWith("I")) {
      interfaces.push(entry);
    } else if (originalName.startsWith("library.")) {
      libraries.push(entry);
    } else {
      contracts.push(entry);
    }
  }

  if (interfaces.length > 0) {
    indexContent += "\n## Interfaces\n\n";
    indexContent += interfaces.sort().join("\n");
  }

  if (libraries.length > 0) {
    indexContent += "\n\n## Libraries\n\n";
    indexContent += libraries.sort().join("\n");
  }

  if (contracts.length > 0) {
    indexContent += "\n\n## Core Contracts\n\n";
    indexContent += contracts.sort().join("\n");
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
  } catch (error) {
    console.error("Error processing documentation:", error);
  }
}

flattenFiles();
