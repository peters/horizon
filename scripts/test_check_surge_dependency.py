#!/usr/bin/env python3
"""Regression coverage for recurring lockfile-only Surge updates."""

import unittest

from check_surge_dependency import check


class SurgeDependencyTests(unittest.TestCase):
    def setUp(self):
        self.root = {"workspace": {"dependencies": {}}}
        self.pin = {"git": "https://github.com/fintermobilityas/surge.git", "tag": "v1.0.0-beta.61"}
        self.ui = {"dependencies": {"surge-core": self.pin}}
        self.lock = {"package": [{"name": "surge-core", "source": self.source(self.pin["tag"])}]}

    def source(self, tag):
        return f"git+{self.pin['git']}?tag={tag}#0123456789abcdef"

    def test_current_direct_pin(self):
        self.assertEqual(check(self.root, self.ui, self.lock), [])

    def test_next_release_updates_both_files(self):
        self.pin["tag"] = "v1.0.0-beta.62"
        self.lock["package"][0]["source"] = self.source(self.pin["tag"])
        self.assertEqual(check(self.root, self.ui, self.lock), [])

    def test_original_workspace_inheritance_is_rejected(self):
        self.root["workspace"]["dependencies"]["surge-core"] = self.pin
        self.ui["dependencies"]["surge-core"] = {"workspace": True}
        self.assertTrue(any("workspace updater" in e for e in check(self.root, self.ui, self.lock)))

    def test_lockfile_only_update_is_rejected(self):
        self.lock["package"][0]["source"] = self.source("v1.0.0-beta.62")
        self.assertTrue(check(self.root, self.ui, self.lock))

    def test_manifest_only_update_is_rejected(self):
        self.pin["tag"] = "v1.0.0-beta.62"
        self.assertTrue(check(self.root, self.ui, self.lock))

    def test_different_repository_is_rejected(self):
        self.pin["git"] = "https://example.com/surge.git"
        self.assertTrue(check(self.root, self.ui, self.lock))

    def test_multiple_locked_versions_are_rejected(self):
        self.lock["package"].append(dict(self.lock["package"][0]))
        self.assertTrue(check(self.root, self.ui, self.lock))


if __name__ == "__main__":
    unittest.main()
