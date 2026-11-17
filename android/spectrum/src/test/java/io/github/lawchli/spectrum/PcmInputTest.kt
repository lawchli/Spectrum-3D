package io.github.lawchli.spectrum

import org.junit.Assert.*
import org.junit.Test

class PcmInputTest {
    @Test fun stereoIsMixedAndProducerBuffersAreNotRetained() {
        val input = PcmInput()
        val producer = floatArrayOf(.2f, .6f, -.2f, .2f)
        input.push(producer, 2); producer.fill(99f)
        assertArrayEquals(floatArrayOf(.4f, 0f), input.read(0), .000001f)
    }
    @Test fun largeBlocksAreSplitAndBacklogIsBounded() {
        val input = PcmInput(capacity = 2, maxFrames = 2)
        input.push(floatArrayOf(1f, 2f, 3f, 4f, 5f), 1)
        assertEquals(1L, input.droppedBlocks.get())
        assertArrayEquals(floatArrayOf(3f, 4f), input.read(0), 0f)
        assertArrayEquals(floatArrayOf(5f), input.read(0), 0f)
        assertNull(input.read(0))
    }
    @Test fun invalidSamplesCannotPoisonTheFft() {
        val input = PcmInput()
        input.push(floatArrayOf(Float.NaN, .5f, Float.POSITIVE_INFINITY, -.5f), 2)
        assertArrayEquals(floatArrayOf(.25f, -.25f), input.read(0), 0f)
    }
    @Test fun invalidInterleavingIsRejected() {
        try { PcmInput().push(floatArrayOf(1f), 2); fail("Expected invalid channel layout") }
        catch (_: IllegalArgumentException) { }
    }
    @Test fun configurationRejectsInvalidAnalysisOrEmptyDisplay() {
        for (create in listOf<() -> SpectrumConfiguration>(
            { SpectrumConfiguration(rowRate = 61) },
            { SpectrumConfiguration(bandsPerSemitone = 3) },
            { SpectrumConfiguration(seconds = Float.NaN) },
            { SpectrumConfiguration(spectrum = false, waterfall = false) },
        )) {
            try { create(); fail("Expected invalid configuration") }
            catch (_: IllegalArgumentException) { }
        }
    }
    @Test fun concurrentProducersCannotLoseUnaccountedBlocks() {
        val input = PcmInput(capacity = 64, maxFrames = 1)
        val start = java.util.concurrent.CountDownLatch(1)
        val producers = List(2) { producer -> kotlin.concurrent.thread(start = true) {
            start.await()
            repeat(1000) { input.push(floatArrayOf((producer * 1000 + it).toFloat()), 1) }
        } }
        start.countDown()
        producers.forEach { it.join(5000); assertFalse("Producer stalled", it.isAlive) }
        var remaining = 0
        while (input.read(0) != null) remaining++
        assertEquals(64, remaining)
        assertEquals(2000L, remaining + input.droppedBlocks.get())
    }

    @Test fun stoppedPcmCannotRefillTheQueueUntilANewSessionStarts() {
        val input = PcmInput()
        input.push(floatArrayOf(1f), 1)
        input.close()
        assertNull(input.read(0))
        try { input.push(floatArrayOf(2f), 1); fail("Stopped input accepted audio") }
        catch (_: IllegalStateException) { }
        input.reset()
        input.push(floatArrayOf(3f), 1)
        assertArrayEquals(floatArrayOf(3f), input.read(0), 0f)
    }

}
