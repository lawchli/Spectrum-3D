package io.github.lawchli.spectrum

import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlin.math.min

/** Bounded, copying producer/consumer handoff. Kept separate from Android APIs for deterministic QA. */
internal class PcmInput(capacity: Int = 16, private val maxFrames: Int = 1024) {
    private val queue = ArrayBlockingQueue<FloatArray>(capacity)
    val droppedBlocks = AtomicLong(0)
    private val handoff = Any()
    private var generation = 0L
    private var accepting = true
    fun clear() = synchronized(handoff) { generation++; queue.clear() }
    fun close() = synchronized(handoff) { accepting = false; generation++; queue.clear() }
    fun reset() = synchronized(handoff) { accepting = true; generation++; queue.clear(); droppedBlocks.set(0) }
    fun read(timeoutMs: Long = 100): FloatArray? = queue.poll(timeoutMs, TimeUnit.MILLISECONDS)
    fun push(pcm: FloatArray, channels: Int) {
        require(channels > 0 && pcm.size % channels == 0)
        val session = synchronized(handoff) { check(accepting) { "PCM input is stopped" }; generation }
        val frames = pcm.size / channels
        var offset = 0
        while (offset < frames) {
            val count = min(maxFrames, frames - offset)
            val mono = FloatArray(count) { frame ->
                var value = 0.0
                for (channel in 0 until channels) {
                    val sample = pcm[(offset + frame) * channels + channel]
                    if (sample.isFinite()) value += sample
                }
                (value / channels).toFloat()
            }
            synchronized(handoff) {
                // Stop/reconfigure invalidates an in-flight large push before it can reach a new session.
                if (!accepting || session != generation) return
                if (!queue.offer(mono)) {
                    if (queue.poll() != null) droppedBlocks.incrementAndGet()
                    queue.offer(mono)
                }
            }
            offset += count
        }
    }
}
