#!/usr/bin/env python

import sys
import os
from pathlib import Path
import subprocess

HEADER = """
// Copyright (c) {YEAR} RISC Zero, Inc.
//
// All rights reserved.
""".strip().splitlines()

EXTENSIONS = [
    ".cpp",
    ".h",
    ".rs",
    '.sol',
]

SKIP_PATHS = [
    # ImageID.sol is automatically generated.
    str(Path.cwd()) + "/contracts/src/SetBuilderImageID.sol",
    str(Path.cwd()) + "/contracts/src/AssessorImageID.sol",
    str(Path.cwd()) + "/contracts/src/UtilImageID.sol"
]


def check_header(expected_year, lines_actual):
    for expected, actual in zip(HEADER, lines_actual):
        expected = expected.replace("{YEAR}", expected_year)
        if expected != actual:
            return (expected, actual)
    return None


def check_file(root, file):
    cmd = ["git", "log", "-1", "--format=%ad", "--date=format:%Y", file]
    expected_year = subprocess.check_output(cmd, encoding="UTF-8").strip()
    rel_path = file.relative_to(root)
    lines = file.read_text().splitlines()
    result = check_header(expected_year, lines)
    if result:
        print(f"{rel_path}: invalid header!")
        print(f"  expected: {result[0]}")
        print(f"    actual: {result[1]}")
        return 1
    return 0


def repo_root():
    """Return an absolute Path to the repo root"""
    cmd = ["git", "rev-parse", "--show-toplevel"]
    return Path(subprocess.check_output(cmd, encoding="UTF-8").strip())


def tracked_files():
    """Yield all file paths tracked by git"""
    cmd = ["git", "ls-tree", "--full-tree", "--name-only", "-r", "HEAD"]
    tree = subprocess.check_output(cmd, encoding="UTF-8").strip()
    for path in tree.splitlines():
        yield (repo_root() / Path(path)).absolute()


def main():
    root = repo_root()
    ret = 0
    for path in tracked_files():
        if path.suffix in EXTENSIONS:
            skip = False
            for path_start in SKIP_PATHS:
                if str(path).startswith(path_start):
                    skip = True
                    break
            if skip:
                continue

            ret |= check_file(root, path)
    sys.exit(ret)


if __name__ == "__main__":
    main()
