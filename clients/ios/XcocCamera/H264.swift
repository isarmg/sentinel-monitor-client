import Foundation

enum H264 {
    static func annexB(_ data: Data, lengthBytes: Int = 4) throws -> Data {
        guard (1...4).contains(lengthBytes) else { throw InvalidFrame.malformed }
        var output = Data()
        var offset = 0
        while offset < data.count {
            guard offset + lengthBytes <= data.count else { throw InvalidFrame.malformed }
            var length = 0
            for byte in data[offset..<(offset + lengthBytes)] { length = (length << 8) | Int(byte) }
            offset += lengthBytes
            guard length > 0, length <= data.count - offset else { throw InvalidFrame.malformed }
            output.append(contentsOf: [0, 0, 0, 1]); output.append(data[offset..<(offset + length)])
            offset += length
        }
        return output
    }
    enum InvalidFrame: Error { case malformed }
}
