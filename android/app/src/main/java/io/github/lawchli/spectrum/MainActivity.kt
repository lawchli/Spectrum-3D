package io.github.lawchli.spectrum

import android.Manifest
import android.content.*
import android.content.pm.PackageManager
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.media.projection.MediaProjectionManager
import android.opengl.GLSurfaceView
import android.os.*
import android.view.*
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.*
import androidx.compose.foundation.gestures.*
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.*
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.*
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject
import kotlin.math.*

class MainActivity : ComponentActivity() {
    private lateinit var controller: SpectrumController
    private lateinit var model: SpectrumModel
    private var pausedSource: String? = null
    private var pausedDevice: Int? = null
    private var spectrumView: SpectrumView? = null
    private var pendingInput: (() -> Unit)? = null
    private val permission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) pendingInput?.invoke() else model.error = NativeStrings(this).text("microphonePermissionDenied"); pendingInput = null
    }
    private val projection = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode == RESULT_OK && result.data != null) {
            pausedSource = null
            model.stop()
            DemoSession.active = model
            startForegroundService(Intent(this, CaptureService::class.java).putExtra("data", result.data))
        }
    }
    private var notificationReady: (() -> Unit)? = null
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        notificationReady?.invoke(); notificationReady = null
    }
    private fun requestPlaybackCapture() {
        val launch = { projection.launch(getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent()) }
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            notificationReady = launch; notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else launch()
    }
    private fun restoreSettings(prefs: android.content.SharedPreferences) {
        model.rate = prefs.getInt("rate", 60).takeIf { it in listOf(60, 120, 240, 480) } ?: 60
        model.sub = prefs.getInt("sub", 1).takeIf { it in listOf(1, 2, 4, 8) } ?: 1
        model.seconds = prefs.getFloat("seconds", 4f).coerceIn(1f, 12f)
        model.spectrum = prefs.getBoolean("spectrum", true); model.waterfall = prefs.getBoolean("waterfall", true)
        if (!model.spectrum && !model.waterfall) model.spectrum = true
        model.peakLabels = prefs.getBoolean("peakLabels", false); model.maxHold = prefs.getBoolean("maxHold", false)
        model.piano = prefs.getBoolean("piano", false); model.lowLevel = prefs.getBoolean("lowLevel", false)
        model.frozen = prefs.getBoolean("frozen", false)
        model.palette = prefs.getString("palette", model.palette)?.takeIf { it in model.appearance.paletteNames } ?: model.palette
        model.style = prefs.getString("style", model.appearance.variantStyles.getValue(model.palette))?.takeIf { it in model.appearance.styles } ?: model.appearance.defaultStyle
        val view = prefs.getString("view", "perspective") ?: "perspective"
        model.chooseView(view.takeIf { it in model.appearance.views } ?: "perspective")
        if (view == "free") {
            model.view = "free"
            model.yaw = prefs.getFloat("yaw", model.yaw)
            model.pitch = prefs.getFloat("pitch", model.pitch).coerceIn(model.appearance.pitchMin, model.appearance.pitchMax)
            model.fov = prefs.getFloat("fov", model.fov).coerceIn(1f, 90f)
            model.zoom = prefs.getFloat("zoom", model.zoom).coerceIn(.25f, 3f)
            model.panX = prefs.getFloat("panX", model.panX); model.panY = prefs.getFloat("panY", model.panY); model.panZ = prefs.getFloat("panZ", model.panZ)
        }
        // New key: builds before 2026-11-14 stored their old 4x default under "msaa" for every user.
        model.requestedSamples = if (prefs.getInt("msaaChoice", 0) == 4) 4 else 0
    }
    private fun saveSettings(prefs: android.content.SharedPreferences = getSharedPreferences("spectrum", MODE_PRIVATE)) {
        prefs.edit().putInt("rate", model.rate).putInt("sub", model.sub).putFloat("seconds", model.seconds)
            .putBoolean("spectrum", model.spectrum).putBoolean("waterfall", model.waterfall)
            .putBoolean("peakLabels", model.peakLabels).putBoolean("maxHold", model.maxHold)
            .putBoolean("piano", model.piano).putBoolean("lowLevel", model.lowLevel).putBoolean("frozen", model.frozen)
            .putString("palette", model.palette).putString("style", model.style).putString("view", model.view)
            .putFloat("yaw", model.yaw).putFloat("pitch", model.pitch).putFloat("fov", model.fov).putFloat("zoom", model.zoom)
            .putFloat("panX", model.panX).putFloat("panY", model.panY).putFloat("panZ", model.panZ)
            .putInt("msaaChoice", model.requestedSamples).apply()
    }
    /** USB inputs to offer as sources. Every input the system reports goes to logcat (tag
     *  SpectrumDevices) with its type, so a sound card that shows up there but not here can be
     *  told apart from one the system never offered. */
    private fun usbInputs(): List<AudioDeviceInfo> {
        val all = getSystemService(AudioManager::class.java).getDevices(AudioManager.GET_DEVICES_INPUTS)
        // AudioDeviceInfo.address needs Android 9 (API 28); the library and demo run from API 26.
        for (d in all) android.util.Log.i("SpectrumDevices", "input id=${d.id} type=${d.type} name=${d.productName} address=${if (Build.VERSION.SDK_INT >= 28) d.address else "n/a"}")
        return all.filter { it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET }
    }
    private fun withAudioPermission(action: () -> Unit) {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) action()
        else { pendingInput = action; permission.launch(Manifest.permission.RECORD_AUDIO) }
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false)
            window.decorView.post { window.insetsController?.apply { hide(android.view.WindowInsets.Type.systemBars()); systemBarsBehavior = android.view.WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE } }
        } else { @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_FULLSCREEN or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
        }
        controller = SpectrumController(applicationContext)
        model = controller.model; DemoSession.active = model
        model.onCaptureStopped = { stopService(Intent(this, CaptureService::class.java)) }
        val prefs = getSharedPreferences("spectrum", MODE_PRIVATE)
        restoreSettings(prefs)
        applyLaunchExtras(intent, initial = true)
        setContent {
            MaterialTheme(colorScheme = darkColorScheme(primary = Color(0xfff0bb68), background = Color.Black, surface = Color(0xff111214), onSurface = Color(0xffd5d8dc))) {
                App(prefs)
            }
        }
    }
    /** Test and benchmark launches (`adb shell am start ... --ei rate 480`). With singleTask a second
     *  launch arrives in onNewIntent; an intent without extras (launcher, notification) changes nothing. */
    private fun applyLaunchExtras(intent: Intent, initial: Boolean) {
        val rate = if (intent.hasExtra("rate")) intent.getIntExtra("rate", 60).takeIf { it in listOf(60, 120, 240, 480) } ?: 60 else model.rate
        val sub = if (intent.hasExtra("sub")) intent.getIntExtra("sub", 1).takeIf { it in listOf(1, 2, 4, 8) } ?: 1 else model.sub
        if (initial || rate != model.rate || sub != model.sub) model.resetAnalysis(rate, sub)
        if (intent.getStringExtra("orientation") == "landscape") requestedOrientation = android.content.pm.ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
        if (intent.getStringExtra("orientation") == "portrait") requestedOrientation = android.content.pm.ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
        if (intent.hasExtra("seconds")) model.seconds = intent.getFloatExtra("seconds", model.seconds).coerceIn(1f, 12f)
        // Read when the GL surface is created, so on a running instance it applies after a restart.
        if (intent.hasExtra("msaa")) model.requestedSamples = if (intent.getIntExtra("msaa", 0) == 4) 4 else 0
        if (intent.getStringExtra("source") == "test") { model.frozen = false; model.startTest() }
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent); setIntent(intent)
        applyLaunchExtras(intent, initial = false)
    }
    @Composable private fun App(prefs: android.content.SharedPreferences) {
        var refresh by remember { mutableIntStateOf(0) }
        LaunchedEffect(Unit) { while (true) { delay(250); refresh++ } }
        // The USB list follows the system as cards come and go, not only when the panel redraws.
        var usb by remember { mutableStateOf(usbInputs()) }
        DisposableEffect(Unit) {
            val audio = getSystemService(AudioManager::class.java)
            val callback = object : AudioDeviceCallback() {
                override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) { usb = usbInputs() }
                override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) { usb = usbInputs() }
            }
            audio.registerAudioDeviceCallback(callback, Handler(Looper.getMainLooper()))
            onDispose { audio.unregisterAudioDeviceCallback(callback) }
        }
        var lang by remember { mutableStateOf(prefs.getString("lang", if (resources.configuration.locales[0].language == "zh") "zh" else "en")!!) }
        val strings = remember { JSONObject(assets.open("spectrum/strings.json").bufferedReader().use { it.readText() }) }
        fun t(key: String): String = strings.getJSONObject(lang).optString(key, key)
        var panel by remember { mutableStateOf(prefs.getBoolean("panel", resources.configuration.screenWidthDp > 600 && resources.configuration.screenHeightDp > 500)) }
        var hud by remember { mutableStateOf(prefs.getBoolean("hud", true)) }
        var bare by remember { mutableStateOf(prefs.getBoolean("bare", false)) }
        val memory = remember { MemoryHistory() }
        var memoryNow by remember { mutableDoubleStateOf(0.0) }
        LaunchedEffect(hud) {
            memory.clear()
            while (hud) {
                val (resident, peak) = withContext(Dispatchers.IO) { readResident() }
                memoryNow = SystemClock.elapsedRealtime() / 1000.0
                memory.record(memoryNow, resident, peak)
                delay(1000)
            }
        }
        fun save() { prefs.edit().putString("lang", lang).putBoolean("panel", panel).putBoolean("hud", hud).putBoolean("bare", bare).putInt("rate", model.rate).putInt("sub", model.sub).apply(); saveSettings(prefs); refresh++ }
        @Composable fun Button(text: String, selected: Boolean = false, modifier: Modifier = Modifier, oneLine: Boolean = false, click: () -> Unit) {
            TextButton(onClick = { click(); saveSettings(prefs); refresh++ }, modifier = modifier, colors = ButtonDefaults.textButtonColors(contentColor = if (selected) Color(0xfff0bb68) else Color(0xffb6bbc3)), contentPadding = PaddingValues(horizontal = if (oneLine) 2.dp else 8.dp, vertical = 4.dp), shape = RoundedCornerShape(3.dp)) { Text(text, fontSize = 12.sp, maxLines = if (oneLine) 1 else Int.MAX_VALUE, softWrap = !oneLine) }
        }
        BoxWithConstraints(Modifier.fillMaxSize().background(Color.Black)) {
            // Portrait phones: the panel floats over the spectrum (translucent) instead of taking
            // 60 % of the width; landscape and tablets keep it beside the spectrum.
            val overlay = maxWidth < maxHeight
            val monitorMax = maxHeight * .4f
            @Composable fun Panel(background: Color) {
                // The model is not Compose state: reading the tick redraws the panel with it,
                // so a choice lights up at once and readings stay current.
                @Suppress("UNUSED_VARIABLE") val tick = refresh
                Column(Modifier.width(224.dp).fillMaxHeight().background(background).verticalScroll(rememberScrollState()).padding(10.dp)) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) { Text("SPECTRUM", color = Color(0xffd5d8dc), fontSize = 13.sp); Button("‹") { panel = false; save() } }
                    Row { Button("中", lang == "zh") { lang = "zh"; save() }; Button("EN", lang == "en") { lang = "en"; save() } }
                    HorizontalDivider(color = Color(0xff292c31))
                    Text(t("source"), color = Color(0xff777f89), fontSize = 12.sp)
                    Row { Button(t("synth"), model.source == "test") { model.startTest() }; Button(t("stop")) { model.stop(); stopService(Intent(this@MainActivity, CaptureService::class.java)) } }
                    Button(t("input"), model.source == "mic") { withAudioPermission { model.startMicrophone() } }
                    if (Build.VERSION.SDK_INT >= 29) Button(t("systemAudio"), model.source == "system") { withAudioPermission { requestPlaybackCapture() } }
                    for (device in usb) Button("USB  ${device.productName}", model.source == "usb") { withAudioPermission { model.startMicrophone(device.id) } }
                    HorizontalDivider(color = Color(0xff292c31))
                    Text(t("view"), color = Color(0xff777f89), fontSize = 12.sp)
                    Row { for (v in model.appearance.views.keys) Button(t(v), model.view == v) { model.chooseView(v) } }
                    Row { Button(t("layerSpectrum"), model.spectrum) { if (model.waterfall) model.spectrum = !model.spectrum }; Button(t("layerWaterfall"), model.waterfall) { if (model.spectrum) model.waterfall = !model.waterfall } }
                    Row { Button(t("freeze"), model.frozen) { model.frozen = !model.frozen }; Button(t("hideUi")) { bare = true; save() } }
                    Text(t("look"), color = Color(0xff777f89), fontSize = 12.sp)
                    for ((key, style) in model.appearance.styles) Button(style.label, model.style == key) { model.style = key }
                    Row { Button(t("palette.${model.palette}")) { val names = model.appearance.paletteNames; model.choosePalette(names[(names.indexOf(model.palette) + 1) % names.size]) } }
                    Text("${t("time")}  ${model.seconds.roundToInt()} s", color = Color(0xffa8afb8), fontSize = 12.sp)
                    Slider(value = model.seconds, onValueChange = { model.seconds = it.roundToInt().toFloat(); saveSettings(prefs); refresh++ }, valueRange = 1f..12f, steps = 10)
                    Button(t("peaks"), model.peakLabels) { model.peakLabels = !model.peakLabels }
                    Row { Button(t("maxHold"), model.maxHold) { model.maxHold = !model.maxHold }; Button(t("clear")) { model.clearMax() } }
                    Button(t("piano"), model.piano) { model.piano = !model.piano }
                    Text(t("rowRate"), color = Color(0xff777f89), fontSize = 12.sp)
                    Row(Modifier.fillMaxWidth()) { for (rate in listOf(60, 120, 240, 480)) Button("$rate", model.rate == rate, Modifier.weight(1f), oneLine = true) { model.resetAnalysis(rate, model.sub); save() } }
                    Text(t("bandSub"), color = Color(0xff777f89), fontSize = 12.sp)
                    Row(Modifier.fillMaxWidth()) { for (sub in listOf(1, 2, 4, 8)) Button("$sub", model.sub == sub, Modifier.weight(1f), oneLine = true) { model.resetAnalysis(model.rate, sub); save() } }
                    Button(t("lowLevel"), model.lowLevel) { model.lowLevel = !model.lowLevel }
                    Button("${t("antiAliasing")}  ${model.requestedSamples}×", model.requestedSamples > 0) { model.requestedSamples = if (model.requestedSamples == 0) 4 else 0 }
                    Text(t("restartRequired"), color = Color(0xff777f89), fontSize = 10.sp)
                }
            }
            Row(Modifier.fillMaxSize()) {
                if (!bare && !overlay) {
                    if (panel) Panel(Color(0xff111214))
                    else Column(Modifier.width(28.dp).fillMaxHeight().background(Color(0xff111214))) { Button("›") { panel = true; save() } }
                }
                Column(Modifier.weight(1f).fillMaxHeight()) {
                    Box(Modifier.weight(1f).fillMaxWidth()) {
                        AndroidView(factory = { context ->
                            SpectrumView(context, controller, model.requestedSamples).apply {
                                gesturesEnabled = false; spectrumView = this
                            }
                        }, modifier = Modifier.fillMaxSize().pointerInput(Unit) {
                            awaitEachGesture {
                                awaitFirstDown(requireUnconsumed = false)
                                do {
                                    val event = awaitPointerEvent()
                                    val count = event.changes.count { it.pressed }
                                    val pan = event.calculatePan(); val scale = event.calculateZoom()
                                    if (count > 1) {
                                        model.zoom = (model.zoom / scale).coerceIn(.25f, 3f); model.view = "free"
                                        model.panX -= pan.x / 600; model.panY += pan.y / 600
                                    } else if (count == 1 && model.waterfall) {
                                        model.yaw -= pan.x * .3f; model.pitch = (model.pitch + pan.y * .3f).coerceIn(model.appearance.pitchMin, model.appearance.pitchMax)
                                        model.fov = model.appearance.freeFov; model.view = "free"
                                    }
                                    event.changes.forEach { if (it.position != it.previousPosition) it.consume() }
                                } while (event.changes.any { it.pressed })
                                saveSettings(prefs)
                            }
                        }.pointerInput(Unit) { detectTapGestures(onDoubleTap = { model.chooseView(if (model.waterfall) "perspective" else "front") }) })
                        if (!bare && overlay) {
                            if (panel) Panel(Color(0xd9111214))
                            else Button("›", modifier = Modifier.align(Alignment.TopStart).background(Color(0xb3111214), RoundedCornerShape(3.dp))) { panel = true; save() }
                        }
                    }
                    // At most 40 % of the height with its head included; the readings scroll inside.
                    if (!bare) Column(Modifier.fillMaxWidth().heightIn(max = monitorMax).background(Color(0xff111214))) {
                        @Suppress("UNUSED_VARIABLE") val tick = refresh
                        // The monitor's head as on the web: title, "frozen", and the fold toggle on the right.
                        Row(Modifier.fillMaxWidth().padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(t("monitor"), color = Color(0xff8e97a4), fontSize = 11.sp)
                            if (model.frozen) Text("●  ${t("mon.frozen")}", color = Color(0xffe8a246), fontSize = 11.sp, modifier = Modifier.padding(start = 12.dp))
                            Spacer(Modifier.weight(1f))
                            if (!hud) Text("${model.fps.roundToInt()} fps    ${t("mon.rowsPerSec").replace("{rows}", "${model.rowsPerSecond.roundToInt()}")}", color = Color(0xff8e97a4), fontSize = 10.sp, fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace)
                            Button(if (hud) "⌄" else "⌃") { hud = !hud; save() }
                        }
                        if (hud) Box(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
                            MonitorDock(monitorGroups(::t, memory), memory, memoryNow, Modifier.padding(start = 12.dp, end = 12.dp, bottom = 8.dp))
                        }
                    }
                }
            }
            if (bare) TextButton(onClick = { bare = false; save() }, modifier = Modifier.align(Alignment.TopStart).background(Color(0xff17191d), RoundedCornerShape(3.dp))) { Text(t("restoreUi"), color = Color(0xfff0bb68)) }
            if (model.error.isNotEmpty()) Text(model.error, color = Color(0xfff0bb68), fontSize = 11.sp, modifier = Modifier.align(Alignment.BottomCenter).background(Color(0xff17191d)).padding(8.dp))
        }
    }
    /** The monitor groups of the web and desktop apps, from what the Android renderer measures. */
    private fun monitorGroups(t: (String) -> String, memory: MemoryHistory): List<MonitorGroup> {
        val ms = { v: Float -> "%.2f ms".format(v) }
        val mb = { bytes: Long? -> bytes?.let { "${(it / 1048576.0).roundToInt()} MB" } ?: "—" }
        val msaa = if (model.msaaSamples > 1) " ${model.msaaSamples}x" else ""
        val audio = if (model.source == "off") MonitorGroup("audio", t("monAudio"), "", "—", listOf("" to t("mon.notStarted")))
        else MonitorGroup("audio", t("monAudio"), "", "%.1f kHz".format(model.sampleRate / 1000f), listOf(
            "" to t("mon.rowsPerSec").replace("{rows}", "${model.rowsPerSecond.roundToInt()}"),
            "FFT" to "${model.fftRowsPerSecond.roundToInt()}/s",
            t("mon.lat") to "${model.latencyMs.roundToInt()} ms",
            "" to "${model.rate}×${model.sub}",
        ))
        return listOf(
            MonitorGroup("frame", t("monFrame"), "", "${model.fps.roundToInt()} fps",
                listOf(t("mon.avg") to if (model.fps > 0) "%.1f ms".format(1000 / model.fps) else "—")),
            MonitorGroup("gpu", "GPU", model.rendererName, model.gpuMs?.let(ms) ?: "—", listOf(
                "" to "${model.viewportWidth}×${model.viewportHeight}$msaa",
                t("mon.verts") to "${model.vertices / 1000}k",
            )),
            MonitorGroup("cpu", "CPU", t("mon.cores").replace("{n}", "${Runtime.getRuntime().availableProcessors()}"), ms(model.drawMs),
                listOf(t("mon.analysis") to ms(model.analysisMs))),
            MonitorGroup("memory", t("mon.memTitle"), t("mon.resident"), mb(memory.resident), listOf(t("mon.peak") to mb(memory.peak))),
            audio,
        )
    }
    override fun onResume() {
        super.onResume(); spectrumView?.onResume()
        // Another instance's onDestroy may have cleared the session; CaptureService reads it.
        DemoSession.active = model
        val source = pausedSource; pausedSource = null
        when (source) {
            "test" -> model.startTest()
            "mic", "usb" -> if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) model.startMicrophone(pausedDevice)
        }
    }
    override fun onPause() { saveSettings(); spectrumView?.onPause(); super.onPause() }
    // Playback capture stays in the demo's explicitly started foreground service. Other sources
    // release their hardware while invisible, then resume without requesting permissions again.
    override fun onStop() {
        if (model.source in listOf("test", "mic", "usb")) {
            pausedSource = model.source; pausedDevice = model.activeDeviceId; model.stop()
        }
        super.onStop()
    }
    override fun onDestroy() {
        controller.dispose(); stopService(Intent(this, CaptureService::class.java))
        if (DemoSession.active === model) DemoSession.active = null
        super.onDestroy()
    }
}
