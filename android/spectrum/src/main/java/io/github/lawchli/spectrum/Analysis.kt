package io.github.lawchli.spectrum

import kotlin.math.*

fun bandCount(sub: Int) = 126 * sub + 1
fun bandHz(band: Double) = 440.0 * 2.0.pow((band - 57.0) / 12.0)
fun dbToByte(db: Double) = floor((db + 120.0) * 2.0 + .5).toInt().coerceIn(0, 255)

/**
 * Double precision FFT, Float32 PCM and output match the browser data contract.
 * The real input is packed into one n/2-point complex FFT (even samples real, odd samples
 * imaginary) and split afterwards: half the butterflies of the browser's full complex FFT.
 */
class RealFFT(val n: Int) {
    private val half = n / 2
    private val re = DoubleArray(half)
    private val im = DoubleArray(half)
    private val window = DoubleArray(n) { i -> val x = 2 * PI * i / n; .42 - .5 * cos(x) + .08 * cos(2 * x) }
    private val reverse = IntArray(half) { Integer.reverse(it) ushr (32 - Integer.numberOfTrailingZeros(half)) }
    private val wr = DoubleArray(half) { cos(2 * PI * it / n) }
    private val wi = DoubleArray(half) { -sin(2 * PI * it / n) }
    fun run(ring: FloatArray, end: Long, out: FloatArray, bins: Int) {
        val mask = ring.size - 1; val start = end - n
        for (m in 0 until half) {
            val j = reverse[m]; val i = 2 * m
            re[j] = ring[(start + i).toInt() and mask] * window[i]
            im[j] = ring[(start + i + 1).toInt() and mask] * window[i + 1]
        }
        var size = 2
        while (size <= half) {
            val h = size / 2; val step = n / size  // exp(-2 pi i j / size) is entry j * n / size of the n-point table
            var base = 0
            while (base < half) {
                for (j in 0 until h) {
                    val a = base + j; val b = a + h; val k = j * step
                    val tr = re[b] * wr[k] - im[b] * wi[k]; val ti = re[b] * wi[k] + im[b] * wr[k]
                    re[b] = re[a] - tr; im[b] = im[a] - ti; re[a] += tr; im[a] += ti
                }; base += size
            }; size *= 2
        }
        val scale = (2 / (n * .42)).pow(2)
        for (k in 0 until bins) {
            // Even part E = (Z[k] + conj Z[N-k]) / 2, odd part O = (Z[k] - conj Z[N-k]) / 2i, X = E + W^k O.
            val nk = if (k == 0) 0 else half - k
            val a = re[k]; val b = im[k]; val c = re[nk]; val d = im[nk]
            val er = (a + c) * .5; val ei = (b - d) * .5; val odr = (b + d) * .5; val odi = (c - a) * .5
            val xr = er + wr[k] * odr - wi[k] * odi; val xi = ei + wr[k] * odi + wi[k] * odr
            out[k] = (10 * log10((xr * xr + xi * xi) * scale + 1e-30)).toFloat()
        }
    }
}
private class Analyser(sr: Int, val n: Int, sub: Int, low: Double, high: Double, rate: Int) {
    val fft = RealFFT(n)
    val data = FloatArray(n / 2) { -120f }
    val bands = FloatArray(bandCount(sub)) { -120f }
    val from = FloatArray(bands.size) { -120f }  // the analysis before `bands`, where the glide starts
    val every = max(1, floor(n / 16.0 / (sr.toDouble() / rate)).toInt())
    private fun ramp(hz: Double, edge: Double) = ((12 * log2(hz / edge) + 2) / 4).coerceIn(0.0, 1.0)
    val weights = FloatArray(bands.size) { k -> val hz = bandHz(k.toDouble() / sub)
        ((if (low > 0) ramp(hz, low) else 1.0) * (if (high.isFinite()) 1 - ramp(hz, high) else 1.0)).toFloat() }
    // Per band: FFT bin range lo..hi (max of bins), or a fractional bin position (cubic interpolation), or empty.
    private val lo = IntArray(bands.size) { -1 }
    private val hi = IntArray(bands.size) { -1 }
    private val pos = DoubleArray(bands.size) { -1.0 }
    val bins: Int
    init {
        val width = sr.toDouble() / n; val half = 2.0.pow(1.0 / (24 * sub)); val widen = 2.0.pow(2.0 / 12); var top = 0
        for (k in bands.indices) {
            val hz = bandHz(k.toDouble() / sub)
            if (hz < low / widen || hz >= high * widen || hz / half >= sr / 2.0) continue
            val b0 = ceil(hz / half / width).toInt(); val b1 = min(floor(hz * half / width).toInt(), n / 2 - 1)
            if (b1 >= b0 && b0 >= 1) { top = max(top, b1); lo[k] = b0; hi[k] = b1 }
            else { val p = min(hz / width, (n / 2 - 1).toDouble()); top = max(top, min(floor(p).toInt() + 2, n / 2 - 1)); pos[k] = p }
        }; bins = top + 1
    }
    fun analyse(ring: FloatArray, end: Long) {
        fft.run(ring, end, data, bins)
        val last = data.lastIndex
        for (k in bands.indices) {
            val p = pos[k]
            val db = if (p >= 0) {
                val i = floor(p).toInt(); val f = p - i
                val a = data[max(0, i - 1)].toDouble(); val b = data[i].toDouble(); val c = data[min(last, i + 1)].toDouble(); val d = data[min(last, i + 2)].toDouble()
                b + .5 * f * (c - a + f * (2 * a - 5 * b + 4 * c - d + f * (3 * (b - c) + d - a)))
            } else if (lo[k] >= 0) { var v = -120.0; for (j in lo[k]..hi[k]) v = max(v, data[j].toDouble()); v } else -120.0
            bands[k] = (if (db.isFinite()) db.coerceIn(-120.0, 0.0) else -120.0).toFloat()
        }
    }
}
class Analysis(val sampleRate: Int = 48000, val rowRate: Int = 60, val sub: Int = 1) {
    val ring = FloatArray(32768)
    var written = 0L; private set
    var rowIndex = 0L; private set
    val bands = FloatArray(bandCount(sub)) { -120f }
    private val analysers = listOf(Analyser(sampleRate, 8192, sub, 0.0, 350.0, rowRate),
        Analyser(sampleRate, 2048, sub, 350.0, 1500.0, rowRate), Analyser(sampleRate, 1024, sub, 1500.0, Double.POSITIVE_INFINITY, rowRate))
    // Row each analyser last ran. With every row refreshed this is the browser's rowIndex % every schedule;
    // when rows are skipped under load, a due analyser runs at the next refreshed row instead of waiting a cycle.
    private val lastRun = LongArray(analysers.size) { Long.MIN_VALUE / 4 }
    fun receive(pcm: FloatArray, length: Int = pcm.size, offset: Int = 0) {
        for (i in offset until offset + length) { ring[(written.toInt()) and 32767] = pcm[i]; written++ }
    }
    /** One data row ending at sample `end`. With refresh = false no FFT runs and the previous row is repeated. */
    fun analyse(end: Long, refresh: Boolean = true): FloatArray {
        if (refresh) {
            bands.fill(0f)
            analysers.forEachIndexed { i, a ->
                if (rowIndex - lastRun[i] >= a.every) { if (a.every > 1) a.bands.copyInto(a.from); a.analyse(ring, end); lastRun[i] = rowIndex }
                // Rows between two analyses of a long window glide from the previous one to the new one (audio.js).
                val f = min(1.0, (rowIndex - lastRun[i] + 1).toDouble() / a.every)
                for (k in bands.indices) if (a.weights[k] > 0) {
                    val db = if (f >= 1.0) a.bands[k].toDouble() else a.from[k] + (a.bands[k].toDouble() - a.from[k]) * f
                    bands[k] = (bands[k].toDouble() + a.weights[k].toDouble() * db).toFloat()
                }
            }
        }
        rowIndex++; return bands
    }
    fun peakHz(band: Double): Double? {
        val k = (band * sub).roundToInt().coerceIn(bands.indices); val a = analysers.maxByOrNull { it.weights[k] } ?: return null
        val width = sampleRate.toDouble() / a.n; val lo = max(1, floor(bandHz(band - .6) / width).toInt()); val hi = min(a.bins - 2, ceil(bandHz(band + .6) / width).toInt())
        if (hi < lo) return null
        val i = (lo..hi).maxByOrNull { a.data[it] } ?: return null
        val curve = a.data[i - 1].toDouble() - 2 * a.data[i] + a.data[i + 1]
        val off = if (curve < 0) (.5 * (a.data[i - 1].toDouble() - a.data[i + 1]) / curve).coerceIn(-.5, .5) else 0.0
        return (i + off) * width
    }
}
/**
 * Six-level power-mean history. The analysis thread pushes and the GL thread reads `written` and
 * `dirty` only while holding SpectrumModel.lock; rows below that snapshot stay unchanged until the
 * ring wraps, so they can be uploaded after the lock is released.
 */
