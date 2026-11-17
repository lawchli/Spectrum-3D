package io.github.lawchli.spectrum

import org.junit.Assert.assertEquals
import org.junit.Test

/** The same cases as the desktop monitor's tests (rust/spectrum-wgpu/src/monitor.rs). */
class MonitorTest {
    @Test fun columnsTakeTheFewestRowsThatFit() {
        val five = listOf(240f, 160f, 120f, 200f, 230f)
        assertEquals(listOf(5), pack(five, 1050f, 25f))
        assertEquals(listOf(3, 2), pack(five, 1049f, 25f))
        assertEquals(listOf(3, 2), pack(five, 570f, 25f))
        assertEquals(listOf(2, 2, 1), pack(five, 569f, 25f))
        assertEquals(listOf(1, 1, 1, 1, 1), pack(five, 100f, 25f))
        assertEquals(emptyList<Int>(), pack(emptyList(), 400f, 25f))
    }

    @Test fun aRowFillsItsWidthByWeight() {
        assertEquals(listOf(200f, 125f, 125f), spread(listOf(100f, 100f, 100f), listOf(4f, 1f, 1f), 500f, 25f))
        assertEquals(listOf(100f, 100f), spread(listOf(100f, 100f), listOf(1f, 1f), 150f, 25f))
    }

    @Test fun theMemoryScaleStepsAndHolds() {
        assertEquals(1.0, niceCeiling(0.0), 0.0)
        assertEquals(1.0, niceCeiling(Double.NaN), 0.0)
        assertEquals(300.0, niceCeiling(260.0), 1e-9)
        assertEquals(1000.0, niceCeiling(810.0), 1e-9)
        val history = MemoryHistory()
        for (second in 0 until 60) history.record(second.toDouble(), 236L * 1048576, null)
        assertEquals(300.0, history.top(), 1e-9)
        history.record(60.0, 290L * 1048576, null)
        assertEquals(400.0, history.top(), 1e-9)
        for (second in 61..180) history.record(second.toDouble(), 236L * 1048576, null)
        assertEquals(400.0, history.top(), 1e-9)
        history.record(181.0, 236L * 1048576, null)
        assertEquals(300.0, history.top(), 1e-9)
        assertEquals(61.0, history.samples.first().first, 0.0)
        // A failed reading keeps no point and says so.
        history.record(182.0, null, null)
        assertEquals(null, history.resident)
        assertEquals(181.0, history.samples.last().first, 0.0)
    }
}
