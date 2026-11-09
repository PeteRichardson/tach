"""Tests for tach-sender.py exit codes.

The launchd agent uses KeepAlive: SuccessfulExit=false, which means
launchd only restarts the process if it exits non-zero (crashes).
Configuration errors must exit 0 so the agent does not crash-loop.
"""
import importlib.util
import os
import sys
from unittest import mock


def load_tach_sender():
    """Load tach-sender.py as a module despite its hyphenated filename."""
    # Insert tests/ dir into path[0] so that `import code` doesn't shadow stdlib
    sender_path = os.path.join(os.path.dirname(__file__), "..", "tach-sender.py")
    spec = importlib.util.spec_from_file_location("tach_sender", sender_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_exits_zero_when_macmon_missing():
    """When macmon is not installed, main() should return 0 (clean exit),
    not 1. With KeepAlive: SuccessfulExit=false, a 0 exit tells launchd
    not to restart — preventing an infinite crash-loop."""
    sender = load_tach_sender()
    with mock.patch.object(sender.shutil, "which", return_value=None):
        with mock.patch.object(sender, "log_once") as mock_log:
            with mock.patch.object(sys, "argv", ["tach-sender.py"]):
                exit_code = sender.main()
                assert exit_code == 0, (
                    f"Expected exit code 0 for missing macmon, got {exit_code}. "
                    "A non-zero exit would cause launchd to restart the agent "
                    "in a crash-loop (8600 log lines/day)."
                )
                mock_log.assert_called_once()


def test_exits_zero_when_pyserial_missing():
    """When pyserial is missing but macmon exists, main() should return 0.
    The ImportError from SerialSink construction must lead to a clean exit."""
    sender = load_tach_sender()
    with mock.patch.object(sender.shutil, "which", return_value="/opt/homebrew/bin/macmon"):
        with mock.patch.object(sender, "Memory"):
            with mock.patch.object(sender, "Macmon") as MockMacmon:
                MockMacmon.return_value.start = mock.Mock()
                with mock.patch.object(sender, "log_once"):
                    # SerialSink.__init__ imports serial; make it raise ImportError
                    with mock.patch.object(sender, "SerialSink", side_effect=ImportError("No module 'serial'")):
                        with mock.patch.object(sys, "argv", ["tach-sender.py"]):
                            exit_code = sender.main()
                            assert exit_code == 0, (
                                f"Expected exit code 0 for missing pyserial, got {exit_code}. "
                                "A non-zero exit would cause launchd to restart the agent "
                                "in a crash-loop."
                            )