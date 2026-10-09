package org.sarmg.xcoc
import org.junit.Assert.*
import org.junit.Test

class H264Test {
    @Test fun convertsLengthPrefixedUnitsAndPreservesAnnexB() {
        val avcc = byteArrayOf(0,0,0,2,0x67,1,0,0,0,2,0x68,2)
        val annex = byteArrayOf(0,0,0,1,0x67,1,0,0,0,1,0x68,2)
        assertArrayEquals(annex, H264.annexB(avcc))
        assertArrayEquals(annex, H264.annexB(annex))
        assertEquals(2, H264.units(annex).size)
    }
    @Test(expected = IllegalArgumentException::class) fun rejectsTruncatedAvcc() { H264.annexB(byteArrayOf(0,0,0,8,0x65)) }
}
