package io.github.lawchli.spectrum

import android.content.Context
import android.media.projection.MediaProjection

/** Configuration shared by View and Compose hosts. No capture or persistence happens implicitly. */
data class SpectrumConfiguration(
    val rowRate: Int = 60,
    val bandsPerSemitone: Int = 1,
    val seconds: Float = 4f,
    val view: String = "perspective",
    val palette: String = "tungsten",
    val style: String? = null,
    val spectrum: Boolean = true,
    val waterfall: Boolean = true,
    val peakLabels: Boolean = false,
    val maxHold: Boolean = false,
    val piano: Boolean = false,
    val lowLevel: Boolean = false,
    val frozen: Boolean = false,
) {
    init {
        require(rowRate in setOf(60, 120, 240, 480)) { "Unsupported row rate" }
        require(bandsPerSemitone in setOf(1, 2, 4, 8)) { "Unsupported band resolution" }
        require(seconds.isFinite() && seconds in 1f..12f) { "Time window must be 1..12 seconds" }
        require(spectrum || waterfall) { "At least one layer must be enabled" }
    }
}

/**
 * Reusable spectrum engine. Construct and configure on the UI thread; pushPcm may run on an audio
 * producer thread. One controller drives one SpectrumView. The caller owns permissions/lifecycle.
 */
class SpectrumController(context: Context, configuration: SpectrumConfiguration = SpectrumConfiguration()) : AutoCloseable {
    /** Advanced model access for custom control surfaces; configure() is preferred for embedding. */
    val model = SpectrumModel(context.applicationContext)
    @Volatile private var closed = false
    private var attachedView = java.lang.ref.WeakReference<SpectrumView>(null)
    val isDisposed: Boolean get() = closed
    internal fun attach(view: SpectrumView) {
        check(!closed) { "Controller is disposed" }
        check(attachedView.get() == null || attachedView.get() === view) { "One controller can drive only one attached SpectrumView" }
        attachedView = java.lang.ref.WeakReference(view)
    }
    internal fun detach(view: SpectrumView): Boolean {
        if (attachedView.get() !== view) return false
        attachedView.clear()
        return true
    }
    val availablePalettes: List<String> get() = model.appearance.paletteNames
    val availableStyles: Map<String, String> get() = model.appearance.styles.mapValues { it.value.label }
    val availableViews: Set<String> get() = model.appearance.views.keys
    val error: String get() = model.error
    val source: String get() = model.source
    val droppedPcmBlocks: Long get() = model.droppedPcmBlocks

    init { configure(configuration) }

    fun configure(configuration: SpectrumConfiguration) {
        check(!closed) { "Controller is disposed" }
        require(configuration.view in availableViews)
        require(configuration.palette in availablePalettes)
        val style = configuration.style ?: model.appearance.variantStyles.getValue(configuration.palette)
        require(style in model.appearance.styles)
        if (model.rate != configuration.rowRate || model.sub != configuration.bandsPerSemitone) {
            model.resetAnalysis(configuration.rowRate, configuration.bandsPerSemitone)
        }
        model.chooseView(configuration.view)
        model.palette = configuration.palette; model.style = style
        model.seconds = configuration.seconds; model.spectrum = configuration.spectrum
        model.waterfall = configuration.waterfall; model.peakLabels = configuration.peakLabels
        model.maxHold = configuration.maxHold; model.piano = configuration.piano
        model.lowLevel = configuration.lowLevel; model.frozen = configuration.frozen
    }

    fun startPcm(sampleRate: Int = 48000) { check(!closed); model.startPcm(sampleRate) }
    /** Interleaved Float32 PCM. Samples are copied; channels are averaged to mono. Queue is bounded. */
    fun pushPcm(samples: FloatArray, channels: Int = 1) { check(!closed); model.pushPcm(samples, channels) }
    fun startTestSignal() { check(!closed); model.startTest() }
    /** Optional helper. The host must obtain RECORD_AUDIO before calling this. */
    fun startMicrophone(deviceId: Int? = null) { check(!closed); model.startMicrophone(deviceId) }
    /** Optional helper. The host must own the MediaProjection consent and foreground service. */
    fun startPlaybackCapture(projection: MediaProjection) { check(!closed); model.startPlayback(projection) }
    fun stop() = model.stop()
    fun clearMaximum() { check(!closed); model.clearMax() }
    fun freeze(frozen: Boolean) { check(!closed); model.frozen = frozen }
    fun chooseView(view: String) { check(!closed); require(view in availableViews); model.chooseView(view) }
    fun choosePalette(palette: String) { check(!closed); model.choosePalette(palette) }
    fun dispose() {
        if (closed) return
        closed = true  // Close the public producer API before waiting for the audio worker.
        attachedView.get()?.onPause()
        attachedView.clear()
        model.onLabelsChanged = null
        model.stop()
        model.onCaptureStopped = null
    }
    override fun close() = dispose()
}
