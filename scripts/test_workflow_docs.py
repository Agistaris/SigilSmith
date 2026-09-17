#!/usr/bin/env python3
"""Cheap guidance checks; no build, release, application data, or sibling reads."""

import re
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = "docs/WORKFLOW.md"
ROUTES = {
    "../README.md#run-dev": ("README.md", "## Run (Dev)"),
    "../README.md#from-source": ("README.md", "### From Source"),
    "INSTALL.md": ("docs/INSTALL.md", None),
    "RELEASE.md": ("docs/RELEASE.md", None),
    "../scripts/test_workflow_docs.py": ("scripts/test_workflow_docs.py", None),
}


def live_text(relative: str) -> str:
    return re.sub(r"<!--.*?-->", "", (REPO_ROOT / relative).read_text(encoding="utf-8"), flags=re.DOTALL)


def document_references(text: str) -> set[str]:
    """Use actual link destinations or inline path/command tokens, not labels."""
    links = r"\[[^\]\n]+\]\(([^)\s]+)\)"
    destinations = set(re.findall(links, text))
    inline = re.findall(r"`([^`\n]+)`", re.sub(links, "", text))
    return destinations | {token for span in inline for token in span.split()}


class WorkflowDocumentationTests(unittest.TestCase):
    def test_readme_routes_to_portable_guidance(self) -> None:
        self.assertIn(WORKFLOW, document_references(live_text("README.md")))
        self.assertTrue((REPO_ROOT / WORKFLOW).is_file())

    def test_task_routes_target_existing_files_and_sections(self) -> None:
        references = document_references(live_text(WORKFLOW))
        for route, (target, heading) in ROUTES.items():
            with self.subTest(route=route):
                self.assertIn(route, references)
                self.assertTrue((REPO_ROOT / target).is_file(), target)
                if heading:
                    self.assertIn(heading, live_text(target).splitlines())

    def test_local_guide_remains_optional_and_ignored(self) -> None:
        ignore_lines = (REPO_ROOT / ".gitignore").read_text().splitlines()
        self.assertIn("/AGENTS.md", ignore_lines)
        text = live_text(WORKFLOW)
        self.assertIn("ignored; never commit it", text)
        self.assertIn("checks work without that file", text)

    def test_local_notes_when_present_remain_local_and_routed(self) -> None:
        if not (REPO_ROOT / "AGENTS.md").is_file():
            self.skipTest("Local ignored instructions are intentionally optional")
        text = live_text("AGENTS.md")
        self.assertIn("local and ignored; never commit it", text)
        self.assertIn(WORKFLOW, document_references(text))

    def test_release_authority_and_known_runbook_gap_remain_explicit(self) -> None:
        self.assertIn("Release, push, tag, upload, and publication require explicit user authorization.", live_text(WORKFLOW))
        release = live_text("docs/RELEASE.md")
        self.assertIn("no maintained mod-site publishing runbook", release)
        self.assertIn("explicit user authorization before publication", release)
        self.assertIn("WORKFLOW.md", document_references(release))

    def test_completion_record_has_each_field_once_with_guidance(self) -> None:
        section = re.search(r"(?ms)^## Completion record\n(.*?)(?=^## |\Z)", live_text(WORKFLOW))
        self.assertIsNotNone(section, "missing Completion record section")
        template = re.search(r"(?ms)^```text\n(.*?)^```[ \t]*$", section[1])
        self.assertIsNotNone(template, "missing completion template")
        fields = re.findall(r"^([A-Za-z]+):([^\n]*)$", template[1], flags=re.MULTILINE)
        for field in ("Step", "Change", "Checks", "Review", "Blockers", "Next"):
            with self.subTest(field=field):
                values = [value.strip() for name, value in fields if name == field]
                self.assertEqual(1, len(values))
                self.assertTrue(values[0])

    def test_entry_guidance_remains_compact(self) -> None:
        for target in (WORKFLOW, "AGENTS.md"):
            if (REPO_ROOT / target).is_file():
                self.assertLessEqual((REPO_ROOT / target).stat().st_size, 8192, target)

    def test_docs_ci_keeps_the_check_unconditional(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/docs.yml").read_text()
        self.assertIn("run: python3 -B scripts/test_workflow_docs.py", {line.strip() for line in workflow.splitlines()})
        self.assertNotRegex(workflow, r"(?m)^[ \t]*(?:if|'if'|\"if\")[ \t]*:")

    def test_link_label_cannot_substitute_for_its_destination(self) -> None:
        references = document_references("[`docs/WORKFLOW.md`](missing.md)")
        self.assertNotIn(WORKFLOW, references)
        self.assertIn("missing.md", references)


if __name__ == "__main__":
    unittest.main()
