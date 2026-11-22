"""Offline regression tests for fail-closed sidecar inventory auditing."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("audit_rclone", Path(__file__).with_name("audit-rclone.py"))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class InventoryTests(unittest.TestCase):
    def fixture(self, version="(devel)"):
        def string(value):
            data = value.encode()
            self.assertLess(len(data), 128)
            return bytes([len(data)]) + data

        header = b"\xff Go buildinf:" + bytes([8, 2]) + bytes(16)
        raw = "x" * 16 + f"mod\tgithub.com/rclone/rclone\t{version}\t\n" + "x" * 16
        data = header + string("go1.27.4") + string(raw)
        manifest = {"binarySha256": {"arm64": hashlib.sha256(data).hexdigest()},
                    "sourceVersion": "v1.76.0", "goVersion": "go1.27.4"}
        return data, manifest

    def check(self, data, manifest):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "sidecar"
            binary.write_bytes(data)
            return audit.inventory(binary, manifest)

    def test_verified_devel_uses_pinned_source(self):
        _, modules = self.check(*self.fixture())
        self.assertEqual(modules, [{"name": "github.com/rclone/rclone", "version": "v1.76.0"},
                                   {"name": "stdlib", "version": "1.27.4"}])

    def test_tampered_binary_rejected(self):
        data, manifest = self.fixture()
        with self.assertRaisesRegex(ValueError, "SHA256"):
            self.check(data + b"tampered", manifest)

    def test_unrecognized_main_version_rejected(self):
        with self.assertRaisesRegex(ValueError, "disagrees"):
            self.check(*self.fixture("unknown"))

    def test_toolchain_mismatch_rejected(self):
        data, manifest = self.fixture()
        manifest["goVersion"] = "go1.27.3"
        with self.assertRaisesRegex(ValueError, "disagrees"):
            self.check(data, manifest)


if __name__ == "__main__":
    unittest.main()
