package org.sarmg.xcoc

object NativeBridge {
    init { System.loadLibrary("xcoc_mobile_ffi") }
    @JvmStatic external fun call(handle: Long, input: String): String
    @JvmStatic external fun frame(handle: Long, bytes: ByteArray, timestampUs: Long): Boolean
}
