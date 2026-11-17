package io.github.lawchli.spectrum

import android.opengl.GLES30.*
import android.opengl.GLSurfaceView
import android.opengl.Matrix
import org.json.JSONObject
import java.nio.*
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10
import kotlin.math.*

private fun floats(data: FloatArray): FloatBuffer = ByteBuffer.allocateDirect(data.size * 4).order(ByteOrder.nativeOrder()).asFloatBuffer().apply { put(data); position(0) }
private fun ints(data: IntArray): IntBuffer = ByteBuffer.allocateDirect(data.size * 4).order(ByteOrder.nativeOrder()).asIntBuffer().apply { put(data); position(0) }
// `context` is the EGL context generation the names belong to; names from a lost context are never deleted.
private class Geometry(val vao: Int, val vbo: Int, val count: Int, val ibo: Int, val data: FloatArray, val upload: FloatBuffer, val context: Int)
private class Program(val id: Int) {
    private val locations = HashMap<String, Int>()
    fun loc(name: String) = locations.getOrPut(name) { glGetUniformLocation(id, name) }
    fun f(name: String, v: Float) = glUniform1f(loc(name), v)
    fun vec(name: String, x: Float, y: Float) = glUniform2f(loc(name), x, y)
    fun vec(name: String, x: Float, y: Float, z: Float) = glUniform3f(loc(name), x, y, z)
    fun vec(name: String, x: Float, y: Float, z: Float, w: Float) = glUniform4f(loc(name), x, y, z, w)
    fun color(name: String, color: FloatArray) = glUniform3f(loc(name), color[0], color[1], color[2])
}

