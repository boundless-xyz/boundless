"""Exercise the stock Bash entrypoint without an explicit block or network access."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

class PrepareShellTests(unittest.TestCase):
    def test_unset_block_is_resolved_before_forge(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bins = root / "bin"
            bins.mkdir()
            (bins / "cast").write_text("#!/bin/sh\nprintf '123\\n'\n")
            (bins / "forge").write_text("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$SUNSET_CAPTURE\"\nexit 42\n")
            for tool in bins.iterdir(): tool.chmod(0o755)
            env = os.environ.copy()
            env.pop("SUNSET_BLOCK_NUMBER", None)
            env.update(PATH=str(bins) + os.pathsep + env.get("PATH", ""), SUNSET_CAPTURE=str(root / "args"), SUNSET_REPORT_DIR=str(root / "report"))
            script = Path(__file__).with_name("prepare.sh")
            result = subprocess.run(["/bin/bash", str(script), *["base"]], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 42, result.stderr + result.stdout)
            argv = (root / "args").read_text().splitlines()
            self.assertEqual(argv[argv.index("--fork-block-number") + 1], "123")

if __name__ == "__main__": unittest.main()
