package org.sarmg.xcoc

/** MediaCodec normally emits Annex B; accept length-prefixed access units too. */
object H264 {
    fun annexB(bytes: ByteArray): ByteArray {
        if (bytes.size >= 4 && bytes[0] == 0.toByte() && bytes[1] == 0.toByte() &&
            (bytes[2] == 1.toByte() || (bytes[2] == 0.toByte() && bytes[3] == 1.toByte()))) return bytes
        val output = java.io.ByteArrayOutputStream()
        var offset = 0
        while (offset < bytes.size) {
            require(offset + 4 <= bytes.size)
            val length = java.nio.ByteBuffer.wrap(bytes, offset, 4).int
            offset += 4
            require(length > 0 && length <= bytes.size - offset)
            output.write(byteArrayOf(0, 0, 0, 1)); output.write(bytes, offset, length)
            offset += length
        }
        return output.toByteArray()
    }
    fun units(bytes: ByteArray): List<ByteArray> {
        val frame = annexB(bytes)
        val starts = mutableListOf<Pair<Int, Int>>()
        var index = 0
        while (index + 3 <= frame.size) {
            val size = if (index + 4 <= frame.size && frame.sliceArray(index until index + 4).contentEquals(byteArrayOf(0, 0, 0, 1))) 4
                else if (frame.sliceArray(index until index + 3).contentEquals(byteArrayOf(0, 0, 1))) 3 else 0
            if (size > 0) { starts += index to index + size; index += size } else index++
        }
        return starts.mapIndexed { i, start -> frame.copyOfRange(start.second, starts.getOrNull(i + 1)?.first ?: frame.size) }.filter { it.isNotEmpty() }
    }
}