class SpectrumRenderer(private val model: SpectrumModel) : GLSurfaceView.Renderer {
    private val look = model.appearance
    private val width = look.width
    private var height = look.height
    private val baseHeight = look.height
    private val depth = look.depth
    private val tail = look.tail
    private val floorDb = look.floorDb
    private val exposure = look.exposure
    private val density = model.context.resources.displayMetrics.density
    private var contextGeneration = 0
    // Copied under model.lock each frame, so GL calls never run while the analysis thread waits for the lock.
    private var shownCopy = FloatArray(0); private var peakCopy = FloatArray(0); private var maxCopy = FloatArray(0)
    private val dirtyCopy = IntArray(6)
    private lateinit var surface: Program; private lateinit var front: Program; private lateinit var lines: Program; private lateinit var keyProgram: Program
    private lateinit var mesh: Geometry; private lateinit var guides: Geometry; private lateinit var keys: Geometry
    private lateinit var live: Geometry; private lateinit var peaks: Geometry; private lateinit var curtain: Geometry; private lateinit var maximum: Geometry
    private var historyTexture = 0; private var paletteTexture = 0
    private var activeHistory: History? = null; private var palette = ""
    private var meshRate = -1; private var meshSub = -1; private var meshSeconds = -1f
    private var meshFloor = Float.NaN; private var meshHeight = Float.NaN
    private var frontVersion = -1L; private var maxVersion = -1L
    private var frontColors = look.frontColors.getValue(model.palette)
    private var viewportW = 1; private var viewportH = 1; private var cols = 253
    private val vp = FloatArray(16); private val inv = FloatArray(16)
    private var last = 0L; private var statsStart = 0L; private var frames = 0
    private val labelMatrix = FloatArray(16)
    private var labelSeconds = -1f; private var labelFloor = Float.NaN
    private var labelPiano = false; private var labelWaterfall = false
    private var labelPeakEnabled = false; private var labelPeakText = ""
    // Labels are in viewport pixels: a resize that keeps the aspect (split screen, a freeform
    // window) leaves the matrix as it was but still moves every label.
    private var labelW = 0; private var labelH = 0
    private var currentYaw = model.yaw; private var currentPitch = model.pitch; private var currentFov = model.fov; private var currentZoom = model.zoom
    private var currentX = model.panX; private var currentY = model.panY; private var currentZ = model.panZ
    private val cameraView = FloatArray(16); private val cameraProjection = FloatArray(16)
    private val timerScratch = IntArray(1)
    private var guideRanges = mutableMapOf<String, Pair<Int, Int>>()
    private var pendingQueries = java.util.ArrayDeque<Int>()
    private var timer = false
    private var gpuMs: Float? = null
        set(value) { field = value; model.gpuMs = value }
    private var samples = 0
    private var lastLatencyRow = -1L
    private var litKey = -2
    private val cameraVelocity = FloatArray(7)
    private fun shader(name: String): String {
        val raw = model.context.assets.open("spectrum/shaders/$name").bufferedReader().readText()
        return Regex("#include \"([^\"]+)\"").replace(raw) { model.context.assets.open("spectrum/shaders/${it.groupValues[1]}").bufferedReader().readText() }
    }
    private fun program(vert: String, frag: String): Program {
        fun compile(type: Int, name: String): Int { val s = glCreateShader(type); glShaderSource(s, shader(name)); glCompileShader(s)
            val ok = IntArray(1); glGetShaderiv(s, GL_COMPILE_STATUS, ok, 0); check(ok[0] != 0) { "$name: ${glGetShaderInfoLog(s)}" }; return s }
        val v = compile(GL_VERTEX_SHADER, vert); val f = compile(GL_FRAGMENT_SHADER, frag); val p = glCreateProgram()
        glAttachShader(p, v); glAttachShader(p, f); glLinkProgram(p); glDeleteShader(v); glDeleteShader(f)
        val ok = IntArray(1); glGetProgramiv(p, GL_LINK_STATUS, ok, 0); check(ok[0] != 0) { glGetProgramInfoLog(p) }; return Program(p)
    }
    private fun geometry(data: FloatArray, components: IntArray, indices: IntArray? = null): Geometry {
        val ids = IntArray(3); glGenVertexArrays(1, ids, 0); glGenBuffers(1, ids, 1)
        glBindVertexArray(ids[0]); glBindBuffer(GL_ARRAY_BUFFER, ids[1]); glBufferData(GL_ARRAY_BUFFER, data.size * 4, floats(data), GL_DYNAMIC_DRAW)
        val stride = components.sum() * 4; var offset = 0
        components.forEachIndexed { i, count -> glEnableVertexAttribArray(i); glVertexAttribPointer(i, count, GL_FLOAT, false, stride, offset); offset += count * 4 }
        if (indices != null) { glGenBuffers(1, ids, 2); glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, ids[2]); glBufferData(GL_ELEMENT_ARRAY_BUFFER, indices.size * 4, ints(indices), GL_STATIC_DRAW) }
        glBindVertexArray(0); return Geometry(ids[0], ids[1], indices?.size ?: (data.size * 4 / stride), ids[2], data, floats(data), contextGeneration)
    }
    // After a lost context the old names may already belong to new objects, so only delete our own.
    private fun delete(g: Geometry) { if (g.context != contextGeneration) return; glDeleteVertexArrays(1, intArrayOf(g.vao), 0); glDeleteBuffers(2, intArrayOf(g.vbo, g.ibo), 0) }
    override fun onSurfaceCreated(unused: GL10?, config: EGLConfig?) {
        contextGeneration++
        try {
            surface = program("terrain.vert", "surface.frag"); front = program("front.vert", "front.frag"); lines = program("line.vert", "line.frag"); keyProgram = program("keys.vert", "keys.frag")
            val tex = IntArray(2); glGenTextures(2, tex, 0); historyTexture = tex[0]; paletteTexture = tex[1]
            activeHistory = null; palette = ""; meshRate = -1; frontVersion = -1; maxVersion = -1; litKey = -2; pendingQueries.clear()
            val k = FloatArray(127 * 6 * 4); val corners = arrayOf(-.5f to 0f, .5f to 0f, -.5f to 1f, .5f to 0f, .5f to 1f, -.5f to 1f)
            for (b in 0..126) corners.forEachIndexed { i, (x, y) -> val j = (b * 6 + i) * 4; k[j] = b + x; k[j + 1] = y; k[j + 2] = b.toFloat() }
            keys = geometry(k, intArrayOf(1, 1, 1, 1))
            model.rendererName = glGetString(GL_RENDERER) ?: "GLES30"
            val sampleCount = IntArray(1); glGetIntegerv(GL_SAMPLES, sampleCount, 0); samples = sampleCount[0]; model.msaaSamples = samples
            timer = glGetString(GL_EXTENSIONS)?.contains("GL_EXT_disjoint_timer_query") == true
            last = System.nanoTime(); statsStart = last
        } catch (e: Exception) { model.error = e.message ?: e.toString(); throw e }
    }
    override fun onSurfaceChanged(unused: GL10?, w: Int, h: Int) { viewportW = w; viewportH = h; model.viewportWidth = w; model.viewportHeight = h; glViewport(0, 0, w, h) }
    private fun allocateHistory(h: History) {
        activeHistory = h
        glActiveTexture(GL_TEXTURE0); glBindTexture(GL_TEXTURE_2D, historyTexture); glPixelStorei(GL_UNPACK_ALIGNMENT, 1)
        glTexImage2D(GL_TEXTURE_2D, 0, GL_R8, h.count * 6, h.rows, 0, GL_RED, GL_UNSIGNED_BYTE, null)
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR); glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR)
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE); glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_REPEAT)
        dirtyCopy.fill(h.rows)
        if (::live.isInitialized) { delete(live); delete(peaks); delete(curtain); delete(maximum) }
        fun buffer(per: Int) = geometry(FloatArray(h.count * per * 3), intArrayOf(1, 1, 1))
        live = buffer(1); peaks = buffer(1); maximum = buffer(1); curtain = buffer(2); meshRate = -1; frontVersion = -1; maxVersion = -1
    }
    // Rows before the `written` snapshot are complete; the analysis thread only writes after it.
    private fun uploadHistory(h: History, written: Long) {
        glActiveTexture(GL_TEXTURE0); glBindTexture(GL_TEXTURE_2D, historyTexture)
        for (l in 0..5) {
            val n = min(h.rows, dirtyCopy[l]); if (n == 0) continue
            val end = (written / (1 shl l) % h.rows).toInt(); val start = (end - n + h.rows) % h.rows; val first = min(n, h.rows - start)
            fun upload(row: Int, count: Int) { val data = ByteBuffer.wrap(h.levels[l], row * h.count, count * h.count); glTexSubImage2D(GL_TEXTURE_2D, 0, l * h.count, row, h.count, count, GL_RED, GL_UNSIGNED_BYTE, data) }
            upload(start, first); if (first < n) upload(0, n - first); dirtyCopy[l] = 0
        }
    }
    private fun buildMesh() {
        if (::mesh.isInitialized) delete(mesh)
        val near = (model.seconds * model.rate).roundToInt(); val span = (near * tail).roundToInt(); var step = 1
        while (step < 32 && near / step > 360) step *= 2
        fun stepAt(age: Int) = if (age < near) step else min(32, step * 2.0.pow(floor(log2(age.toDouble() / near)) + 1).toInt())
        val ages = mutableListOf(0); while (ages.last() < span) ages.add(ages.last() + stepAt(ages.last()))
        cols = 126 * max(2, model.sub) + 1
        val data = FloatArray(cols * ages.size * 3)
        for (r in ages.indices) for (c in 0 until cols) { val i = (r * cols + c) * 3; data[i] = c.toFloat() / (cols - 1); data[i + 1] = ages[r].toFloat(); data[i + 2] = stepAt(ages[r]).toFloat() }
        val indices = IntArray((ages.size - 1) * (cols - 1) * 6); var p = 0
        for (r in 0 until ages.size - 1) for (c in 0 until cols - 1) { val i = r * cols + c; for (v in intArrayOf(i, i + 1, i + cols, i + 1, i + cols + 1, i + cols)) indices[p++] = v }
        mesh = geometry(data, intArrayOf(3), indices); model.vertices = cols * ages.size
        buildGuides()
    }
    private fun buildGuides() {
        if (::guides.isInitialized) delete(guides)
        val v = mutableListOf<Float>(); guideRanges.clear(); val floor = if (model.lowLevel) -120f else floorDb
        fun seg(ax: Float, ay: Float, az: Float, bx: Float, by: Float, bz: Float) { v.addAll(listOf(ax, ay, az, bx, by, bz)) }
        fun group(name: String, block: () -> Unit) { val start = v.size / 3; block(); guideRanges[name] = start to (v.size / 3 - start) }
        val x0 = -width / 2; val x1 = width / 2; val zf = depth / 2; val end = zf - depth * tail
        group("floor") { for (hz in look.hz) { val x = ((57 + 12 * log2(hz / 440.0)) / 126 - .5).toFloat() * width; seg(x, .0006f * height, zf, x, .0006f * height, end) }; for (tick in 1..(model.seconds * tail / look.secondsStep).toInt()) { val z = zf - tick * look.secondsStep / model.seconds * depth; seg(x0, 0f, z, x1, 0f, z) } }
        group("rails") { seg(x0, 0f, zf, x1, 0f, zf); seg(x0, 0f, zf, x0, 0f, end); seg(x1, 0f, zf, x1, 0f, end) }
        group("spine") { seg(x0, 0f, zf, x0, height, zf); for (db in look.db.filter { it >= floor } + if (floor.toInt() !in look.db) listOf(floor.toInt()) else emptyList()) { val y = (db - floor) / -floor * height; seg(x0, y, zf, x0 - width * .015f, y, zf) } }
        group("front") { for (db in -24 downTo floor.toInt() + 1 step 24) { val y = (db - floor) / -floor * height; seg(x0, y, zf, x1, y, zf) } }
        guides = geometry(v.toFloatArray(), intArrayOf(3))
    }
    private fun setPalette() {
        palette = model.palette; frontColors = look.frontColors.getValue(palette); glActiveTexture(GL_TEXTURE1); glBindTexture(GL_TEXTURE_2D, paletteTexture)
        val bytes = model.context.assets.open("spectrum/palette-$palette.rgba").readBytes()
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, 256, 1, 0, GL_RGBA, GL_UNSIGNED_BYTE, ByteBuffer.wrap(bytes))
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR); glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR)
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE); glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE)
    }
    // `written` is this frame's snapshot: u_newest never points at a row the texture does not have yet.
    private fun common(p: Program, written: Long) {
        glUseProgram(p.id); p.f("u_rows", 4096f); p.f("u_sub", model.sub.toFloat()); p.f("u_rowScale", model.rate / 60f)
        p.f("u_newest", ((written - 1 + 131072) % 131072).toFloat()); p.f("u_phase", if (model.frozen) model.phase else ((System.nanoTime() - model.lastAudioNano) / 1e9 * model.rate).toFloat().coerceIn(0f, 1f))
        p.f("u_spanRows", model.seconds * model.rate * tail); p.vec("u_dbRange", if (model.lowLevel) -120f else floorDb, 0f)
        p.vec("u_size", width, height, depth); p.f("u_tail", tail); p.f("u_fogStart", look.fogStart); p.f("u_fogPower", look.fogPower)
        glUniformMatrix4fv(p.loc("u_viewProj"), 1, false, vp, 0); glUniformMatrix4fv(p.loc("u_invViewProj"), 1, false, inv, 0)
        p.vec("u_viewport", viewportW.toFloat(), viewportH.toFloat()); p.f("u_dpr", density)
        glActiveTexture(GL_TEXTURE0); glBindTexture(GL_TEXTURE_2D, historyTexture); glUniform1i(p.loc("u_history"), 0)
        glActiveTexture(GL_TEXTURE1); glBindTexture(GL_TEXTURE_2D, paletteTexture); glUniform1i(p.loc("u_palette"), 1)
    }
    private fun camera(dt: Float) {
        fun spring(current: Float, target: Float, index: Int): Float {
            val omega = 2f / (if (model.view == "free") look.dragSeconds else look.presetSeconds)
            val x = omega * dt; val decay = 1 / (1 + x + .48f * x * x + .235f * x * x * x)
            val change = current - target; val temp = (cameraVelocity[index] + omega * change) * dt
            cameraVelocity[index] = (cameraVelocity[index] - omega * temp) * decay
            return target + (change + temp) * decay
        }
        val flat = !model.waterfall
        val frontPreset = look.views.getValue("front")
        val targetYaw = if (flat) frontPreset.yaw else model.yaw
        currentYaw = spring(currentYaw, targetYaw + 360 * round((currentYaw - targetYaw) / 360), 0)
        currentPitch = spring(currentPitch, if (flat) frontPreset.pitch else model.pitch, 1)
        currentFov = exp(spring(ln(currentFov), ln(if (flat) frontPreset.fov else model.fov), 2))
        currentZoom = exp(spring(ln(currentZoom), ln(if (flat) frontPreset.zoom else model.zoom), 3))
        currentX = spring(currentX, if (flat) frontPreset.pan[0] else model.panX, 4)
        currentY = spring(currentY, if (flat) frontPreset.pan[1] else model.panY, 5)
        currentZ = spring(currentZ, if (flat) frontPreset.pan[2] else model.panZ, 6)
        val yaw = Math.toRadians(currentYaw.toDouble()); val pitch = Math.toRadians(currentPitch.toDouble())
        val back = doubleArrayOf(cos(pitch) * sin(yaw), sin(pitch), cos(pitch) * cos(yaw)); val right = doubleArrayOf(cos(yaw), 0.0, -sin(yaw))
        val up = doubleArrayOf(back[1] * right[2], back[2] * right[0] - back[0] * right[2], -back[1] * right[0])
        val tanV = tan(Math.toRadians(currentFov.toDouble()) / 2); val tanH = tanV * viewportW / viewportH; var dist = 0.0
        for (ix in -1..1 step 2) for (iy in -1..1 step 2) for (iz in -1..1 step 2) {
            val x = ix * width / 2; val y = iy * height / 2; val z = iz * depth / 2
            val px = x * right[0] + y * right[1] + z * right[2]; val py = x * up[0] + y * up[1] + z * up[2]; val toward = x * back[0] + y * back[1] + z * back[2]
            dist = max(dist, max(abs(px) / tanH + toward, abs(py) / tanV + toward))
        }; dist *= 1.06 * currentZoom
        val radius = sqrt((width * width + height * height + depth * depth).toDouble()) / 2; val near = max(dist * .01, dist - radius * 2.5); val far = dist + radius * 2.5 + depth * tail
        val view = cameraView; val projection = cameraProjection
        val cx = currentX; val cy = height / 2 + currentY; val cz = currentZ
        val ux = if (currentPitch > 89) -sin(yaw).toFloat() else 0f; val uy = if (currentPitch > 89) 0f else 1f; val uz = if (currentPitch > 89) -cos(yaw).toFloat() else 0f
        Matrix.setLookAtM(view, 0, cx + (back[0] * dist).toFloat(), cy + (back[1] * dist).toFloat(), cz + (back[2] * dist).toFloat(), cx, cy, cz, ux, uy, uz)
        Matrix.perspectiveM(projection, 0, currentFov, viewportW.toFloat() / viewportH, near.toFloat(), far.toFloat()); Matrix.multiplyMM(vp, 0, projection, 0, view, 0); Matrix.invertM(inv, 0, vp, 0)
        vp.copyInto(model.cameraMatrix)
    }
    private fun copy(src: FloatArray, dst: FloatArray) = if (dst.size == src.size) src.copyInto(dst) else src.clone()
    private fun updateFront(g: Geometry, db: FloatArray, per: Int) {
        val data = g.data
        for (k in db.indices) for (j in 0 until per) { val p = (k * per + j) * 3; data[p] = k.toFloat() / model.sub; data[p + 1] = db[k]; data[p + 2] = if (per == 2) 1f - j else 1f }
        glBindBuffer(GL_ARRAY_BUFFER, g.vbo); g.upload.position(0); g.upload.put(data); g.upload.position(0); glBufferSubData(GL_ARRAY_BUFFER, 0, data.size * 4, g.upload)
    }
    override fun onDrawFrame(unused: GL10?) {
        if (!::surface.isInitialized) return
        val now = System.nanoTime(); val dt = ((now - last) / 1e9f).coerceIn(.001f, .1f); last = now; frames++
        // Only copies happen under the lock; texture and buffer uploads can block in the driver and
        // would otherwise hold up the analysis thread, which takes the same lock for every row.
        val spectrumOn = model.spectrum; val maxOn = model.maxHold
        val h: History; val written: Long; val version: Long
        synchronized(model.lock) {
            h = model.history; written = h.written; version = model.displayVersion
            for (l in 0..5) { dirtyCopy[l] += h.dirty[l]; h.dirty[l] = 0 }
            if (spectrumOn && (frontVersion != version || activeHistory !== h)) { shownCopy = copy(model.shown, shownCopy); peakCopy = copy(model.peak, peakCopy) }
            if (spectrumOn && maxOn && (maxVersion != version || activeHistory !== h)) maxCopy = copy(model.maximum, maxCopy)
        }
        if (written != lastLatencyRow && model.rowTimestampNano > 0) {
            lastLatencyRow = written
            model.latencyMs = model.latencyMs * .9f + ((now - model.rowTimestampNano) / 1e6f).coerceAtLeast(0f) * .1f
        }
        if (activeHistory !== h) allocateHistory(h)
        uploadHistory(h, written)
        if (spectrumOn && frontVersion != version) {
            updateFront(live, shownCopy, 1); updateFront(peaks, peakCopy, 1); updateFront(curtain, shownCopy, 2)
            frontVersion = version
        }
        if (spectrumOn && maxOn && maxVersion != version) { updateFront(maximum, maxCopy, 1); maxVersion = version }
        val newHeight = if (!model.waterfall) min(1.5f, max(baseHeight, width * viewportH / viewportW * .84f)) else baseHeight
        val newFloor = if (model.lowLevel) -120f else floorDb
        if (meshRate != model.rate || meshSub != model.sub || meshSeconds != model.seconds || meshFloor != newFloor || meshHeight != newHeight) {
            meshRate = model.rate; meshSub = model.sub; meshSeconds = model.seconds; meshFloor = newFloor; meshHeight = newHeight
            height = newHeight; buildMesh()
        }
        if (palette != model.palette) setPalette()
        camera(dt)
        glClearColor(0f, 0f, 0f, 1f); glClear(GL_COLOR_BUFFER_BIT or GL_DEPTH_BUFFER_BIT); glEnable(GL_DEPTH_TEST); glDepthFunc(GL_LEQUAL); glDepthMask(true)
        var query = 0
        if (timer) {
            val disjoint = IntArray(1); glGetIntegerv(0x8FBB, disjoint, 0)
            while (pendingQueries.isNotEmpty()) { val id = pendingQueries.first(); val available = IntArray(1); glGetQueryObjectuiv(id, GL_QUERY_RESULT_AVAILABLE, available, 0); if (available[0] == 0) break
                pendingQueries.removeFirst(); val result = IntArray(1); glGetQueryObjectuiv(id, GL_QUERY_RESULT, result, 0); if (disjoint[0] == 0) gpuMs = (result[0].toLong() and 0xffffffffL) / 1e6f; glDeleteQueries(1, intArrayOf(id), 0) }
            if (pendingQueries.size < 8) { val ids = IntArray(1); glGenQueries(1, ids, 0); query = ids[0]; glBeginQuery(0x88BF, query) }
        }
        val top = ((currentPitch - 62) / 24).coerceIn(0f, 1f); val frontness = if (currentFov < 3 && currentPitch < 5) 1f else 0f
        if (model.waterfall) {
            val p = surface; common(p, written); p.f("u_stepU", 1f / (cols - 1)); p.f("u_lift", 0f); p.vec("u_background", 0f, 0f, 0f)
            p.color("u_keyLight", look.keyLight); p.color("u_rimLight", look.rimLight)
            p.f("u_ambient", look.ambient); p.f("u_exposure", exposure)
            for ((uniform, value) in look.styles.getValue(model.style).uniforms) p.f(uniform, value)
            p.f("u_registerTint", look.registerTint.getValue(palette)); p.f("u_topness", top); p.f("u_frontness", frontness)
            glDisable(GL_BLEND); glEnable(GL_POLYGON_OFFSET_FILL); glPolygonOffset(1f, 1f); glBindVertexArray(mesh.vao); glDrawElements(GL_TRIANGLES, mesh.count, GL_UNSIGNED_INT, 0); glDisable(GL_POLYGON_OFFSET_FILL)
        }
        common(lines, written); glEnable(GL_BLEND); glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA); glDepthMask(false); glBindVertexArray(guides.vao)
        fun guide(name: String, color: String, alpha: Float) { val range = guideRanges[name] ?: return; if (alpha <= .002f) return; lines.color("u_color", look.guideColors.getValue(color)); lines.f("u_alpha", alpha); glDrawArrays(GL_LINES, range.first, range.second) }
        // Same colours and alphas as renderer.js; the spine fades with pitch instead of its fadeDb.
        if (model.waterfall) guide("floor", "floorColor", (look.floorAlpha3d + (look.floorAlphaTop - look.floorAlpha3d) * top) * (1 - frontness))
        guide("rails", "railColor", look.railAlpha); if (top < .98f) guide("spine", "spineColor", look.spineAlpha * cos(currentPitch * PI.toFloat() / 180)); guide("front", "frontGridColor", look.frontGridAlpha * frontness)
        if (model.spectrum) {
            common(front, written); front.f("u_lift", height * .0025f); front.f("u_exposure", exposure); front.f("u_frontness", frontness); front.f("u_topness", top)
            front.color("u_traceColor", frontColors.trace); front.color("u_peakColor", frontColors.peak)
            front.color("u_headroomColor", frontColors.headroom); front.color("u_maxColor", frontColors.maximum)
            glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA)
            fun draw(mode: Float, g: Geometry, primitive: Int) { front.f("u_mode", mode); glBindVertexArray(g.vao); glDrawArrays(primitive, 0, g.count) }
            draw(0f, curtain, GL_TRIANGLE_STRIP); if (top < .98f) { if (model.maxHold) draw(3f, maximum, GL_LINE_STRIP); draw(1f, peaks, GL_LINE_STRIP) }; draw(2f, live, GL_LINE_STRIP)
        }
        if (model.piano) {
            val wanted = if (model.peakLabels) model.pianoBand else -1
            if (wanted != litKey) {
                litKey = wanted
                for (k in 0..126) for (j in 0..5) keys.data[(k * 6 + j) * 4 + 3] = if (k == wanted) 1f else 0f
                keys.upload.position(0); keys.upload.put(keys.data); keys.upload.position(0)
                glBindBuffer(GL_ARRAY_BUFFER, keys.vbo); glBufferSubData(GL_ARRAY_BUFFER, 0, keys.data.size * 4, keys.upload)
            }
            common(keyProgram, written)
            keyProgram.f("u_keyGapPx", look.keyGap * density)
            keyProgram.f("u_keyHeightPx", (if (viewportH / density < 300) look.keyHeightSmall else look.densityIndependentKeyHeight) * density)
            keyProgram.f("u_hoverBand", -10f); keyProgram.f("u_alpha", look.keyAlpha)
            for ((uniform, color) in look.keyColors) keyProgram.color(uniform, color)
            glDisable(GL_DEPTH_TEST); glBindVertexArray(keys.vao); glDrawArrays(GL_TRIANGLES, 0, keys.count)
        }
        glBindVertexArray(0); glDepthMask(true)
        if (query != 0) { glEndQuery(0x88BF); pendingQueries.add(query) }
        if (!labelMatrix.contentEquals(vp) || labelW != viewportW || labelH != viewportH || labelSeconds != model.seconds || labelFloor != newFloor || labelPiano != model.piano || labelWaterfall != model.waterfall || labelPeakEnabled != model.peakLabels || labelPeakText != model.peakText) {
            vp.copyInto(labelMatrix); labelW = viewportW; labelH = viewportH; labelSeconds = model.seconds; labelFloor = newFloor
            labelPiano = model.piano; labelWaterfall = model.waterfall
            labelPeakEnabled = model.peakLabels; labelPeakText = model.peakText; updateLabels()
        }
        model.drawMs = model.drawMs * .9f + (System.nanoTime() - now) / 1e6f * .1f
        if (now - statsStart >= 1000000000) {
            model.fps = frames * 1e9f / (now - statsStart); frames = 0; statsStart = now
            android.util.Log.i("SpectrumBench", JSONObject().put("source", model.source).put("canvas", "$viewportW×$viewportH").put("msaa", samples).put("captureToSubmitMs", model.latencyMs).put("fps", model.fps).put("drawMs", model.drawMs).put("gpuMs", gpuMs ?: JSONObject.NULL).put("analysisMs", model.analysisMs).put("rows", written).put("rowRate", model.rowsPerSecond).put("fftRowRate", model.fftRowsPerSecond).put("analysisStride", model.analysisStride).put("tier", "${model.rate}x${model.sub}").put("vertices", model.vertices).put("heap", Runtime.getRuntime().totalMemory() - Runtime.getRuntime().freeMemory()).toString())
        }
        val error = glGetError(); if (error != GL_NO_ERROR) model.error = "GL error 0x${error.toString(16)}"
    }
    private fun updateLabels() {
        val labels = mutableListOf<AxisLabel>(); val p = FloatArray(4)
        fun add(text: String, x: Float, y: Float, z: Float, alpha: Float = 1f, kind: String = "hz") {
            Matrix.multiplyMV(p, 0, vp, 0, floatArrayOf(x, y, z, 1f), 0); if (p[3] <= 0) return
            val sx = (p[0] / p[3] * .5f + .5f) * viewportW; val sy = (1 - (p[1] / p[3] * .5f + .5f)) * viewportH
            val density = model.context.resources.displayMetrics.density
            val textW = max(12f, text.length * 5.5f * density); val textH = 12f * density
            val px = (sx + if (kind == "db") -textW - 4 else if (kind == "time") 6f else -textW / 2).coerceIn(2f, max(2f, viewportW - textW - 2))
            val py = (sy + if (kind == "hz") 5f * density + if (model.piano) 14f * density else 0f else -textH / 2).coerceIn(2f, max(2f, viewportH - textH - 2))
            val collision = labels.any { old ->
                val oldW = max(12f, old.text.length * 5.5f * density)
                px < old.x + oldW + 4 && px + textW + 4 > old.x && py < old.y + textH && py + textH > old.y
            }
            if (!collision) labels.add(AxisLabel(text, px, py, alpha))
        }
        val compact = viewportW < 600
        for (hz in if (compact) look.hzCompact else look.hz) {
            val u = (57 + 12 * log2(hz / 440.0)) / 126; add(if (hz >= 1000) "${hz / 1000}k" else "$hz", ((u - .5) * width).toFloat(), 0f, depth / 2 + depth * .026f)
        }
        val floor = if (model.lowLevel) -120 else floorDb.toInt()
        if (currentPitch < 78) for (db in ((if (compact) look.dbCompact else look.db).filter { it >= floor } + if (floor !in look.db) listOf(floor) else emptyList())) add(if (db == 0) "0 dB" else "−${-db}", -width / 2 - width * .018f, (db - floor).toFloat() / -floor * height, depth / 2, kind = "db")
        if (model.waterfall) for (tick in 1..(model.seconds / look.secondsStep).toInt()) {
            val seconds = tick * look.secondsStep
            val value = if (seconds == seconds.toInt().toFloat()) seconds.toInt().toString() else seconds.toString()
            add("−$value s", width / 2 + width * .017f, 0f, depth / 2 - seconds / model.seconds * depth, kind = "time")
        }
        model.publishLabels(labels)
    }
}
