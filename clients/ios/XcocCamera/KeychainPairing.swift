import Foundation
import Security

enum KeychainPairing {
    private static func query(_ account: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "org.sarmg.xcoc", kSecAttrAccount as String: account]
    }
    static func read(_ account: String) throws -> Data? {
        var request = query(account)
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &item)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = item as? Data else { throw RustBridge.NativeError.failed }
        return data
    }
    static func save(_ account: String, data: Data) throws {
        var attributes: [String: Any] = [kSecValueData as String: data, kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly]
        let status = SecItemUpdate(query(account) as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            attributes.merge(query(account)) { old, _ in old }
            guard SecItemAdd(attributes as CFDictionary, nil) == errSecSuccess else { throw RustBridge.NativeError.failed }
        } else if status != errSecSuccess { throw RustBridge.NativeError.failed }
    }
    static func installationId() throws -> String {
        if let data = try read("installation"), let id = String(data: data, encoding: .utf8) { return id }
        let id = UUID().uuidString.lowercased()
        try save("installation", data: Data(id.utf8)); return id
    }
}
