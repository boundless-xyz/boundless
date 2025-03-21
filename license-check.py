#!/usr/bin/env python

import sys
import os
from pathlib import Path
import subprocess

BSL_HEADER = """
// Copyright (c) 2025 RISC Zero Inc,
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
""".strip().splitlines()

APACHE_HEADER = """
// Copyright {YEAR} RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
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
    str(Path.cwd()) + "/contracts/src/libraries/AssessorImageID.sol",
    str(Path.cwd()) + "/contracts/src/libraries/UtilImageID.sol",
    str(Path.cwd()) + "/crates/boundless-market/src/contracts/artifacts",
    str(Path.cwd()) + "/crates/boundless-market/src/contracts/bytecode.rs",
]

APACHE_PATHS = [
    str(Path.cwd()) + "/contracts/src/HitPoints.sol",
    str(Path.cwd()) + "/contracts/src/IBoundlessMarket.sol",
    str(Path.cwd()) + "/contracts/src/IHitPoints.sol",
    str(Path.cwd()) + "/crates/assessor",
    str(Path.cwd()) + "/crates/balance-alerts-layer",
    str(Path.cwd()) + "/crates/boundless-cli",
    str(Path.cwd()) + "/crates/boundless-market",
    str(Path.cwd()) + "/crates/slasher",
    str(Path.cwd()) + "/documentation",
    str(Path.cwd()) + "/examples",
]

def check_header(file, expected_year, lines_actual):
    if any(map(lambda path: file.is_relative_to(path), APACHE_PATHS)):
        header = APACHE_HEADER
    else:
        header = BSL_HEADER

    for expected, actual in zip(header, lines_actual):
        expected = expected.replace("{YEAR}", expected_year)
        if expected != actual:
            return (expected, actual)
    return None


def check_file(root, file):
    cmd = ["git", "log", "-1", "--format=%ad", "--date=format:%Y", file]
    expected_year = subprocess.check_output(cmd, encoding="UTF-8").strip()
    rel_path = file.relative_to(root)
    lines = file.read_text().splitlines()
    result = check_header(file, expected_year, lines)
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
