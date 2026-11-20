"""Host tests for quoted error messages in the docs (seam: README text vs
tach-sender.py source).

Issue #3 / finding F3: the troubleshooting table told the user to look for
`found only one Adafruit port`. Commit 669d2f2 deleted that string from
tach-sender.py and printed a different one, and the table kept quoting the dead
text. Nothing linked the two, so the doc could not disagree with the code.

A troubleshooting row whose Problem cell is one backticked literal is a claim
that the program prints that exact text. These tests check that claim against
tach-sender.py.

Scope: only the Troubleshooting section of pyportal/README.md is checked,
against tach-sender.py only. Prose rows ("The numbers are grey.", "NO FONT at
the top of the screen") are not literals and are never checked; the file-name
and command cells in the other tables are outside the section, so they are out
of scope too; and the v2 device's messages live in device/, not here.
"""
import os
import re

REPO_ROOT = os.path.join(os.path.dirname(__file__), "..")
SENDER_PATH = os.path.join(REPO_ROOT, "tach-sender.py")
PYPORTAL_README = os.path.join(REPO_ROOT, "pyportal", "README.md")

# The string the docs quoted after the code stopped printing it (issue #3).
DELETED_MESSAGE = "found only one Adafruit port"

# The only section these tests read. Other tables hold file names and commands.
SECTION = "Troubleshooting"

_HEADING = re.compile(r"^##\s+(.*?)\s*$")


def quoted_literals(doc_text):
    """Every quoted literal in the troubleshooting table, in document order.

    A literal is a row whose first cell holds nothing but one backtick span;
    every other row is prose and is left alone.
    """
    literals = []
    in_section = False
    for line in doc_text.splitlines():
        heading = _HEADING.match(line)
        if heading:
            in_section = heading.group(1) == SECTION
            continue
        if not in_section or not line.startswith("|"):
            continue
        first = line.strip().strip("|").split("|")[0].strip()
        if len(first) >= 2 and first.startswith("`") and first.endswith("`"):
            literals.append(first[1:-1])
    return literals


def dead_messages(doc_text, code_text):
    """The quoted literals that code_text never prints."""
    return [msg for msg in quoted_literals(doc_text) if msg not in code_text]


def test_quoted_literal_checker_flags_a_deleted_string():
    # Guards the checker itself: a checker that always returned nothing would
    # pass the real-doc test for the wrong reason.
    doc = (
        "## Troubleshooting\n"
        "\n"
        "| Problem | What to do |\n"
        "|---|---|\n"
        f"| `{DELETED_MESSAGE}` | Check the cable. |\n"
        "| The numbers are grey. | Make sure the sender runs. |\n"
    )
    code = 'log_once("no Adafruit board has a data port: is boot.py on the device?")\n'

    assert dead_messages(doc, code) == [DELETED_MESSAGE]


def test_pyportal_readme_quotes_only_messages_the_sender_prints():
    # The regression test for issue #3: this failed while the table still
    # quoted the string 669d2f2 removed.
    with open(PYPORTAL_README) as f:
        doc = f.read()
    with open(SENDER_PATH) as f:
        code = f.read()

    # A table with no quoted rows would pass the check vacuously, so the rows
    # found are pinned alongside the result.
    assert quoted_literals(doc) != [], "no quoted message rows found: the check went vacuous"
    dead = dead_messages(doc, code)
    assert dead == [], f"pyportal/README.md quotes messages tach-sender.py never prints: {dead}"
