package io.github.lawchli.spectrum
import org.junit.Test
import org.junit.Assert.*
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.*
class AnalysisTest {
    @Test fun matchesJavascriptFixtures() {
        for (name in listOf("tone", "sweep", "noise", "chord", "pad")) {
            val raw = javaClass.classLoader!!.getResourceAsStream("$name.svf")!!.readBytes()
            val b = ByteBuffer.wrap(raw).order(ByteOrder.LITTLE_ENDIAN)
            assertEquals(0x31465653, b.int); val sr = b.int; val length = b.int; val cases = b.int
            val pcm = FloatArray(length) { b.float }
            repeat(cases) {
                val rate = b.int; val sub = b.int; val rows = b.int; val count = b.int
                val a = Analysis(sr, rate, sub)
                repeat(rows) { row ->
                    val end = (row + 1) * sr / rate
                    a.receive(pcm, end - a.written.toInt(), a.written.toInt()); val db = a.analyse(end.toLong())
                    for (k in 0 until count) {
                        val expected = b.get().toInt() and 255; val actual = dbToByte(db[k].toDouble())
                        assertTrue("$name $rate x $sub row $row band $k: $actual / $expected", abs(actual - expected) <= 1)
                    }
                }
                if (name == "tone") assertTrue(abs(1200 * log2(a.peakHz(57.25)!! / (440 * 2.0.pow(25.0 / 1200)))) < 1)
            }; assertEquals(raw.size, b.position())
        }
    }
    @Test fun realFftMatchesDirectDft() {
        val n = 64; val ring = FloatArray(128) { (sin(it * .23) * .3 + cos(it * 1.3) * .2).toFloat() }
        val out = FloatArray(n / 2); RealFFT(n).run(ring, 103, out, n / 2)
        for (k in 0 until n / 2) {
            var re = 0.0; var im = 0.0
            for (i in 0 until n) {
                val w = .42 - .5 * cos(2 * PI * i / n) + .08 * cos(4 * PI * i / n); val v = ring[(103 - n + i) and 127] * w
                re += v * cos(2 * PI * k * i / n); im -= v * sin(2 * PI * k * i / n)
            }
            val expected = 10 * log10((re * re + im * im) * (2 / (n * .42)).pow(2) + 1e-30)
            assertEquals("bin $k", expected, out[k].toDouble(), 1e-4)
        }
    }
    @Test fun skippedRowsRepeatAndLowAnalyserStillRuns() {
        // 480 rows/s: the 8192-point analyser is due every 5 rows. Refreshing only rows 1, 6, 11 ... never
        // lands on rowIndex % 5 == 0, which a modulo schedule would turn into a frozen bass region.
        val sr = 48000; val a = Analysis(sr, 480, 1); val hop = sr / 480; val pcm = FloatArray(hop)
        var phase = 0.0; var previous = FloatArray(0); var bass = -120f
        for (row in 0 until 480) {
            for (i in 0 until hop) { phase += 2 * PI * 110 / sr; pcm[i] = (.5 * sin(phase)).toFloat() }
            a.receive(pcm)
            val refresh = row % 5 == 1
            val db = a.analyse(a.written, refresh)
            if (!refresh && row > 0) assertArrayEquals(previous, db, 0f)
            previous = db.clone(); bass = max(bass, db[33])  // A2, 110 Hz (band = MIDI - 12), below the 350 Hz split
        }
        assertTrue("bass band stayed at $bass dB", bass > -30)
    }
    @Test fun historyKeepsPowerOfImpulseAcrossWrap() {
        val h = History(1, 8); repeat(160) { h.push(byteArrayOf(if (it % 2 == 0) 240.toByte() else 0)) }
        assertTrue(h.levels[1].all { (it.toInt() and 255) == 234 })
        assertEquals(-120.0, h.sample(0.0, 0, 1), .0); assertEquals(0.0, h.sample(0.0, 1, 1), .0)
    }
    @Test fun externalPcmUsesItsDeclaredSampleRate() {
        for (sampleRate in listOf(16000, 44100, 96000)) {
            val analysis = Analysis(sampleRate, 120, 2)
            val pcm = FloatArray(sampleRate) { (.5 * sin(2 * PI * 440 * it / sampleRate)).toFloat() }
            repeat(120) { row ->
                val end = ((row + 1).toLong() * sampleRate / 120).toInt()
                analysis.receive(pcm, end - analysis.written.toInt(), analysis.written.toInt())
                analysis.analyse(end.toLong())
            }
            assertEquals(sampleRate.toLong(), analysis.written)
            assertEquals(120L, analysis.rowIndex)
            assertEquals("$sampleRate Hz input", 440.0, analysis.peakHz(57.0)!!, 1.5)
            assertTrue(analysis.bands[57 * 2] > -12f)
        }
    }

}
