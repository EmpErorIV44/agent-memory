import hashlib
import importlib.util
from pathlib import Path
import tarfile
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    "package_release", Path(__file__).resolve().parents[1] / "scripts/package-release.py"
)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleasePackageTests(unittest.TestCase):
    def test_archive_contents_modes_checksum_and_repeated_packaging(self):
        for target in sorted(release.TARGETS):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                binary = root / "target" / target / "release" / "agent-memory"
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"fixture executable")
                (root / "README.md").write_text("Readme")
                (root / "LICENSE").write_text("License")
                archive = release.package(root, target, "0.1.0-alpha.1", root / "dist")
                digest = hashlib.sha256(archive.read_bytes()).hexdigest()
                self.assertEqual(
                    archive.with_name(archive.name + ".sha256").read_text(),
                    f"{digest}  {archive.name}\n",
                )
                with tarfile.open(archive) as tar:
                    self.assertEqual(len(tar.getmembers()), 3)
                    entry = next(m for m in tar.getmembers() if m.name.endswith("/agent-memory"))
                    self.assertEqual(entry.mode, 0o755)
                    self.assertEqual(tar.extractfile(entry).read(), b"fixture executable")
                release.package(root, target, "0.1.0-alpha.1", root / "dist")
                self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), digest)

    def test_rejects_unsafe_version_unsupported_target_and_missing_binary(self):
        with tempfile.TemporaryDirectory() as temp:
            for target, version in [("x86_64-unknown-linux-gnu", "../bad"), ("unknown", "0.1.0"), ("x86_64-unknown-linux-gnu", "0.1.0")]:
                with self.subTest(target=target, version=version), self.assertRaises(ValueError):
                    release.package(temp, target, version, Path(temp) / "dist")


if __name__ == "__main__":
    unittest.main()
