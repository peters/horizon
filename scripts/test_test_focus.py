#!/usr/bin/env python3
"""Tests for scripts/test-focus.py."""

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "test-focus.py"


def load_module():
    """Import the hyphenated script by path."""
    spec = importlib.util.spec_from_file_location("test_focus", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    # A dataclass resolves its own module while it is created.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


focus = load_module()


def write_crate(root, directory, name, files, bins=()):
    """Create a crate fixture and return its directory."""
    crate = root / directory
    (crate / "src").mkdir(parents=True, exist_ok=True)
    manifest = f'[package]\nname = "{name}"\nversion = "0.1.0"\n'
    for binary in bins:
        manifest += f'\n[[bin]]\nname = "{binary}"\npath = "src/{binary}.rs"\n'
        files = [*files, f"src/{binary}.rs"]
    (crate / "Cargo.toml").write_text(manifest, encoding="utf-8")
    for entry in files:
        target = crate / entry
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("", encoding="utf-8")
    return crate


class ModuleFilterTests(unittest.TestCase):
    """The filter is the module path that the test harness matches."""

    def test_nested_module_file(self):
        self.assertEqual(
            focus.module_filter(Path("src/cloud_runtime/deployment/hetzner/mod.rs")),
            "cloud_runtime::deployment::hetzner::",
        )

    def test_leaf_file_keeps_its_stem(self):
        self.assertEqual(
            focus.module_filter(Path("src/browser/webdriver.rs")),
            "browser::webdriver::",
        )

    def test_crate_root_has_no_module(self):
        self.assertIsNone(focus.module_filter(Path("src/lib.rs")))
        self.assertIsNone(focus.module_filter(Path("src/main.rs")))

    def test_binary_source_has_no_module(self):
        self.assertIsNone(focus.module_filter(Path("src/bin/horizon-browser.rs")))

    def test_path_outside_src_has_no_module(self):
        self.assertIsNone(focus.module_filter(Path("tests/session_lifecycle.rs")))


class SelectionTests(unittest.TestCase):
    """Each kind of changed path becomes one test target and filter."""

    def setUp(self):
        self.hold = tempfile.TemporaryDirectory()
        self.root = Path(self.hold.name)
        (self.root / "Cargo.toml").write_text(
            "[workspace]\nmembers = []\n", encoding="utf-8"
        )
        self.crate = write_crate(
            self.root,
            "crates/demo",
            "demo",
            [
                "src/lib.rs",
                "src/board/mod.rs",
                "src/cloud_runtime/deployment/mod.rs",
                "tests/session_lifecycle.rs",
                "tests/common/mod.rs",
                "examples/deploy.rs",
                "examples/navigate.json",
                "examples/cloud_deploy/main.rs",
                "build.rs",
                "README.md",
            ],
        )

    def tearDown(self):
        self.hold.cleanup()

    def select(self, entry):
        return focus.selection_for(self.root / entry, self.crate)

    def test_library_module_gets_a_filter(self):
        self.assertEqual(
            self.select("crates/demo/src/board/mod.rs"),
            focus.Selection("demo", ("lib", None), ("board::",)),
        )

    def test_crate_manifest_runs_the_whole_library(self):
        self.assertEqual(
            self.select("crates/demo/Cargo.toml"),
            focus.Selection("demo", ("lib", None)),
        )

    def test_integration_file_is_its_own_target(self):
        self.assertEqual(
            self.select("crates/demo/tests/session_lifecycle.rs"),
            focus.Selection("demo", ("test", "session_lifecycle")),
        )

    def test_integration_helper_covers_the_whole_tests_directory(self):
        self.assertEqual(
            self.select("crates/demo/tests/common/mod.rs"),
            focus.Selection("demo", ("tests", None)),
        )

    def test_example_is_its_own_target(self):
        self.assertEqual(
            self.select("crates/demo/examples/deploy.rs"),
            focus.Selection("demo", ("example", "deploy")),
        )

    def test_example_fixture_beside_the_examples_covers_every_example(self):
        self.assertEqual(
            self.select("crates/demo/examples/navigate.json"),
            focus.Selection("demo", ("examples", None)),
        )

    def test_directory_style_example_is_one_target(self):
        self.assertEqual(
            self.select("crates/demo/examples/cloud_deploy/main.rs"),
            focus.Selection("demo", ("example", "cloud_deploy")),
        )

    def test_build_script_runs_the_default_target_and_the_tests(self):
        crate = write_crate(
            self.root, "crates/built", "built", ["src/lib.rs", "build.rs", "tests/one.rs"]
        )
        self.assertEqual(
            focus.selection_for(self.root / "crates/built/build.rs", crate),
            [
                focus.Selection("built", ("lib", None)),
                focus.Selection("built", ("tests", None)),
            ],
        )

    def test_a_build_input_file_runs_the_targets_that_compile_it(self):
        crate = write_crate(
            self.root,
            "crates/built",
            "built",
            ["src/lib.rs", "build/fonts.rs", "tests/fonts.rs"],
        )
        self.assertEqual(
            focus.selection_for(self.root / "crates/built/build/fonts.rs", crate),
            [
                focus.Selection("built", ("lib", None)),
                focus.Selection("built", ("tests", None)),
            ],
        )

    def test_a_document_inside_a_crate_names_the_full_matrix(self):
        crate = write_crate(self.root, "crates/doc", "doc", ["src/lib.rs", "README.md"])
        outcome = focus.selection_for(self.root / "crates/doc/README.md", crate)
        self.assertIsInstance(outcome, str)
        self.assertIn("full matrix", outcome)

    def test_explicit_binary_source_is_its_own_target(self):
        crate = write_crate(self.root, "crates/many", "many", ["src/lib.rs"])
        self.assertEqual(
            focus.selection_for(self.root / "crates/many/src/bin/first.rs", crate),
            focus.Selection("many", ("bin", "first")),
        )

    def test_main_only_crate_takes_the_package_name(self):
        crate = write_crate(self.root, "crates/mainly", "mainly", ["src/main.rs"])
        self.assertEqual(
            focus.selection_for(self.root / "crates/mainly/src/main.rs", crate),
            focus.Selection("mainly", ("bin", "mainly")),
        )

    def test_file_without_a_test_target_is_a_note(self):
        outcome = self.select("crates/demo/README.md")
        self.assertIsInstance(outcome, str)
        self.assertIn("full matrix", outcome)

    def test_binary_only_crate_selects_its_one_binary(self):
        crate = write_crate(self.root, "crates/app", "app-ui", [], bins=["app"])
        self.assertEqual(
            focus.selection_for(self.root / "crates/app/src/browser_requests.rs", crate),
            focus.Selection("app-ui", ("bin", "app"), ("browser_requests::",)),
        )

    def test_binary_only_crate_with_several_binaries_selects_all(self):
        crate = write_crate(
            self.root, "crates/tools", "tools", ["src/shared.rs"], bins=["one", "two"]
        )
        self.assertEqual(
            focus.selection_for(self.root / "crates/tools/src/shared.rs", crate),
            focus.Selection("tools", ("bins", None), ("shared::",)),
        )

    def test_a_binary_root_selects_its_whole_binary(self):
        crate = write_crate(self.root, "crates/tool", "tool", [], bins=["tool"])
        self.assertEqual(
            focus.selection_for(self.root / "crates/tool/src/tool.rs", crate),
            focus.Selection("tool", ("bin", "tool")),
        )


class MixedCrateTests(unittest.TestCase):
    """A filter follows the root that declares the module."""

    def setUp(self):
        self.hold = tempfile.TemporaryDirectory()
        self.root = Path(self.hold.name)
        (self.root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        self.crate = write_crate(
            self.root,
            "crates/mixed",
            "mixed",
            ["src/lib.rs", "src/main.rs", "src/shared.rs", "src/onlybin.rs"],
        )
        (self.crate / "src" / "lib.rs").write_text("pub mod shared;\n", encoding="utf-8")
        (self.crate / "src" / "main.rs").write_text("mod onlybin;\n", encoding="utf-8")

    def tearDown(self):
        self.hold.cleanup()

    def select(self, entry):
        return focus.selection_for(self.root / entry, self.crate)

    def test_a_library_module_filters_the_library(self):
        self.assertEqual(
            self.select("crates/mixed/src/shared.rs"),
            focus.Selection("mixed", ("lib", None), ("shared::",)),
        )

    def test_a_binary_module_filters_the_binary(self):
        self.assertEqual(
            self.select("crates/mixed/src/onlybin.rs"),
            focus.Selection("mixed", ("bin", "mixed"), ("onlybin::",)),
        )

    def test_the_main_root_selects_the_whole_binary(self):
        self.assertEqual(
            self.select("crates/mixed/src/main.rs"),
            focus.Selection("mixed", ("bin", "mixed")),
        )

    def test_the_library_root_selects_the_whole_library(self):
        self.assertEqual(
            self.select("crates/mixed/src/lib.rs"),
            focus.Selection("mixed", ("lib", None)),
        )


class PathAttributeTests(unittest.TestCase):
    """A `#[path]` module takes its name from the module that declares it."""

    def setUp(self):
        self.hold = tempfile.TemporaryDirectory()
        self.root = Path(self.hold.name)
        (self.root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        self.crate = write_crate(
            self.root,
            "crates/demo",
            "demo",
            [
                "src/lib.rs",
                "src/endpoint.rs",
                "src/endpoint/tailnet.rs",
                "src/endpoint/tests/tailnet.rs",
                "src/relay.rs",
                "src/tests/unix_relay.rs",
                "src/tests/budgets.rs",
                "src/tests/graceful.rs",
                "src/moved.rs",
                "src/board/mod.rs",
            ],
        )
        source = self.crate / "src"
        (source / "lib.rs").write_text(
            'pub mod endpoint;\npub mod board;\n#[path = "moved.rs"]\nmod video;\n',
            encoding="utf-8",
        )
        (source / "endpoint.rs").write_text("pub mod tailnet;\n", encoding="utf-8")
        (source / "endpoint" / "tailnet.rs").write_text(
            '#[cfg(test)]\n#[path = "tests/tailnet.rs"]\nmod tests;\n',
            encoding="utf-8",
        )
        (source / "relay.rs").write_text(
            '#[path = "tests/unix_relay.rs"]\nmod unix_tests;\n'
            '#[path = "tests/budgets.rs"]\nmod tests;\n',
            encoding="utf-8",
        )
        (source / "tests" / "budgets.rs").write_text(
            '#[path = "graceful.rs"]\nmod graceful;\n', encoding="utf-8"
        )

    def tearDown(self):
        self.hold.cleanup()

    def select(self, entry):
        return focus.selection_for(self.root / entry, self.crate)

    def test_a_path_attribute_in_a_neighbour_file_names_the_module(self):
        self.assertEqual(
            self.select("crates/demo/src/endpoint/tests/tailnet.rs"),
            focus.Selection("demo", ("lib", None), ("endpoint::tailnet::tests::",)),
        )

    def test_a_path_attribute_from_the_source_root_names_the_module(self):
        self.assertEqual(
            self.select("crates/demo/src/tests/unix_relay.rs"),
            focus.Selection("demo", ("lib", None), ("relay::unix_tests::",)),
        )

    def test_a_path_attribute_in_a_crate_root_names_a_top_level_module(self):
        self.assertEqual(
            self.select("crates/demo/src/moved.rs"),
            focus.Selection("demo", ("lib", None), ("video::",)),
        )

    def test_a_chain_of_path_attributes_keeps_every_declaring_module(self):
        self.assertEqual(
            self.select("crates/demo/src/tests/graceful.rs"),
            focus.Selection("demo", ("lib", None), ("relay::tests::graceful::",)),
        )

    def test_a_file_without_a_path_attribute_keeps_its_place(self):
        self.assertEqual(
            self.select("crates/demo/src/board/mod.rs"),
            focus.Selection("demo", ("lib", None), ("board::",)),
        )


class NestedDeclarationTests(unittest.TestCase):
    """A plain child of a `#[path]` module keeps the declared parent name."""

    def setUp(self):
        self.hold = tempfile.TemporaryDirectory()
        self.root = Path(self.hold.name)
        (self.root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        self.crate = write_crate(
            self.root,
            "crates/ui",
            "ui",
            [
                "src/lib.rs",
                "src/app.rs",
                "src/app/lifecycle.rs",
                "src/app/lifecycle/startup_organization_tests.rs",
                "src/app/lifecycle/startup_organization_tests/mod.rs",
                "src/app/lifecycle/startup_organization_tests/layout.rs",
            ],
        )
        source = self.crate / "src"
        (source / "lib.rs").write_text("pub mod app;\n", encoding="utf-8")
        (source / "app.rs").write_text("pub mod lifecycle;\n", encoding="utf-8")
        (source / "app" / "lifecycle.rs").write_text(
            "mod startup_organization_tests;\n", encoding="utf-8"
        )
        (source / "app" / "lifecycle" / "startup_organization_tests.rs").write_text(
            '#[path = "startup_organization_tests/mod.rs"]\nmod tests;\n',
            encoding="utf-8",
        )
        (
            source / "app" / "lifecycle" / "startup_organization_tests" / "mod.rs"
        ).write_text("mod layout;\n", encoding="utf-8")

    def tearDown(self):
        self.hold.cleanup()

    def test_a_plain_child_keeps_the_declared_parent_name(self):
        self.assertEqual(
            focus.selection_for(
                self.root
                / "crates/ui/src/app/lifecycle/startup_organization_tests/layout.rs",
                self.crate,
            ),
            focus.Selection(
                "ui",
                ("lib", None),
                ("app::lifecycle::startup_organization_tests::tests::layout::",),
            ),
        )


class PlanTests(unittest.TestCase):
    """A plan groups the change by crate and target."""

    def setUp(self):
        self.hold = tempfile.TemporaryDirectory()
        self.root = Path(self.hold.name)
        (self.root / "Cargo.toml").write_text(
            "[workspace]\nmembers = []\n", encoding="utf-8"
        )
        write_crate(
            self.root,
            "crates/one",
            "one",
            ["src/lib.rs", "src/board/mod.rs", "src/panel.rs"],
        )
        write_crate(self.root, "crates/two", "two", ["src/lib.rs", "src/cloud/mod.rs"])
        (self.root / "docs").mkdir()
        (self.root / "docs" / "note.md").write_text("", encoding="utf-8")

    def tearDown(self):
        self.hold.cleanup()

    def test_modules_of_one_crate_merge_into_one_selection(self):
        plan = focus.build_plan(
            [
                "crates/one/src/board/mod.rs",
                "crates/one/src/panel.rs",
                "crates/one/src/board/mod.rs",
            ],
            self.root,
        )
        self.assertEqual(
            plan.selections,
            [focus.Selection("one", ("lib", None), ("board::", "panel::"))],
        )

    def test_whole_target_wins_over_a_module_filter(self):
        plan = focus.build_plan(
            [
                "crates/one/src/lib.rs",
                "crates/one/src/board/mod.rs",
            ],
            self.root,
        )
        self.assertEqual(plan.selections, [focus.Selection("one", ("lib", None))])

    def test_separate_crates_stay_separate(self):
        plan = focus.build_plan(
            ["crates/one/src/panel.rs", "crates/two/src/cloud/mod.rs"], self.root
        )
        self.assertEqual(
            plan.selections,
            [
                focus.Selection("one", ("lib", None), ("panel::",)),
                focus.Selection("two", ("lib", None), ("cloud::",)),
            ],
        )

    def test_workspace_wide_and_outside_paths_select_nothing(self):
        plan = focus.build_plan(["Cargo.lock", "docs/note.md"], self.root)
        self.assertEqual(plan.selections, [])
        self.assertEqual(
            [entry for entry, _ in plan.unselected], ["Cargo.lock", "docs/note.md"]
        )
        self.assertIn("full matrix", plan.unselected[0][1])
        self.assertIn("full matrix", plan.unselected[1][1])


class CommandTests(unittest.TestCase):
    """The same selection renders as one nextest command or one per filter."""

    def setUp(self):
        self.selection = focus.Selection("core", ("lib", None), ("board::", "panel::"))

    def test_cargo_test_runs_one_command_per_filter(self):
        self.assertEqual(
            focus.commands(self.selection, use_nextest=False, whole_crate=False),
            [
                ["cargo", "test", "-p", "core", "--lib", "board::"],
                ["cargo", "test", "-p", "core", "--lib", "panel::"],
            ],
        )

    def test_nextest_combines_the_filters(self):
        self.assertEqual(
            focus.commands(self.selection, use_nextest=True, whole_crate=False),
            [
                [
                    "cargo",
                    "nextest",
                    "run",
                    "-p",
                    "core",
                    "--lib",
                    "-E",
                    "test(board::) | test(panel::)",
                ]
            ],
        )

    def test_whole_crate_drops_the_filter(self):
        self.assertEqual(
            focus.commands(self.selection, use_nextest=True, whole_crate=True),
            [["cargo", "nextest", "run", "-p", "core", "--lib"]],
        )

    def test_target_name_is_passed_and_unfiltered(self):
        selection = focus.Selection("ui", ("bin", "horizon"), ("app::",))
        self.assertEqual(
            focus.commands(selection, use_nextest=False, whole_crate=True),
            [["cargo", "test", "-p", "ui", "--bin", "horizon"]],
        )


class NoTestDetectionTests(unittest.TestCase):
    """A filter that selects nothing is a warning, not a failure."""

    def test_libtest_zero_selection(self):
        output = (
            "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; "
            "0 measured; 113 filtered out; finished in 0.00s"
        )
        self.assertFalse(focus.has_tests(output))

    def test_nextest_zero_selection(self):
        output = (
            "    Starting 0 tests across 1 binary (2114 tests skipped)\n"
            "     Summary [   0.002s] 0 tests run: 0 passed, 2114 skipped\n"
            "error: no tests to run"
        )
        self.assertFalse(focus.has_tests(output))

    def test_passing_run_has_tests(self):
        self.assertTrue(
            focus.has_tests(
                "test result: ok. 227 passed; 0 failed; 0 ignored; finished in 0.38s"
            )
        )
        self.assertTrue(
            focus.has_tests("Summary [   0.383s] 227 tests run: 227 passed, 1887 skipped")
        )

    def test_a_compile_error_is_not_an_empty_selection(self):
        self.assertTrue(focus.has_tests("error[E0308]: mismatched types"))

    def test_one_empty_binary_does_not_empty_a_multi_binary_run(self):
        output = (
            "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 ignored out\n"
            "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out"
        )
        self.assertTrue(focus.has_tests(output))

    def test_a_refused_run_is_reported_as_no_tests(self):
        self.assertTrue(focus.reports_no_tests("error: no tests to run"))
        self.assertFalse(focus.reports_no_tests("test result: FAILED. 1 passed; 1 failed"))


class CommandLineTests(unittest.TestCase):
    """The script prints commands for a real repository path."""

    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), *args],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_print_shows_the_narrow_command(self):
        result = self.run_script(
            "--print", "--no-nextest", "crates/horizon-core/src/board/tests/mod.rs"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("horizon-core: lib -> board::tests::", result.stdout)
        self.assertIn(
            "$ cargo test -p horizon-core --lib board::tests::", result.stdout
        )

    def test_print_of_a_workspace_file_explains_the_full_matrix(self):
        result = self.run_script("--print", "Cargo.lock")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("skipped Cargo.lock", result.stdout)
        self.assertIn("full matrix", result.stdout)
        self.assertNotIn("$ cargo", result.stdout)

    def test_no_selection_names_the_base(self):
        result = self.run_script("--print", "docs/architecture/maintainability.md")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("$ cargo", result.stdout)


FAKE_CARGO = """#!/usr/bin/env python3
import os, sys
args = sys.argv[1:]
with open(os.environ["FAKE_CARGO_LOG"], "a", encoding="utf-8") as log:
    log.write(" ".join(args) + "\\n")
if args[:2] == ["nextest", "--version"]:
    print("cargo-nextest 0.9.99")
    sys.exit(0 if os.environ.get("FAKE_NEXTEST") == "1" else 1)
mode = os.environ.get("FAKE_CARGO_MODE", "pass")
if mode == "fail":
    print("error[E0308]: mismatched types")
    sys.exit(101)
if mode == "mixed":
    print("running 0 tests")
    print()
    print("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 ignored out")
    print()
    print("test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out")
    sys.exit(101)
if mode == "none":
    print("running 0 tests")
    print()
    print("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out")
    sys.exit(0)
print("running 3 tests")
print()
print("test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out")
"""


class CrateRootTests(unittest.TestCase):
    """A crate is found from any depth, and only inside the repository."""

    def setUp(self):
        self.hold = tempfile.TemporaryDirectory()
        self.root = Path(self.hold.name)
        (self.root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        write_crate(self.root, "crates/outer", "outer", ["src/lib.rs"])
        write_crate(self.root, "crates/outer/inner", "inner", ["src/lib.rs"])
        (self.root / "docs").mkdir()

    def tearDown(self):
        self.hold.cleanup()

    def test_the_nearest_crate_wins(self):
        self.assertEqual(
            focus.crate_root(self.root / "crates/outer/inner/src/lib.rs", self.root),
            self.root / "crates/outer/inner",
        )

    def test_a_path_outside_every_crate_has_no_crate(self):
        (self.root / "docs" / "note.md").write_text("", encoding="utf-8")
        self.assertIsNone(focus.crate_root(self.root / "docs" / "note.md", self.root))

    def test_the_workspace_manifest_has_no_crate(self):
        self.assertIsNone(focus.crate_root(self.root / "Cargo.toml", self.root))


class NextestProbeTests(unittest.TestCase):
    """A missing cargo is not an installed nextest."""

    def test_missing_cargo_reports_no_nextest(self):
        from unittest import mock

        with mock.patch.object(focus.subprocess, "run", side_effect=FileNotFoundError):
            self.assertFalse(focus.nextest_available())


@unittest.skipIf(os.name == "nt", "the fake cargo has a POSIX shebang")
class RunLoopTests(unittest.TestCase):
    """The run loop reports status, keeps going on request, and warns on no test."""

    board = "crates/horizon-core/src/board/mod.rs"
    panel = "crates/horizon-core/src/panel.rs"

    def setUp(self):
        self.hold = tempfile.TemporaryDirectory()
        self.bin = Path(self.hold.name)
        self.log = self.bin / "cargo.log"
        fake = self.bin / "cargo"
        fake.write_text(FAKE_CARGO, encoding="utf-8")
        fake.chmod(0o755)

    def tearDown(self):
        self.hold.cleanup()

    def run_script(self, args, mode="pass", nextest=False):
        environment = dict(os.environ)
        environment["PATH"] = f"{self.bin}{os.pathsep}{environment['PATH']}"
        environment["FAKE_CARGO_LOG"] = str(self.log)
        environment["FAKE_CARGO_MODE"] = mode
        environment["FAKE_NEXTEST"] = "1" if nextest else "0"
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), *args],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )

    def invocations(self):
        """Return the cargo runs, without the nextest probe."""
        if not self.log.exists():
            return []
        lines = self.log.read_text(encoding="utf-8").splitlines()
        return [line for line in lines if line != "nextest --version"]

    def test_a_passing_run_exits_zero(self):
        result = self.run_script([self.board])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.invocations(), ["test -p horizon-core --lib board::"])

    def test_a_failing_run_reports_its_status_and_stops(self):
        result = self.run_script([self.board, self.panel], mode="fail")
        self.assertEqual(result.returncode, 101)
        self.assertEqual(len(self.invocations()), 1)

    def test_keep_going_runs_every_selection(self):
        result = self.run_script([self.board, self.panel, "--keep-going"], mode="fail")
        self.assertEqual(result.returncode, 101)
        self.assertEqual(len(self.invocations()), 2)

    def test_an_empty_binary_beside_a_failure_reports_the_failure(self):
        result = self.run_script([self.board], mode="mixed")
        self.assertEqual(result.returncode, 101, result.stderr)
        self.assertNotIn("selected no test", result.stderr)

    def test_a_filter_without_tests_warns_and_exits_zero(self):
        result = self.run_script([self.board], mode="none")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("selected no test", result.stderr)
        self.assertIn("--features", result.stderr)

    def test_features_are_passed_to_cargo(self):
        result = self.run_script([self.board, "--features", "one-service-gateway"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.invocations(),
            ["test -p horizon-core --lib --features one-service-gateway board::"],
        )

    def test_the_printed_nextest_command_is_shell_safe(self):
        result = self.run_script(["--print", self.board, self.panel], nextest=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "$ cargo nextest run -p horizon-core --lib -E 'test(board::) | test(panel::)'",
            result.stdout,
        )


if __name__ == "__main__":
    unittest.main()
