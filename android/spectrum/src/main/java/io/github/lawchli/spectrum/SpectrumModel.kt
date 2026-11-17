package io.github.lawchli.spectrum

import android.content.Context
import android.media.*
import android.os.Handler
import android.os.Looper
import android.media.projection.MediaProjection
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import kotlin.math.*

class SpectrumModel(val context: Context) {
    val lock = Any()
    val appearance = Appearance.load(context)
    private val initial = appearance.views.getValue("perspective")
    private val uiHandler = Handler(Looper.getMainLooper())
    var onLabelsChanged: ((List<AxisLabel>) -> Unit)? = null
    var onCaptureStopped: (() -> Unit)? = null
    private val labelDispatchPending = AtomicBoolean(false)
    @Volatile private var pendingLabels: List<AxisLabel> = emptyList()
    @Volatile var displayVersion = 0L
    @Volatile var msaaSamples = 0
    @Volatile var requestedSamples = 0
    val droppedPcmBlocks: Long get() = pcmInput.droppedBlocks.get()
    private val pcmInput = PcmInput().apply { close() }
    @Volatile var rate = 60
    @Volatile var sub = 1
    @Volatile var seconds = 4f
    @Volatile var lowLevel = false
    @Volatile var frozen = false
    @Volatile var spectrum = true
    @Volatile var waterfall = true
    @Volatile var piano = false
    @Volatile var maxHold = false
    @Volatile var peakLabels = false
    @Volatile var palette = appearance.defaultPalette
    @Volatile var style = appearance.defaultStyle
    @Volatile var source = "off"
    @Volatile var error = ""
    @Volatile var fps = 0f
    @Volatile var drawMs = 0f
    /** GPU time of the last timed frame in ms, or null where the GPU has no timer queries. */
    @Volatile var gpuMs: Float? = null
    /** Size of the drawing surface in pixels. */
    @Volatile var viewportWidth = 0
    @Volatile var viewportHeight = 0
    @Volatile var analysisMs = 0f
    @Volatile var rowsPerSecond = 0f
    @Volatile var fftRowsPerSecond = 0f  // rows that ran a fresh FFT; below rowsPerSecond when overloaded
    @Volatile var analysisStride = 1
    @Volatile var vertices = 0
    @Volatile var rendererName = ""
    @Volatile var sampleRate = 48000
    @Volatile var latencyMs = 0f
    @Volatile var peakText = ""
    @Volatile var positions: List<AxisLabel> = emptyList()
    @Volatile var cameraMatrix = FloatArray(16)
    @Volatile var view = "perspective"
    @Volatile var yaw = initial.yaw
    @Volatile var pitch = initial.pitch
    @Volatile var zoom = initial.zoom
    @Volatile var panX = initial.pan[0]
    @Volatile var panY = initial.pan[1]
    @Volatile var panZ = initial.pan[2]
    @Volatile var fov = initial.fov
    @Volatile var history = History(bandCount(1))
    @Volatile var shown = FloatArray(bandCount(1)) { -120f }
    @Volatile var peak = shown.clone()
    @Volatile var peakAge = shown.clone().apply { fill(0f) }
    @Volatile var pianoBand = -1
    @Volatile var maximum = shown.clone()
    @Volatile var phase = 0f
    @Volatile var lastAudioNano = 0L
    @Volatile var rowTimestampNano = 0L
    var activeDeviceId: Int? = null
        private set
    private var running = AtomicBoolean(false)
    private var worker: Thread? = null
    private var recorder: AudioRecord? = null
    var projection: MediaProjection? = null
    fun chooseView(name: String) {
        val preset = appearance.views[name] ?: return
        view = name
        yaw = preset.yaw; pitch = preset.pitch; fov = preset.fov; zoom = preset.zoom
        panX = preset.pan[0]; panY = preset.pan[1]; panZ = preset.pan[2]
    }
    fun choosePalette(name: String) {
        require(name in appearance.paletteNames)
        palette = name
        style = appearance.variantStyles.getValue(name)
    }
    fun publishLabels(labels: List<AxisLabel>) {
        pendingLabels = labels
        if (labelDispatchPending.compareAndSet(false, true)) uiHandler.post {
            labelDispatchPending.set(false)
            positions = pendingLabels
            onLabelsChanged?.invoke(positions)
        }
    }
    /** External PCM is opt-in; the library never opens a recorder merely by being mounted. */
    fun startPcm(inputSampleRate: Int = 48000) {
        require(inputSampleRate in 8000..192000)
        stop(); sampleRate = inputSampleRate; pcmInput.reset()
        startWorker(null, "pcm")
    }
    fun pushPcm(pcm: FloatArray, channels: Int = 1) {
        check(source == "pcm") { "Call startPcm() before pushPcm()" }
        pcmInput.push(pcm, channels)
    }
    // A new resolution restarts only the analysis thread. The recorder, and for playback capture the
    // MediaProjection grant, stay alive: stopping the projection would need a fresh consent dialog.
    fun resetAnalysis(newRate: Int, newSub: Int) {
        require(newRate in listOf(60, 120, 240, 480) && newSub in listOf(1, 2, 4, 8))
        val previous = source; val record = recorder
        stopWorker(); rate = newRate; sub = newSub
        synchronized(lock) { history = History(bandCount(sub)); shown = FloatArray(bandCount(sub)) { -120f }; peak = shown.clone(); peakAge = FloatArray(shown.size); maximum = shown.clone(); displayVersion++ }
        when {
            previous == "test" -> startWorker(null, "test")
            previous == "pcm" -> { pcmInput.clear(); startWorker(null, "pcm") }
            record != null && previous in listOf("mic", "usb", "system") ->
                try { record.startRecording(); startWorker(record, previous) } catch (e: Exception) { stop(); error = e.message ?: e.toString() }
        }
    }
    fun clearMax() = synchronized(lock) { maximum.fill(-120f); displayVersion++ }
    // Stopping the recorder unblocks a blocking read; startRecording() resumes the same AudioRecord.
    private fun stopWorker() {
        running.set(false)
        try { recorder?.stop() } catch (_: Exception) {}
        worker?.interrupt()
        if (worker !== Thread.currentThread()) worker?.join(1000)
        worker = null
    }
    fun stop() {
        source = "off"  // Reject producer pushes before waiting for a blocking audio read to finish.
        pcmInput.close()
        stopWorker()
        val hadProjection = projection != null
        val record = recorder; recorder = null
        try { record?.release() } catch (_: Exception) {}
        val grant = projection; projection = null
        try { grant?.stop() } catch (_: Exception) {}
        if (hadProjection) onCaptureStopped?.invoke()
    }
    fun startTest() { stop(); sampleRate = 48000; startWorker(null, "test") }
    @Suppress("MissingPermission")
    fun startMicrophone(deviceId: Int? = null) {
        stop()
        try {
            val sr = 48000
            val format = AudioFormat.Builder().setSampleRate(sr).setChannelMask(AudioFormat.CHANNEL_IN_MONO).setEncoding(AudioFormat.ENCODING_PCM_FLOAT).build()
            val size = max(BUFFER_BYTES, AudioRecord.getMinBufferSize(sr, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_FLOAT))
            val audioManager = context.getSystemService(AudioManager::class.java)
            val raw = audioManager.getProperty(AudioManager.PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED) == "true"
            val record = AudioRecord.Builder().setAudioSource(if (raw) MediaRecorder.AudioSource.UNPROCESSED else MediaRecorder.AudioSource.VOICE_RECOGNITION).setAudioFormat(format).setBufferSizeInBytes(size).build()
            recorder = record
            if (deviceId != null) {
                val device = audioManager.getDevices(AudioManager.GET_DEVICES_INPUTS).firstOrNull { it.id == deviceId } ?: error("Input disconnected")
                check(record.setPreferredDevice(device)) { "Cannot select USB input" }
            }
            check(record.state == AudioRecord.STATE_INITIALIZED) { "AudioRecord initialization failed" }
            activeDeviceId = deviceId; sampleRate = sr; record.startRecording(); startWorker(record, if (deviceId == null) "mic" else "usb")
        } catch (e: Exception) { stop(); error = e.message ?: e.toString() }
    }
    @Suppress("MissingPermission")
    fun startPlayback(p: MediaProjection) {
        if (android.os.Build.VERSION.SDK_INT >= 29) {
            stop()
            projection = p
            try {
                val config = AudioPlaybackCaptureConfiguration.Builder(p).addMatchingUsage(AudioAttributes.USAGE_MEDIA).addMatchingUsage(AudioAttributes.USAGE_GAME).addMatchingUsage(AudioAttributes.USAGE_UNKNOWN).build()
                val format = AudioFormat.Builder().setSampleRate(48000).setChannelMask(AudioFormat.CHANNEL_IN_MONO).setEncoding(AudioFormat.ENCODING_PCM_FLOAT).build()
                val record = AudioRecord.Builder().setAudioPlaybackCaptureConfig(config).setAudioFormat(format).setBufferSizeInBytes(max(BUFFER_BYTES, AudioRecord.getMinBufferSize(48000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_FLOAT))).build()
                recorder = record
                check(record.state == AudioRecord.STATE_INITIALIZED)
                sampleRate = 48000; record.startRecording(); startWorker(record, "system")
            } catch (e: Exception) { stop(); error = e.message ?: e.toString() }
        } else throw UnsupportedOperationException("Playback capture requires Android 10 or newer")
    }
    private fun startWorker(record: AudioRecord?, name: String) {
        // Each worker owns its token. A timed-out old read can never be reactivated by a new source.
        val running = AtomicBoolean(true)
        this.running = running
        error = ""; source = name; analysisStride = 1
        val rate = this.rate; val sub = this.sub; val sr = sampleRate  // fixed for this thread; resetAnalysis restarts it
        worker = thread(name = "spectrum-analysis") {
            android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_AUDIO)
            val a = Analysis(sr, rate, sub); var block = FloatArray(256); val bytes = ByteArray(bandCount(sub))
            val tones = doubleArrayOf(110.0, 164.813778, 195.997718, 246.94165, 261.62556)
            val timestamp = AudioTimestamp(); var timestampValid = false; var lastTimestampCheck = 0L
            val hop = sr.toDouble() / rate; var next = hop; var sample = 0L; val started = System.nanoTime(); var lastCount = started; var rows = 0
            // Overload: rows keep coming on the audio clock, but when analysis would use more than ~3/4
            // of real time the FFTs refresh only every `stride` rows and the rows between repeat the last
            // spectrum. The time axis stays correct and the recorder never falls behind and overruns.
            var stride = 1; var skipped = 0; var busyNs = 0L; var windowRows = 0; var fftRows = 0
            try {
                while (running.get()) {
                    val length = if (record != null) record.read(block, 0, block.size, AudioRecord.READ_BLOCKING) else if (name == "pcm") {
                        block = pcmInput.read() ?: continue
                        block.size
                    } else {
                        for (i in block.indices) { val t = sample++ / sr.toDouble(); block[i] = (tones.sumOf { f -> .09 * sin(2 * PI * f * t) }).toFloat() }
                        val due = started + sample * 1000000000L / sr
                        val wait = due - System.nanoTime(); if (wait > 0) Thread.sleep(wait / 1000000, (wait % 1000000).toInt()); block.size
                    }
                    if (!running.get()) break
                    if (length <= 0) { if (running.get()) error("Audio read error $length") else break }
                    if (record != null && System.nanoTime() - lastTimestampCheck > 50000000) {
                        lastTimestampCheck = System.nanoTime()
                        timestampValid = record.getTimestamp(timestamp, AudioTimestamp.TIMEBASE_MONOTONIC) == AudioRecord.SUCCESS
                    }
                    // Split at exact row boundaries so high-rate rows never read overwritten samples.
                    var offset = 0
                    while (offset < length && running.get()) {
                        val take = min(length - offset, max(1, next.roundToLong().toInt() - a.written.toInt()))
                        a.receive(block, take, offset); offset += take
                        if (a.written >= next.roundToLong()) {
                            val refresh = skipped + 1 >= stride
                            val begin = System.nanoTime(); val db = a.analyse(next.roundToLong(), refresh); val spent = System.nanoTime() - begin
                            if (refresh) { skipped = 0; fftRows++; analysisMs = analysisMs * .95f + spent / 1e6f * .05f } else skipped++
                            busyNs += spent; windowRows++
                            if (windowRows >= max(8, rate / 2)) {  // re-evaluate every half second of audio
                                val load = busyNs / (windowRows * 1e9 / rate)  // analysis seconds per second of audio
                                stride = when {
                                    load > .75 && stride < 8 -> stride + 1
                                    stride > 1 && load * stride / (stride - 1) < .5 -> stride - 1
                                    else -> stride
                                }
                                analysisStride = stride; busyNs = 0; windowRows = 0
                            }
                            if (!frozen) synchronized(lock) {
                                if (running.get()) {
                                    for (k in db.indices) { bytes[k] = dbToByte(db[k].toDouble()).toByte(); shown[k] = max(db[k], shown[k] - 36f / rate); if (db[k] >= peak[k]) { peak[k] = db[k]; peakAge[k] = 0f } else { peakAge[k] += 1f / rate; if (peakAge[k] > 1.5f) peak[k] = max(shown[k], peak[k] - 18f / rate) }; maximum[k] = max(maximum[k], shown[k]) }
                                    history.push(bytes); displayVersion++; lastAudioNano = System.nanoTime()
                                    rowTimestampNano = if (name == "pcm") 0L else if (record == null) started + (next / sr * 1e9).toLong()
                                        else if (timestampValid) timestamp.nanoTime + ((next - timestamp.framePosition) / sr * 1e9).toLong() else 0L
                                }
                            }
                            // Text formatting stays outside the lock the GL thread also takes.
                            if (running.get() && !frozen && refresh && peakLabels && a.rowIndex % max(1, rate / 8) < stride) {
                                val k = db.indices.maxByOrNull { db[it] } ?: 0
                                pianoBand = if (db[k] > -96) (k.toFloat() / sub).roundToInt().coerceIn(0, 126) else -1
                                val hz = a.peakHz(k.toDouble() / sub) ?: bandHz(k.toDouble() / sub)
                                val midi = 69 + 12 * log2(hz / 440); val note = midi.roundToInt(); val cents = ((midi - note) * 100).roundToInt()
                                peakText = "${arrayOf("C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B")[(note % 12 + 12) % 12]}${note / 12 - 1} ${if (cents > 0) "+" else ""}$cents¢  ${"%.1f".format(hz)} Hz  ${"%.1f".format(db[k])} dB"
                            }
                            next += hop; rows++
                        }
                    }
                    if (!running.get()) break
                    val now = System.nanoTime(); if (now - lastCount > 1000000000) { rowsPerSecond = rows * 1e9f / (now - lastCount); fftRowsPerSecond = fftRows * 1e9f / (now - lastCount); rows = 0; fftRows = 0; lastCount = now }
                    phase = ((a.written - (next - hop)) / hop).toFloat().coerceIn(0f, 1f)
                }
            } catch (e: Exception) {
                if (running.get()) {
                    val failedWorker = Thread.currentThread()
                    val message = e.message ?: e.toString()
                    // Cleanup on the owner thread avoids joining ourselves and racing a newly selected source.
                    uiHandler.post {
                        if (worker === failedWorker) { stop(); error = message; onCaptureStopped?.invoke() }
                    }
                }
            }
        }
    }
    companion object {
        // A quarter second of mono float audio absorbs GC pauses and scheduling hiccups without
        // adding latency: the analysis thread reads as soon as data is available.
        private const val BUFFER_BYTES = 48000 * 4 / 4
    }
}
data class AxisLabel(val text: String, val x: Float, val y: Float, val alpha: Float = 1f)
