import Foundation
import XCTest
@testable import XcocCamera

final class CaptureGenerationTests: XCTestCase {
    private final class EventLog: @unchecked Sendable {
        private let lock = NSLock()
        private var events = [String]()
        func append(_ event: String) {
            lock.lock(); defer { lock.unlock() }
            events.append(event)
        }
        func snapshot() -> [String] {
            lock.lock(); defer { lock.unlock() }
            return events
        }
    }

    func testStopWhileOpenIsPendingClosesInsteadOfStarting() {
        let lifecycle = CaptureGeneration()
        let generation = lifecycle.begin()
        let control = DispatchQueue(label: "fixture.control")
        let opened = DispatchSemaphore(value: 0)
        let releaseOpen = DispatchSemaphore(value: 0)
        let finished = expectation(description: "stopped")
        let events = EventLog()
        control.async {
            events.append("open")
            opened.signal()
            releaseOpen.wait()
            if lifecycle.isCurrent(generation) {
                events.append("start")
            } else {
                events.append("close")
            }
        }
        XCTAssertEqual(opened.wait(timeout: .now() + 1), .success)
        lifecycle.cancel()
        control.async { events.append("stop"); finished.fulfill() }
        releaseOpen.signal()
        wait(for: [finished], timeout: 2)
        XCTAssertEqual(events.snapshot(), ["open", "close", "stop"])
    }

    func testStopDuringConfigurationCannotQueueALaterPhysicalStart() {
        let lifecycle = CaptureGeneration()
        let generation = lifecycle.begin()
        let capture = DispatchQueue(label: "fixture.capture")
        let configured = DispatchSemaphore(value: 0)
        let releaseConfiguration = DispatchSemaphore(value: 0)
        let finished = expectation(description: "capture stopped")
        let events = EventLog()
        capture.async {
            events.append("configure")
            configured.signal()
            releaseConfiguration.wait()
            events.append("commit")
            if lifecycle.isCurrent(generation) { events.append("startRunning") }
        }
        XCTAssertEqual(configured.wait(timeout: .now() + 1), .success)
        lifecycle.cancel()
        capture.async { events.append("stopRunning"); finished.fulfill() }
        releaseConfiguration.signal()
        wait(for: [finished], timeout: 2)
        XCTAssertEqual(events.snapshot(), ["configure", "commit", "stopRunning"])
    }

    func testRestartUsesANewGenerationAndOldCallbacksStayCancelled() {
        let lifecycle = CaptureGeneration()
        let old = lifecycle.begin()
        XCTAssertTrue(lifecycle.isCurrent(old))
        lifecycle.cancel()
        XCTAssertFalse(lifecycle.isCurrent(old))
        let restarted = lifecycle.begin()
        XCTAssertFalse(lifecycle.isCurrent(old))
        XCTAssertTrue(lifecycle.isCurrent(restarted))
        lifecycle.cancel()
        XCTAssertFalse(lifecycle.isCurrent(restarted))
    }

    func testCaptureStopDoesNotWaitForBlockedNetworkPoll() {
        let owner = CaptureGeneration()
        let generation = owner.begin()
        let control = DispatchQueue(label: "fixture.blocked-control")
        let capture = DispatchQueue(label: "fixture.unblocked-capture")
        let polling = DispatchSemaphore(value: 0)
        let releasePoll = DispatchSemaphore(value: 0)
        let stopped = expectation(description: "physical capture stopped")
        let closed = expectation(description: "native handle closed")
        let events = EventLog()
        control.async {
            polling.signal()
            releasePoll.wait()
            events.append("pollFinished")
        }
        XCTAssertEqual(polling.wait(timeout: .now() + 1), .success)
        owner.cancel()
        capture.async { events.append("stopRunning"); stopped.fulfill() }
        control.async { events.append("close"); closed.fulfill() }
        wait(for: [stopped], timeout: 1)
        XCTAssertEqual(events.snapshot(), ["stopRunning"])
        XCTAssertFalse(owner.isCurrent(generation))
        releasePoll.signal()
        wait(for: [closed], timeout: 1)
        XCTAssertEqual(events.snapshot(), ["stopRunning", "pollFinished", "close"])
    }

    func testLateEncoderHandoffCannotReplaceCancelledUserGeneration() {
        let owner = CaptureGeneration()
        let generation = owner.begin()
        owner.cancel()
        // A control task can reach encoder.start after stop already ran. Its
        // new encoder-local generation must not revive the old user request.
        let encoder = CaptureGeneration()
        let captureGeneration = encoder.begin()
        XCTAssertTrue(encoder.isCurrent(captureGeneration))
        XCTAssertFalse(owner.isCurrent(generation))
        XCTAssertFalse(encoder.isCurrent(captureGeneration) && owner.isCurrent(generation))
    }
}
