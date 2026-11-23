import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "scripts"))
import package_release  # noqa: E402


class TestCreateMacosDmgSigning(unittest.TestCase):
    def _run_dmg_with_fake_codesign(self, sign_identity: str) -> list[str]:
        with tempfile.TemporaryDirectory(prefix="gpui_dmg_sign_test_") as tmp:
            tmp_path = Path(tmp)
            app_dir = tmp_path / "Fabushi.app"
            macos_dir = app_dir / "Contents" / "MacOS"
            macos_dir.mkdir(parents=True)
            (macos_dir / "gpui_telegram_desktop").write_text("#!/bin/sh\nexit 0\n")

            dist_dir = tmp_path / "dist"
            dist_dir.mkdir()
            log_file = tmp_path / "codesign_calls.log"
            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()

            codesign_stub = bin_dir / "codesign"
            codesign_stub.write_text(
                f"#!/bin/sh\necho \"$@\" >> {str(log_file)!r}\nexit 0\n"
            )
            codesign_stub.chmod(codesign_stub.stat().st_mode | stat.S_IEXEC)

            hdiutil_stub = bin_dir / "hdiutil"
            hdiutil_stub.write_text(
                "#!/bin/sh\n"
                "eval last=\\${$#}\n"
                "echo dummy > \"$last\"\n"
                "exit 0\n"
            )
            hdiutil_stub.chmod(hdiutil_stub.stat().st_mode | stat.S_IEXEC)

            old_path = os.environ.get("PATH", "")
            os.environ["PATH"] = f"{bin_dir}:{old_path}"
            try:
                dmg_path = package_release.create_macos_dmg(
                    dist_dir=dist_dir,
                    app_dir=app_dir,
                    app_name="Fabushi",
                    version="1.6.8",
                    arch="arm64",
                    sign_identity=sign_identity,
                    notarize=False,
                )
                self.assertIsNotNone(dmg_path)
                self.assertTrue(dmg_path.exists())
            finally:
                os.environ["PATH"] = old_path

            return log_file.read_text().strip().splitlines()

    def test_self_signed_cert_omits_timestamp(self):
        calls = self._run_dmg_with_fake_codesign("Fabushi Local Dev")
        self.assertEqual(len(calls), 1)
        self.assertNotIn("--timestamp", calls[0])
        self.assertIn("--sign Fabushi Local Dev", calls[0])

    def test_developer_id_cert_includes_timestamp(self):
        calls = self._run_dmg_with_fake_codesign("Developer ID Application: Example Corp (TEAM123)")
        self.assertEqual(len(calls), 1)
        self.assertIn("--timestamp", calls[0])
        self.assertIn("--sign Developer ID Application: Example Corp (TEAM123)", calls[0])


if __name__ == "__main__":
    unittest.main()
