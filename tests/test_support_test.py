"""Output isolation checks; run with python3 tests/test_support_test.py."""

from pathlib import Path
import tempfile
import unittest

from test_support import new_run_directory, package_version


class RunDirectoryTests(unittest.TestCase):
    def test_existing_files_and_previous_runs_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            parent = Path(temporary)
            sentinel = parent / "important.txt"
            sentinel.write_bytes(b"user data")
            first = Path(new_run_directory(parent))
            (first / "result.txt").write_bytes(b"prior run")
            second = Path(new_run_directory(parent))
            self.assertNotEqual(first, second)
            self.assertEqual(first.parent, parent)
            self.assertEqual(sentinel.read_bytes(), b"user data")
            self.assertEqual((first / "result.txt").read_bytes(), b"prior run")
            self.assertEqual(list(second.iterdir()), [])

    def test_symlink_and_file_parents_are_refused_without_modification(self):
        with tempfile.TemporaryDirectory() as temporary:
            parent = Path(temporary)
            target = parent / "target"
            target.mkdir()
            link = parent / "link"
            link.symlink_to(target, target_is_directory=True)
            with self.assertRaises(ValueError):
                new_run_directory(link)
            self.assertEqual(list(target.iterdir()), [])
            file = parent / "file"
            file.write_bytes(b"keep")
            with self.assertRaises(FileExistsError):
                new_run_directory(file)
            self.assertEqual(file.read_bytes(), b"keep")

    def test_manifest_version_is_read(self):
        self.assertRegex(package_version(), r"^\d+\.\d+\.\d+")


if __name__ == "__main__":
    unittest.main()
