"""Host fixtures for tach-sender.py, the Mac side of Tach.

tach-sender.py cannot be imported by name because its filename has a hyphen, so
the tach_sender fixture loads it through importlib under the module name
tach_sender. That name cannot shadow a stdlib module, which is the same reason
the v1 loader that used to sit here named its module tach_code.

v1 is retired: the CircuitPython code, its design.md and the fixtures that faked
the PyPortal's device modules are gone (docs/device-v2.md, Q40). The firmware is
Rust in device/, and its tests run under cargo in device/host-tests/.
"""
import importlib.util
import json
import os

import pytest


@pytest.fixture(scope="session")
def tach_sender():
    """tach-sender.py as a module, despite its hyphenated filename."""
    path = os.path.join(os.path.dirname(__file__), "..", "tach-sender.py")
    spec = importlib.util.spec_from_file_location("tach_sender", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="session")
def macmon_lines():
    """Parsed dicts from tests/fixtures/macmon_pipe.jsonl (recorded `macmon pipe`)."""
    path = os.path.join(os.path.dirname(__file__), "fixtures", "macmon_pipe.jsonl")
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]
