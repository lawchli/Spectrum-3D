package io.github.lawchli.spectrum

import android.system.Os
import android.system.OsConstants
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.io.File
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.pow
import kotlin.math.roundToInt

// The demo's monitor, laid out by the rules of the web and desktop monitors
// (web/src/monitor.js, rust/spectrum-wgpu/src/monitor.rs): the groups in as few rows as the
// width allows, each row filling the width, and resident memory over the last two minutes.

private val INK_1 = Color(0xffeaedf2)
private val INK_2 = Color(0xff8e97a4)
private val INK_3 = Color(0xff545d6a)
private val BG_WELL = Color(0xff07080a)
private val BORDER_SUBTLE = Color(0xff1b1f27)

const val MEMORY_WINDOW_S = 120.0
const val MEMORY_GAP_S = 2.5

/** One group: a title, a note beside it, the headline figure, then label and value rows. */
data class MonitorGroup(
    val part: String,
    val title: String,
    val note: String,
    val value: String,
    val rows: List<Pair<String, String>>,
)

/** Resident memory now and its peak, in bytes, from /proc (what the desktop app reads on Linux). */
fun readResident(): Pair<Long?, Long?> {
    val now = runCatching {
        val pages = File("/proc/self/statm").readText().trim().split(Regex("\\s+"))[1].toLong()
        pages * Os.sysconf(OsConstants._SC_PAGESIZE)
    }.getOrNull()
    val peak = runCatching {
        File("/proc/self/status").readLines().first { it.startsWith("VmHWM:") }
            .split(Regex("\\s+"))[1].toLong() * 1024
    }.getOrNull()
    return now to peak
}

/** Readings over the last two minutes: seconds and MB, oldest first. */
class MemoryHistory {
    val samples = ArrayDeque<Pair<Double, Double>>()
    var resident: Long? = null
    var peak: Long? = null
    fun record(at: Double, resident: Long?, peak: Long?) {
        this.resident = resident; this.peak = peak
        if (resident == null) return
        samples.addLast(at to resident / 1048576.0)
        while (samples.first().first < at - MEMORY_WINDOW_S) samples.removeFirst()
    }
    fun clear() { samples.clear(); resident = null; peak = null }
    /** A tenth above the highest reading in the window, rounded up to a step. */
    fun top() = niceCeiling((samples.maxOfOrNull { it.second } ?: 0.0) * 1.1)
}

/** The smallest of 1, 1.5, 2, 3, 4, 5, 6, 8 times a power of ten at or above [value]. */
fun niceCeiling(value: Double): Double {
    if (!(value > 1)) return 1.0
    var power = 10.0.pow(floor(log10(value)))
    while (true) {
        for (step in doubleArrayOf(1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0)) {
            if (step * power >= value * (1 - 1e-9)) return step * power
        }
        power *= 10
    }
}

/** How many of [widths] go in each row: the fewest rows that fit, earlier rows taking the extra one. */
fun pack(widths: List<Float>, room: Float, gap: Float): List<Int> {
    val n = widths.size
    for (rows in 1..n) {
        val counts = List(rows) { r -> n / rows + if (r < n % rows) 1 else 0 }
        var start = 0
        val fits = counts.all { count ->
            val row = widths.subList(start, start + count); start += count
            row.sum() + gap * (count - 1) <= room
        }
        if (fits) return counts
    }
    return List(n) { 1 }
}

/** One row's widths filling [room]: each its least, the rest shared by weight. */
fun spread(least: List<Float>, weights: List<Float>, room: Float, gap: Float): List<Float> {
    val spare = room - least.sum() - gap * (least.size - 1).coerceAtLeast(0)
    val total = weights.sum()
    return least.mapIndexed { i, l -> l + if (spare > 0 && total > 0) spare * weights[i] / total else 0f }
}

private class Measured(val text: Float, val least: Float, val lines: Int)

@Composable
fun MonitorDock(groups: List<MonitorGroup>, memory: MemoryHistory, now: Double, modifier: Modifier = Modifier) {
    val measurer = rememberTextMeasurer()
    val sans = TextStyle(fontSize = 10.sp, color = INK_3)
    val mono = TextStyle(fontSize = 10.sp, fontFamily = FontFamily.Monospace, color = INK_2)
    val headline = mono.copy(color = INK_1)
    val frameHeadline = headline.copy(fontSize = 16.sp)
    // Each group keeps the widest its text has been, so a reading gaining a digit does not
    // tip the dock between one row and two.
    val widest = remember { mutableMapOf<String, Float>() }
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val density = LocalDensity.current
        val line = with(density) { 14.dp.toPx() }
        val gap = with(density) { 12.dp.toPx() }
        val chartMin = with(density) { 96.dp.toPx() }
        val room = with(density) { maxWidth.toPx() }
        fun width(text: String, style: TextStyle) =
            if (text.isEmpty()) 0f else measurer.measure(text, style, maxLines = 1).size.width.toFloat()
        fun head(g: MonitorGroup) = if (g.part == "frame") frameHeadline else headline
        val measures = groups.map { g ->
            // Room for a whole GPU name ("Adreno (TM) 540"); only a very long one is shortened.
            val note = if (g.note.isEmpty()) 0f else 6f * density.density + width(g.note, sans).coerceAtMost(160f * density.density)
            val top = width(g.title, sans) + note + 14f * density.density + width(g.value, head(g))
            val rows = g.rows.maxOfOrNull { (label, value) ->
                width(label, sans) + (if (label.isEmpty()) 0f else 10f * density.density) + width(value, mono)
            } ?: 0f
            val text = max(widest[g.part] ?: 0f, ceil(max(top, rows)))
            widest[g.part] = text
            Measured(text, text + if (g.part == "memory") gap + chartMin else 0f, 1 + g.rows.size)
        }
        val between = 2 * gap + density.density
        var start = 0
        val rows = pack(measures.map { it.least }, room, between).map { count ->
            val range = start until start + count; start += count
            range to max(line * (measures.slice(range).maxOf { it.lines }) + 4 * density.density, 48 * density.density)
        }
        val height = rows.sumOf { it.second.toDouble() }.toFloat() + (rows.size - 1).coerceAtLeast(0) * between
        Canvas(Modifier.fillMaxWidth().height(with(density) { height.toDp() })) {
            var top = 0f
            rows.forEachIndexed { r, (range, h) ->
                if (r > 0) {
                    drawLine(BORDER_SUBTLE, Offset(0f, top + gap + .5f), Offset(size.width, top + gap + .5f), density.density)
                    top += between
                }
                val weights = range.map { if (groups[it].part == "memory") 4f else 1f }
                val widths = spread(range.map { measures[it].least }, weights, room, between)
                var left = 0f
                range.forEachIndexed { k, i ->
                    if (k > 0) {
                        drawLine(BORDER_SUBTLE, Offset(left + gap + .5f, top), Offset(left + gap + .5f, top + h), density.density)
                        left += between
                    }
                    group(measurer, groups[i], measures[i], left, top, widths[k], h, line, gap, sans, mono, head(groups[i]))
                    if (groups[i].part == "memory") {
                        val x = left + measures[i].text + gap
                        memoryChart(measurer, memory, now, Offset(x, top + 2), Size(left + widths[k] - x, h - 4))
                    }
                    left += widths[k]
                }
                top += h
            }
        }
    }
}

