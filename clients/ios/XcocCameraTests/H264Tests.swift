import XCTest
@testable import XcocCamera

final class H264Tests: XCTestCase {
    func testLengthPrefixedAccessUnit() throws {
        XCTAssertEqual(try H264.annexB(Data([0,0,0,2,0x67,1,0,0,0,2,0x68,2])), Data([0,0,0,1,0x67,1,0,0,0,1,0x68,2]))
        XCTAssertThrowsError(try H264.annexB(Data([0,0,0,8,0x65])))
    }
}
