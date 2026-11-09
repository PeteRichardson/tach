"""Tests for find_data_port() in tach-sender.py.

Covers two bugs:
- #6 (F6): Grouping by serial_number or device fails when serial_number is None,
  because each port has a unique device path. Two ports from the same board end
  up in separate groups, so len(ports) >= 2 is never true.
- #7 (F7): The interface-name check is dead code on macOS — pyserial never
  populates p.interface — so the numeric heuristic is the only path that works.
"""
import importlib.util
import os
from unittest import mock


def load_tach_sender():
    """Load tach-sender.py as a module despite its hyphenated filename."""
    sender_path = os.path.join(os.path.dirname(__file__), "..", "tach-sender.py")
    spec = importlib.util.spec_from_file_location("tach_sender", sender_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FakePort:
    """Minimal stand-in for serial.tools.list_ports ListPortInfo."""

    def __init__(self, device, vid=None, pid=None, serial_number=None,
                 interface=None, location=None, description=""):
        self.device = device
        self.name = os.path.basename(device)
        self.description = description
        self.hwid = ""
        self.vid = vid
        self.pid = pid
        self.serial_number = serial_number
        self.location = location
        self.manufacturer = None
        self.product = None
        self.interface = interface


def test_groups_by_location_when_serial_is_none():
    """Two Adafruit ports with serial_number=None but the same location
    must be grouped as one board, so the numeric heuristic can pick the data port.
    """
    sender = load_tach_sender()

    ports = [
        FakePort("/dev/cu.usbmodem14101", vid=0x239A, pid=0x0000,
                 serial_number=None, location="usb12345678",
                 description="PyPortal"),
        FakePort("/dev/cu.usbmodem14102", vid=0x239A, pid=0x0000,
                 serial_number=None, location="usb12345678",
                 description="PyPortal"),
    ]

    with mock.patch.object(sender, "log_once"):
        with mock.patch("serial.tools.list_ports.comports", return_value=ports):
            result = sender.find_data_port()

    assert result == "/dev/cu.usbmodem14102", (
        "Expected the higher-numbered device (data port), got {}".format(result)
    )


def test_returns_none_when_single_port():
    """A board with only one port (no data channel) returns None."""
    sender = load_tach_sender()

    ports = [
        FakePort("/dev/cu.usbmodem14101", vid=0x239A, pid=0x0000,
                 serial_number="ABC123", location="usb12345678",
                 description="PyPortal"),
    ]

    with mock.patch.object(sender, "log_once"):
        with mock.patch("serial.tools.list_ports.comports", return_value=ports):
            result = sender.find_data_port()

    assert result is None


def test_prefers_pyportal():
    """When multiple boards are present, prefer one whose description
    mentions 'pyportal'.
    """
    sender = load_tach_sender()

    ports = [
        FakePort("/dev/cu.usbmodem14101", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc1",
                 description="Feather"),
        FakePort("/dev/cu.usbmodem14102", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc1",
                 description="Feather"),
        FakePort("/dev/cu.usbmodem14301", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc2",
                 description="PyPortal"),
        FakePort("/dev/cu.usbmodem14302", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc2",
                 description="PyPortal"),
    ]

    with mock.patch.object(sender, "log_once"):
        with mock.patch("serial.tools.list_ports.comports", return_value=ports):
            result = sender.find_data_port()

    assert result == "/dev/cu.usbmodem14302"


def test_different_locations_are_different_boards():
    """Two boards with the same VID/PID but different locations and
    no serial numbers must be treated as separate boards.
    """
    sender = load_tach_sender()

    ports = [
        FakePort("/dev/cu.usbmodem14101", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc1",
                 description="PyPortal"),
        FakePort("/dev/cu.usbmodem14102", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc1",
                 description="PyPortal"),
        # Second board, different location
        FakePort("/dev/cu.usbmodem14301", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc2",
                 description="PyPortal"),
        FakePort("/dev/cu.usbmodem14302", vid=0x239A, pid=0x0000,
                 serial_number=None, location="loc2",
                 description="PyPortal"),
    ]

    with mock.patch.object(sender, "log_once"):
        with mock.patch("serial.tools.list_ports.comports", return_value=ports):
            result = sender.find_data_port()

    # Both are PyPortals; either data port is acceptable, but it should
    # pick from one of the two-port groups
    assert result in ("/dev/cu.usbmodem14102", "/dev/cu.usbmodem14302")


def test_groups_by_serial_when_present():
    """When serial_number is available, group by it (original behavior)."""
    sender = load_tach_sender()

    ports = [
        FakePort("/dev/cu.usbmodem14101", vid=0x239A, pid=0x0000,
                 serial_number="BOARD1", location="usb1",
                 description="PyPortal"),
        FakePort("/dev/cu.usbmodem14102", vid=0x239A, pid=0x0000,
                 serial_number="BOARD1", location="usb1",
                 description="PyPortal"),
    ]

    with mock.patch.object(sender, "log_once"):
        with mock.patch("serial.tools.list_ports.comports", return_value=ports):
            result = sender.find_data_port()

    assert result == "/dev/cu.usbmodem14102"


def test_interface_field_ignored():
    """The interface field is never populated on macOS and should not be
    relied upon. Even if it somehow contained 'data', the numeric heuristic
    is the primary path.
    """
    sender = load_tach_sender()

    # Port with interface set to something — should still work via numeric heuristic
    ports = [
        FakePort("/dev/cu.usbmodem14101", vid=0x239A, pid=0x0000,
                 serial_number=None, location="usb12345678",
                 interface="CircuitPython CDC data",
                 description="PyPortal"),
        FakePort("/dev/cu.usbmodem14102", vid=0x239A, pid=0x0000,
                 serial_number=None, location="usb12345678",
                 interface="CircuitPython CDC data",
                 description="PyPortal"),
    ]

    with mock.patch.object(sender, "log_once"):
        with mock.patch("serial.tools.list_ports.comports", return_value=ports):
            result = sender.find_data_port()

    # Should use the numeric heuristic (higher number = data port)
    assert result == "/dev/cu.usbmodem14102"