private fun DrawScope.text(measurer: TextMeasurer, text: String, style: TextStyle, x: Float, y: Float, right: Boolean, maxWidth: Float = Float.POSITIVE_INFINITY): TextLayoutResult {
    val layout = measurer.measure(
        text, style, overflow = TextOverflow.Ellipsis, maxLines = 1,
        constraints = if (maxWidth.isFinite()) Constraints(maxWidth = max(1, maxWidth.roundToInt())) else Constraints(),
    )
    drawText(layout, topLeft = Offset(if (right) x - layout.size.width else x, y - layout.size.height / 2f))
    return layout
}

private fun DrawScope.group(
    measurer: TextMeasurer, g: MonitorGroup, m: Measured, left: Float, top: Float, width: Float, height: Float,
    line: Float, gap: Float, sans: TextStyle, mono: TextStyle, headline: TextStyle,
) {
    val right = if (g.part == "memory") left + m.text else left + width
    val y = top + line / 2
    val title = text(measurer, g.title, sans, left, y, false)
    val value = text(measurer, g.value, headline, right, y, true)
    if (g.note.isNotEmpty()) {
        val from = left + title.size.width + 6 * density
        text(measurer, g.note, sans.copy(color = INK_2), from, y, false, maxWidth = right - value.size.width - 14 * density - from)
    }
    g.rows.forEachIndexed { i, (label, v) ->
        val ry = y + line * (i + 1) + 4 * density
        text(measurer, label, sans, left, ry, false)
        text(measurer, v, mono, right, ry, true)
    }
}

private fun DrawScope.memoryChart(measurer: TextMeasurer, memory: MemoryHistory, now: Double, at: Offset, size: Size) {
    if (size.width <= 4 || size.height <= 4) return
    drawRoundRect(BG_WELL, at, size, CornerRadius(2 * density))
    drawRoundRect(BORDER_SUBTLE, at, size, CornerRadius(2 * density), style = Stroke(density))
    val inner = 2 * density
    val top = memory.top().toFloat()
    fun x(t: Double) = at.x + size.width - inner - ((now - t) / MEMORY_WINDOW_S).toFloat() * (size.width - 2 * inner)
    fun y(mb: Double) = at.y + size.height - inner - (mb.toFloat() / top).coerceIn(0f, 1f) * (size.height - 2 * inner)
    val half = y(top / 2.0)
    drawLine(BORDER_SUBTLE, Offset(at.x + inner, half), Offset(at.x + size.width - inner, half), density)
    val stroke = Stroke(1.2f * density)
    var run = mutableListOf<Offset>()
    fun flush() {
        if (run.size == 1) drawCircle(INK_2, 1.2f * density, run[0])
        if (run.size > 1) drawPath(Path().apply { moveTo(run[0].x, run[0].y); run.drop(1).forEach { lineTo(it.x, it.y) } }, INK_2, style = stroke)
        run = mutableListOf()
    }
    var previous: Double? = null
    for ((t, mb) in memory.samples) {
        if (previous != null && t - previous > MEMORY_GAP_S) flush()
        run.add(Offset(x(t).coerceAtLeast(at.x + inner), y(mb)))
        previous = t
    }
    flush()
    memory.samples.lastOrNull()?.let { (t, mb) -> drawCircle(INK_1, 1.8f * density, Offset(x(t), y(mb))) }
    val small = TextStyle(fontSize = 9.sp, fontFamily = FontFamily.Monospace, color = INK_3)
    for ((label, bottom) in listOf((if (top % 1f == 0f) "${top.toInt()} MB" else "%.1f MB".format(top)) to false, "${MEMORY_WINDOW_S.toInt()} s" to true)) {
        val layout = measurer.measure(label, small, maxLines = 1)
        val y = if (bottom) at.y + size.height - inner - layout.size.height else at.y + inner
        val x = at.x + inner + 2 * density
        drawRect(BG_WELL.copy(alpha = .85f), Offset(x - density, y), Size(layout.size.width + 2 * density, layout.size.height.toFloat()))
        drawText(layout, topLeft = Offset(x, y))
    }
}
