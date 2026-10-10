#!/usr/bin/env python3
"""Read-only structural guards for iOS lifecycle ordering, without camera access.

The XCTest suite exercises the generation gates with deterministic queue barriers.
These checks tie those gates to production call sites and protect the two queue
ordering regressions even on CI hosts without an Apple SDK.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1] / "clients/ios/XcocCamera"


def block(source, marker):
    start = source.index("{", source.index(marker))
    depth = 1
    for end in range(start + 1, len(source)):
        depth += (source[end] == "{") - (source[end] == "}")
        if depth == 0:
            return source[start + 1:end]
    raise AssertionError(f"unclosed source block: {marker}")


class LifecycleSourceTests(unittest.TestCase):
    def test_store_stops_capture_without_waiting_for_network_control(self):
        source = (ROOT / "CameraStore.swift").read_text()
        stop = block(source, "func stop()")
        self.assertLess(stop.index("lifecycle.cancel()"), stop.index("control.async"))
        controlled = block(stop, "control.async")
        self.assertNotIn("encoder.stop", controlled)
        self.assertIn('"operation": "close"', block(stop, "encoder.stop"))
        self.assertIn("encoder.start(front: front, owner: self.lifecycle, generation: generation)", source)

    def test_pending_open_is_gated_and_cancelled_handle_is_closed(self):
        source = (ROOT / "CameraStore.swift").read_text()
        start = block(source, "func start(front:")
        self.assertGreaterEqual(start.count("lifecycle.isCurrent(generation)"), 3)
        after_open = start[start.index('"operation": "open"'):]
        gate = after_open.index("guard self.lifecycle.isCurrent(generation)")
        close = after_open.index('"operation": "close"')
        capture = after_open.index("self.encoder.start")
        self.assertLess(gate, close)
        self.assertLess(close, capture)
        self.assertIn("poll(active: true, generation: generation)", start)

    def test_encoder_start_cannot_escape_its_serial_queue_operation(self):
        source = (ROOT / "CameraEncoder.swift").read_text()
        start = block(source, "func start(front:")
        self.assertEqual(start.count("queue.async"), 1)
        self.assertEqual(start.count("lifecycle.isCurrent(captureGeneration)"), 3)
        self.assertEqual(start.count("owner.isCurrent(generation)"), 3)
        self.assertLess(start.index("configureCapture(front:"), start.index("session.startRunning()"))
        configured = block(source, "private func configureCapture(front:")
        self.assertIn("defer { session.commitConfiguration() }", configured)
        self.assertNotIn("startRunning", configured)
        stop = block(source, "func stop(completion:")
        self.assertLess(stop.index("lifecycle.cancel()"), stop.index("queue.async"))
        self.assertLess(stop.index("session.stopRunning()"), stop.index("completion?()"))


if __name__ == "__main__":
    unittest.main()
