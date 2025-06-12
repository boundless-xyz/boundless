#!/usr/bin/env bun
import { exec } from "node:child_process";
import { readdir, rename, unlink } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";

const execAsync = promisify(exec);

const VOCS_DIR = "site/dist/.vocs";
const SEARCH_INDEX_PREFIX = "search-index-";
const SEARCH_INDEX_SUFFIX = ".json";

async function findSearchIndexFiles(directory: string): Promise<string[]> {
  try {
    const files = await readdir(directory);
    return files.filter(file =>
      file.startsWith(SEARCH_INDEX_PREFIX) &&
      file.endsWith(SEARCH_INDEX_SUFFIX)
    );
  } catch (error) {
    console.warn(`Could not read directory ${directory}:`, error);
    return [];
  }
}

async function main() {
  console.info("🔍 Finding existing search index files…");

  // Find existing search index files
  const existingFiles = await findSearchIndexFiles(VOCS_DIR);
  console.info(`Found ${existingFiles.length} existing search index files:`, existingFiles);

  // Store the old filename (if exists) to reuse its hash
  const oldFileName = existingFiles.length > 0 ? existingFiles[0] : null;

  console.info("🔄 Running vocs reindex command…");

  // Run the reindex command
  try {
    await execAsync("bun ./node_modules/vocs/cli/index.ts search-index");
    console.info("✅ Reindex completed successfully");
  } catch (error) {
    console.error("❌ Reindex failed:", error);
    process.exit(1);
  }

  console.info("📁 Managing search index files…");

  // Find all search index files after reindex
  const allFiles = await findSearchIndexFiles(VOCS_DIR);

  if (allFiles.length === 0) {
    console.error("❌ No search index files found after reindex");
    process.exit(1);
  }

  // If we had an old file and now have multiple files
  if (oldFileName && allFiles.length > 1) {
    // Find the new file (the one that's not the old file)
    const newFiles = allFiles.filter(file => file !== oldFileName);

    if (newFiles.length === 1) {
      const newFileName = newFiles[0];
      const oldFilePath = join(VOCS_DIR, oldFileName);
      const newFilePath = join(VOCS_DIR, newFileName!);
      const targetFilePath = join(VOCS_DIR, oldFileName);

      console.info(`🗑️  Deleting old file: ${oldFileName}`);
      await unlink(oldFilePath);

      console.info(`📝 Renaming ${newFileName} → ${oldFileName}`);
      await rename(newFilePath, targetFilePath);

      console.info("✅ Search index files updated successfully");
    } else {
      console.info("ℹ️  Multiple new files found, keeping all");
    }
  } else if (oldFileName && allFiles.length === 1) {
    // Only one file exists and it's the same as before
    console.info("ℹ️  Search index file unchanged");
  } else {
    // No old file existed, or complex scenario
    console.info(`ℹ️  Found ${allFiles.length} search index files after reindex`);
  }

  console.info("🎉 Reindex with cleanup completed!");
}

main().catch((error) => {
  console.error("❌ Script failed:", error);
  process.exit(1);
});
