import fs from "node:fs";
import path from "node:path";

type PreviewData = {
  title: string;
  description: string;
  image: string;
  url: string;
};

type PreviewCache = {
  [url: string]: PreviewData;
};

const FILES_TO_IGNORE = ["_kitchen-sink.mdx"];

async function fetchPreview(url: string): Promise<PreviewData> {
  const response = await fetch(
    `https://iframe.ly/api/iframely?url=${encodeURIComponent(url)}&api_key=6b4fc5fa21de39fb366195`,
  );

  if (!response.ok) {
    throw new Error("Failed to fetch preview");
  }

  const data = await response.json();
  return {
    title: data.meta.title,
    description: data.meta.description,
    image: data.links.thumbnail?.[0]?.href || "",
    url: url,
  };
}

async function generatePreviewCache() {
  const cache: PreviewCache = {};
  const mdxDir = path.join(process.cwd(), "site/pages");

  function findMDXFiles(dir: string): string[] {
    const files: string[] = [];
    const entries = fs.readdirSync(dir, { withFileTypes: true });

    for (const entry of entries) {
      const fullPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        files.push(...findMDXFiles(fullPath));
      } else if (entry.name.endsWith(".mdx") && !FILES_TO_IGNORE.includes(entry.name)) {
        files.push(fullPath);
      }
    }

    return files;
  }

  // Extract URLs from LinkPreview components
  const linkPreviewRegex = /<LinkPreview[^>]*url=["']([^"']+)["'][^>]*>/g;
  const mdxFiles = findMDXFiles(mdxDir);

  for (const file of mdxFiles) {
    const content = fs.readFileSync(file, "utf-8");
    const matches = [...content.matchAll(linkPreviewRegex)];

    for (const match of matches) {
      const url = match[1];

      if (url && !cache[url]) {
        try {
          cache[url] = await fetchPreview(url);
        } catch (error) {
          console.error(`Failed to fetch preview for ${url}:`, error);
        }
      }
    }
  }

  // Save cache to JSON file
  fs.writeFileSync(path.join(process.cwd(), "site/public/link-previews.json"), JSON.stringify(cache, null, 2));
}

async function main() {
  await generatePreviewCache();
}

main().catch(console.error);
