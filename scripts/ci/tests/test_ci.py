"""版本与发布门禁回归，不访问 GitHub 或实际发布资产。"""

import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from metadata import CONFIG, metadata
from release import validate, verify_tag

SHA = "a" * 40
NOW = 1_790_510_400


class Versions(unittest.TestCase):
    def test_branch_cannot_inject_names_or_publish(self):
        value = metadata("refs/heads/x;$(echo unsafe)", SHA, "version: 1.2.3+7", NOW)
        self.assertEqual(value["version"], "1.2.3-dev.aaaaaaaaaaaa")
        self.assertEqual(value["release"], "false")

    def test_release_and_prerelease_use_tag_version(self):
        stable = metadata("refs/tags/v2.3.4", SHA, "version: 1.0.0", NOW)
        self.assertEqual(stable["build_name"], "2.3.4")
        self.assertEqual(stable["release"], "true")
        self.assertEqual(stable["prerelease"], "false")
        self.assertEqual(metadata("refs/tags/v2.3.4-rc.1", SHA, "", NOW)["prerelease"], "true")

    def test_invalid_tags_fail(self):
        for tag in ["v01.2.3", "v1.2", "latest", "v1.2.3-rc.01", "v1.2.3;bad", "v1.2.3/other"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                metadata(f"refs/tags/{tag}", SHA, "", NOW)


class ReleaseGate(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.directory = Path(self.temp.name)
        for row in CONFIG["targets"]:
            target = row["id"]
            name = f"{target}.zip"
            data = target.encode()
            (self.directory / name).write_bytes(data)
            manifest = {"target": target, "version": "2.3.4", "commit": SHA, "build_number": "123",
                        "signing": "release" if target == "android" else "unsigned",
                        "files": [{"name": name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}]}
            (self.directory / f"manifest-{target}.json").write_text(json.dumps(manifest))

    def tearDown(self):
        self.temp.cleanup()

    def test_complete_matrix(self):
        self.assertEqual(len(validate(self.directory, "2.3.4", SHA)), len(CONFIG["targets"]))

    def test_missing_target_rejected(self):
        (self.directory / "manifest-android.json").unlink()
        with self.assertRaises(ValueError):
            validate(self.directory, "2.3.4", SHA)

    def test_modified_asset_rejected(self):
        (self.directory / "android.zip").write_bytes(b"tampered")
        with self.assertRaises(ValueError):
            validate(self.directory, "2.3.4", SHA)

    def test_other_commit_rejected(self):
        with self.assertRaises(ValueError):
            validate(self.directory, "2.3.4", "b" * 40)

    def test_secrets_or_unlisted_files_rejected(self):
        (self.directory / "release.jks").write_bytes(b"not-an-artifact")
        with self.assertRaises(ValueError):
            validate(self.directory, "2.3.4", SHA)

    def test_development_android_key_rejected(self):
        path = self.directory / "manifest-android.json"
        data = json.loads(path.read_text())
        data["signing"] = "development"
        path.write_text(json.dumps(data))
        with self.assertRaises(ValueError):
            validate(self.directory, "2.3.4", SHA)

    def test_remote_tag_must_still_reference_the_built_commit(self):
        with patch("release.subprocess.check_output", return_value=SHA + "\n"):
            verify_tag("owner/repository", "v2.3.4", SHA)
        with patch("release.subprocess.check_output", return_value="b" * 40), self.assertRaises(RuntimeError):
            verify_tag("owner/repository", "v2.3.4", SHA)


if __name__ == "__main__":
    unittest.main()
