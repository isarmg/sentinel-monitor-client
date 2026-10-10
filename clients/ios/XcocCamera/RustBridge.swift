import Foundation
import XcocRust

enum RustBridge {
    static func call(_ handle: UInt64, _ input: [String: Any]) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: input)
        var result = XcscFfiResultV1()
        defer { xcsc_ffi_result_free_v1(&result) }
        let status = data.withUnsafeBytes { bytes in
            xcoc_call_v1(handle, bytes.bindMemory(to: UInt8.self).baseAddress, data.count, &result)
        }
        guard status == 0, result.abi_revision == 1 else { throw NativeError.failed }
        guard let pointer = result.bytes.data, result.bytes.length > 0 else { return ["value": result.value] }
        let output = Data(bytes: pointer, count: result.bytes.length)
        guard let value = try JSONSerialization.jsonObject(with: output) as? [String: Any] else { throw NativeError.failed }
        return value
    }
    static func frame(_ handle: UInt64, _ bytes: Data, timestamp: UInt64) -> Bool {
        var result = XcscFfiResultV1()
        defer { xcsc_ffi_result_free_v1(&result) }
        return bytes.withUnsafeBytes { buffer in
            xcoc_frame_v1(handle, buffer.bindMemory(to: UInt8.self).baseAddress, bytes.count, timestamp, &result) == 0 && result.value == 1
        }
    }
    enum NativeError: Error { case failed }
}
