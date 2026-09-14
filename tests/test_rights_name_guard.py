import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "tools" / "rights_name_guard.py"


class RightsNameGuardTests(unittest.TestCase):
    # Check the five supported web-text extensions with a synthetic deny term
    # and require failure without echoing the matched term in standard output.
    def test_detects_protected_term_in_web_manual(self):
        for suffix in (".html", ".htm", ".js", ".css", ".svg"):
            with self.subTest(suffix=suffix), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                denylist = root / "deny_terms.local.txt"
                denylist.write_text("PROTECTED_TITLE_ALPHA\n", encoding="utf-8")
                (root / ("manual" + suffix)).write_text("PROTECTED_TITLE_ALPHA", encoding="utf-8")
                result = self.run_guard(root, denylist)
                self.assertEqual(result.returncode, 1)
                self.assertNotIn("PROTECTED_TITLE_ALPHA", result.stdout)

    # Launch the current Python interpreter as a separate scan process and
    # capture output plus status without raising on the expected match failure.
    def run_guard(self, root: Path, denylist: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root), "--denylist", str(denylist)],
            text=True,
            capture_output=True,
            check=False,
        )

    # Place a synthetic denied term in Rust source and require exit status 1.
    def test_detects_protected_term_in_text(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            denylist = root / "deny_terms.local.txt"
            denylist.write_text("PROTECTED_TITLE_ALPHA\n", encoding="utf-8")
            (root / "source.rs").write_text("PROTECTED_TITLE_ALPHA\n", encoding="utf-8")
            self.assertEqual(self.run_guard(root, denylist).returncode, 1)

    # Keep publishable text clean while placing a denied term under target;
    # require a clean scan and exclusion of the denylist itself.
    def test_accepts_clean_text_and_skips_generated_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            denylist = root / "deny_terms.local.txt"
            denylist.write_text("PROTECTED_TITLE_ALPHA\n", encoding="utf-8")
            (root / "source.rs").write_text("generic board fixture\n", encoding="utf-8")
            generated = root / "target"
            generated.mkdir()
            (generated / "generated.txt").write_text("PROTECTED_TITLE_ALPHA\n", encoding="utf-8")
            self.assertEqual(self.run_guard(root, denylist).returncode, 0)


if __name__ == "__main__":
    unittest.main()
