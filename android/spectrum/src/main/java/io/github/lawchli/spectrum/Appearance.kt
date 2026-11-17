package io.github.lawchli.spectrum

import android.content.Context
import org.json.JSONObject

/** Parsed once at startup. GLSL, look and translations are exported from the web source. */
class Appearance private constructor(private val json: JSONObject) {
    data class View(val yaw: Float, val pitch: Float, val fov: Float, val zoom: Float, val pan: FloatArray)
    data class Style(val label: String, val uniforms: List<Pair<String, Float>>)
    data class Front(val trace: FloatArray, val peak: FloatArray, val headroom: FloatArray, val maximum: FloatArray)
    private fun JSONObject.f(key: String) = getDouble(key).toFloat()
    private fun JSONObject.vector(key: String) = getJSONArray(key).let { a -> FloatArray(a.length()) { a.getDouble(it).toFloat() } }
    private fun JSONObject.color(key: String): FloatArray {
        val color = android.graphics.Color.parseColor(getString(key))
        return floatArrayOf(android.graphics.Color.red(color) / 255f, android.graphics.Color.green(color) / 255f, android.graphics.Color.blue(color) / 255f)
    }
    private fun JSONObject.ints(key: String) = getJSONArray(key).let { a -> IntArray(a.length()) { a.getInt(it) } }
    private val size = json.getJSONObject("size")
    val width = size.f("width")
    val height = size.f("height")
    val depth = size.f("depth")
    val tail = size.f("tail")
    val floorDb = json.getJSONObject("db").f("floor")
    val fogStart = json.getJSONObject("fog").f("start")
    val fogPower = json.getJSONObject("fog").f("power")
    private val surface = json.getJSONObject("surface")
    val exposure = surface.f("exposure")
    val ambient = surface.f("ambient")
    val keyLight = surface.vector("keyLight")
    val rimLight = surface.vector("rimLight")
    val densityIndependentKeyHeight = json.getJSONObject("keys").f("heightPx")
    val keyHeightSmall = json.getJSONObject("keys").f("heightPxSmall")
    val keyGap = json.getJSONObject("keys").f("gapPx")
    val keyAlpha = json.getJSONObject("keys").f("alpha")
    val keyColors = listOf("whiteKey" to "white", "blackKey" to "black", "litKey" to "lit", "hoverKey" to "hover", "octaveEdge" to "octave")
        .map { (uniform, field) -> "u_$uniform" to json.getJSONObject("keys").color(field) }
    private val guides = json.getJSONObject("guides")
    val guideColors = listOf("floorColor", "railColor", "spineColor", "frontGridColor").associateWith { guides.color(it) }
    val floorAlpha3d = guides.f("floorAlpha3d")
    val floorAlphaTop = guides.f("floorAlphaTop")
    val railAlpha = guides.f("railAlpha")
    val spineAlpha = guides.f("spineAlpha")
    val frontGridAlpha = guides.f("frontGridAlpha")
    private val motion = json.getJSONObject("motion")
    val presetSeconds = motion.f("presetSeconds")
    val dragSeconds = motion.f("dragSeconds")
    val pitchMin = motion.f("pitchMin")
    val pitchMax = motion.f("pitchMax")
    val freeFov = json.f("freeFov")
    private val labels = json.getJSONObject("labels")
    val hz = labels.ints("hz")
    val hzCompact = labels.ints("hzCompact")
    val db = labels.ints("db")
    val dbCompact = labels.ints("dbCompact")
    val secondsStep = labels.f("seconds")
    val views: Map<String, View> = json.getJSONObject("views").let { obj -> obj.keys().asSequence().associateWith { name ->
        obj.getJSONObject(name).let { v -> View(v.f("yaw"), v.f("pitch"), v.f("fov"), v.f("zoom"), v.vector("pan")) }
    } }
    val styles: Map<String, Style> = json.getJSONObject("styles").let { obj -> obj.keys().asSequence().associateWith { name ->
        val style = obj.getJSONObject(name)
        Style(style.getString("label"), style.keys().asSequence().filter { it != "label" }.map { "u_$it" to style.f(it) }.toList())
    } }
    val paletteNames = json.getJSONObject("palettes").keys().asSequence().toList()
    val defaultPalette = json.getString("palette")
    val defaultStyle = json.getString("style")
    val registerTint = json.getJSONObject("paletteRegister").let { obj -> paletteNames.associateWith { obj.f(it) } }
    val variantStyles = paletteNames.associateWith { name -> json.getJSONObject("variants").optJSONObject(name)?.optString("style", defaultStyle) ?: defaultStyle }
    val frontColors = paletteNames.associateWith { name ->
        val base = json.getJSONObject("front")
        val variant = json.getJSONObject("variants").optJSONObject(name)?.optJSONObject("front")
        fun color(field: String) = if (variant?.has(field) == true) variant.color(field) else base.color(field)
        Front(color("trace"), color("peak"), color("headroom"), color("maxHold"))
    }
    companion object {
        fun load(context: Context) = Appearance(JSONObject(context.assets.open("spectrum/look.json").bufferedReader().use { it.readText() }))
    }
}
