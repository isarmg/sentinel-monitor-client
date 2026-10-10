import Foundation

/// A queued start may outlive the user's stop request. Each asynchronous layer
/// checks its own generation before opening or starting capture resources.
final class CaptureGeneration: @unchecked Sendable {
    private let lock = NSLock()
    private var current: UUID?

    func begin() -> UUID {
        lock.lock(); defer { lock.unlock() }
        let generation = UUID()
        current = generation
        return generation
    }

    func cancel() {
        lock.lock(); defer { lock.unlock() }
        current = nil
    }

    func isCurrent(_ generation: UUID) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return current == generation
    }
}
