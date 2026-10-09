package org.sarmg.xcoc
import org.junit.Assert.*
import org.junit.Test

class NativeBridgeTest {
    @Test fun hostJniOpensValidStateRejectsBadFramesAndInvalidatesClosedHandles() {
        val pairing = """{"server":"https://example.invalid","installation_id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","instance_id":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","access_token":"${"a".repeat(43)}","name":"fixture"}"""
        val opened = NativeBridge.call(0, """{"operation":"open","pairing":$pairing}""")
        val handle = Regex(""""value"\s*:\s*(\d+)""").find(opened)!!.groupValues[1].toLong()
        assertTrue(handle != 0L)
        try {
            assertThrows(IllegalArgumentException::class.java) { NativeBridge.frame(handle, byteArrayOf(0x65), 0) }
            assertFalse(NativeBridge.frame(handle, byteArrayOf(0,0,0,1,0x65,1), 1000))
            NativeBridge.call(handle, """{"operation":"stop"}""")
        } finally { NativeBridge.call(handle, """{"operation":"close"}""") }
        assertThrows(IllegalStateException::class.java) { NativeBridge.call(handle, """{"operation":"stop"}""") }
        assertThrows(IllegalArgumentException::class.java) { NativeBridge.call(0, "x".repeat(65537)) }
    }
}