class History(val count: Int, val rows: Int = 4096) {
    val levels = Array(6) { ByteArray(count * rows) }
    private val sums = Array(6) { FloatArray(count) }
    private val power = FloatArray(256) { 10.0.pow((it / 2.0 - 120) / 10).toFloat() }
    val dirty = IntArray(6)
    @Volatile var written = 0L; private set
    fun push(bytes: ByteArray) {
        val n = written++; bytes.copyInto(levels[0], (n % rows).toInt() * count); dirty[0]++
        for (l in 1..5) {
            val span = 1 shl l
            for (k in bytes.indices) sums[l][k] += power[bytes[k].toInt() and 255]
            if ((n + 1) % span != 0L) continue
            val base = (n / span % rows).toInt() * count
            for (k in bytes.indices) { levels[l][base + k] = dbToByte(10 * log10(sums[l][k].toDouble() / span + 1e-30)).toByte(); sums[l][k] = 0f }; dirty[l]++
        }
    }
    fun sample(band: Double, age: Int, sub: Int): Double {
        if (age >= written) return -120.0
        var l = 0; while (l < 5 && age >= (rows - 2) * (1 shl l)) l++
        val row = written - 1 - max(0, age); val k = (band * sub).roundToInt().coerceIn(0, count - 1)
        return (levels[l][(row / (1 shl l) % rows).toInt() * count + k].toInt() and 255) / 2.0 - 120
    }
}
