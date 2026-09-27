"""通过临时 Git 历史验证构建来源、签名入口及汇总门禁。"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import android_signing
from metadata import metadata
from policy import build_allowed, check_results, require_build_source, require_release_source


class SourcePolicy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.git("init", "--initial-branch=main")
        self.main = self.commit("主分支已有提交")
        self.git("update-ref", "refs/remotes/origin/main", self.main)
        self.dev = self.commit("开发分支已有提交")
        self.git("update-ref", "refs/remotes/origin/dev", self.dev)
        self.feature = self.commit("尚未合入的功能提交")

    def tearDown(self):
        self.temp.cleanup()

    def git(self, *args):
        return subprocess.check_output(
            ["git", "-c", "user.name=CI", "-c", "user.email=ci@example.invalid", "-c", "commit.gpgsign=false", *args],
            cwd=self.root, text=True, stderr=subprocess.STDOUT,
        ).strip()

    def commit(self, message):
        self.git("commit", "--allow-empty", "-m", message)
        return self.git("rev-parse", "HEAD")

    def env(self, event="push", ref="refs/heads/main", commit=None):
        commit = self.feature if commit is None else commit
        return {"GITHUB_EVENT_NAME": event, "GITHUB_REF": ref, "GITHUB_SHA": commit, "GUOSH_CI_COMMIT": commit}

    def test_only_main_and_dev_branch_pushes_or_dispatches_can_build(self):
        for event in ("push", "workflow_dispatch"):
            for branch in ("main", "dev"):
                with self.subTest(event=event, branch=branch):
                    self.assertTrue(build_allowed(event, f"refs/heads/{branch}", self.feature, self.root))
            for branch in ("feature", "main/feature", "developer", "Main"):
                with self.subTest(event=event, branch=branch):
                    self.assertFalse(build_allowed(event, f"refs/heads/{branch}", self.feature, self.root))

    def test_pr_events_cannot_authorize_even_with_trusted_branch_names(self):
        for event in ("pull_request", "pull_request_target", "workflow_run", "schedule", ""):
            for ref in ("refs/pull/1/merge", "refs/heads/main", "refs/heads/dev", "refs/tags/v1.0.0"):
                with self.subTest(event=event, ref=ref):
                    self.assertFalse(build_allowed(event, ref, self.main, self.root))

    def test_fork_pr_head_and_base_names_do_not_grant_signing(self):
        env = self.env("pull_request", "refs/pull/1/merge")
        env.update(GITHUB_HEAD_REF="main", GITHUB_BASE_REF="main")
        with self.assertRaises(RuntimeError):
            require_build_source(env, self.root)

    def test_version_tag_must_be_in_either_remote_trusted_branch(self):
        for commit in (self.main, self.dev):
            with self.subTest(commit=commit):
                self.assertTrue(build_allowed("push", "refs/tags/v1.0.0-rc.1", commit, self.root))
        with self.assertRaises(RuntimeError):
            build_allowed("push", "refs/tags/v1.0.0", self.feature, self.root)

    def test_local_branch_or_tag_name_does_not_prove_trust(self):
        self.git("update-ref", "-d", "refs/remotes/origin/main")
        self.git("update-ref", "-d", "refs/remotes/origin/dev")
        self.git("tag", "v1.0.0")
        with self.assertRaises(RuntimeError):
            build_allowed("push", "refs/tags/v1.0.0", self.feature, self.root)

    def test_manual_tags_cannot_build_or_publish(self):
        self.assertFalse(build_allowed("workflow_dispatch", "refs/tags/v1.0.0", self.main, self.root))
        with self.assertRaises(RuntimeError):
            require_release_source(self.env("workflow_dispatch", "refs/tags/v1.0.0"), self.root)

    def test_invalid_version_tag_fails_closed(self):
        for tag in ("v1.0", "v01.0.0", "v1.0.0-rc.01"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                build_allowed("push", f"refs/tags/{tag}", self.main, self.root)

    def test_build_and_signature_require_actual_event_commit(self):
        require_build_source(self.env(), self.root)
        with self.assertRaises(RuntimeError):
            require_build_source(self.env(commit=self.main), self.root)
        env = self.env()
        env["GITHUB_SHA"] = self.main
        with self.assertRaises(RuntimeError):
            require_build_source(env, self.root)
        with self.assertRaises(ValueError):
            require_build_source({}, self.root)

    def test_only_trusted_tag_push_can_release(self):
        with self.assertRaises(RuntimeError):
            require_release_source(self.env(), self.root)
        with self.assertRaises(RuntimeError):
            require_release_source(self.env(ref="refs/tags/v1.0.0"), self.root)
        self.git("checkout", "--detach", self.dev)
        require_release_source(self.env(ref="refs/tags/v1.0.0", commit=self.dev), self.root)

    def test_signing_rejects_pr_before_decoding_or_writing_key(self):
        env = self.env("pull_request", "refs/pull/1/merge")
        env["ANDROID_KEYSTORE_BASE64"] = "not-valid-base64"
        key = self.root / "signing.p12"
        with patch.dict(os.environ, env, clear=True), patch.object(android_signing, "KEYSTORE", key), \
             patch.object(sys, "argv", ["android_signing.py"]), \
             patch.object(android_signing, "require_build_source", side_effect=lambda: require_build_source(env, self.root)):
            with self.assertRaisesRegex(RuntimeError, "仅允许静态检查"):
                android_signing.main()
        self.assertFalse(key.exists())

    def test_missing_environment_key_cannot_fall_back_to_development(self):
        env = self.env()
        with patch.dict(os.environ, env, clear=True), patch.object(sys, "argv", ["android_signing.py"]), \
             patch.object(android_signing, "require_build_source", side_effect=lambda: require_build_source(env, self.root)):
            with self.assertRaisesRegex(RuntimeError, "android-signing Environment"):
                android_signing.main()


class ResultPolicy(unittest.TestCase):
    def jobs(self, allowed):
        return {"metadata": {"result": "success", "outputs": {"build_allowed": str(allowed).lower()}},
                "checks": {"result": "success"},
                "build": {"result": "success" if allowed else "skipped"},
                "android": {"result": "success" if allowed else "skipped"}}

    def test_static_only_runs_require_both_build_jobs_skipped(self):
        for event, ref in (("pull_request", "refs/pull/1/merge"), ("pull_request_target", "refs/heads/main"),
                           ("push", "refs/heads/feature"), ("workflow_dispatch", "refs/heads/feature")):
            with self.subTest(event=event, ref=ref):
                check_results(self.jobs(False), event, ref)
                for job in ("build", "android"):
                    jobs = self.jobs(False)
                    jobs[job]["result"] = "success"
                    with self.assertRaises(RuntimeError):
                        check_results(jobs, event, ref)

    def test_trusted_runs_require_all_checks_and_builds(self):
        check_results(self.jobs(True), "push", "refs/heads/main")
        check_results(self.jobs(True), "push", "refs/tags/v1.0.0")
        for job in ("metadata", "checks", "build", "android"):
            for result in ("skipped", "failure", "cancelled"):
                jobs = self.jobs(True)
                jobs[job]["result"] = result
                with self.subTest(job=job, result=result), self.assertRaises(RuntimeError):
                    check_results(jobs, "push", "refs/heads/dev")

    def test_metadata_output_cannot_authorize_untrusted_event(self):
        with self.assertRaises(RuntimeError):
            check_results(self.jobs(True), "pull_request", "refs/pull/1/merge")
        with self.assertRaises(RuntimeError):
            check_results(self.jobs(True), "push", "refs/heads/feature")
        with self.assertRaises(RuntimeError):
            check_results(self.jobs(False), "push", "refs/heads/main")

    def test_android_is_excluded_from_unsigned_build_matrix(self):
        values = metadata("refs/heads/main", "a" * 40, "version: 1.0.0", 1_790_510_400)
        matrix = json.loads(values["matrix"])["include"]
        android = json.loads(values["android_matrix"])["include"]
        self.assertEqual(len(matrix), 8)
        self.assertTrue(all(target["platform"] != "android" for target in matrix))
        self.assertEqual([target["id"] for target in android], ["android"])


if __name__ == "__main__":
    unittest.main()
