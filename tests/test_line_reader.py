"""Tests for LineReader.lines() in code.py.

The reader frames JSON lines off usb_cdc.data: complete lines only, until a
newline arrives; a run with no newline over MAX_LINE is dropped so a noisy port
cannot grow the buffer forever. These pin that framing.
"""


class FakePort:
    """Stands in for usb_cdc.data: bytes arrive in arbitrary chunks."""

    def __init__(self):
        self.data = b""
        self.timeout = None

    @property
    def in_waiting(self):
        return len(self.data)

    def read(self, n):
        out, self.data = self.data[:n], self.data[n:]
        return out


def feed(reader, chunk):
    """Push bytes onto the reader's port as if the host sent them."""
    reader.port.data += chunk
    return reader.lines()


def make_reader(tach_code):
    port = FakePort()
    return tach_code.LineReader(port)


def test_sets_port_timeout_zero(tach_code):
    """Non-blocking read is the contract: the tick loop must never stall on USB."""
    port = FakePort()
    tach_code.LineReader(port)
    assert port.timeout == 0


def test_no_bytes_returns_empty(tach_code):
    assert make_reader(tach_code).lines() == []


def test_complete_line_returned_once(tach_code):
    reader = make_reader(tach_code)
    assert feed(reader, b'{"cpu":1.0}\n') == [b'{"cpu":1.0}']
    assert reader.lines() == []  # drained


def test_partial_line_waits_for_newline(tach_code):
    reader = make_reader(tach_code)
    assert feed(reader, b'{"cpu":') == []
    assert feed(reader, b'1.0,"mem":2.0}\n') == [b'{"cpu":1.0,"mem":2.0}']


def test_two_lines_one_chunk_remainder_buffered(tach_code):
    reader = make_reader(tach_code)
    assert feed(reader, b"a\nb\nc") == [b"a", b"b"]  # "c" stays buffered
    assert feed(reader, b"\n") == [b"c"]


def test_empty_lines_skipped(tach_code):
    reader = make_reader(tach_code)
    assert feed(reader, b"\n\na\n\nb\n") == [b"a", b"b"]


def test_oversized_line_dropped(tach_code):
    """A complete line longer than MAX_LINE is skipped, not delivered truncated."""
    reader = make_reader(tach_code)
    assert feed(reader, b"y" * (tach_code.MAX_LINE + 1) + b"\n") == []


def test_overtaken_buffer_resyncs_after_long_run(tach_code):
    """Bytes with no newline past MAX_LINE are dropped; the next newline starts
    a fresh line — the reader recovers without unbounded memory."""
    reader = make_reader(tach_code)
    assert feed(reader, b"x" * (tach_code.MAX_LINE + 50)) == []
    assert feed(reader, b"short\n") == [b"short"]
