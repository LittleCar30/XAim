import os
import tempfile
import unittest
from pathlib import Path
from paths import safe_path
class PathsTest(unittest.TestCase):
    def test_containment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "root"
            root.mkdir()
            self.assertEqual(safe_path(root, "a/../b.txt"), (root / "b.txt").resolve())
            for user_path in ("../secret.txt", "../root-other/a", str(Path(directory) / "outside")):
                with self.assertRaises(ValueError): safe_path(root, user_path)
            try:
                os.symlink(directory, root / "escape", target_is_directory=True)
            except OSError:
                return
            with self.assertRaises(ValueError): safe_path(root, "escape/outside")
