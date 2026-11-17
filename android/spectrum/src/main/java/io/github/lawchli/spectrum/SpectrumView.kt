package io.github.lawchli.spectrum

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.opengl.GLSurfaceView
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.widget.FrameLayout

/** A View-based, permission-free host. Use AndroidView to embed this same component in Compose. */
@android.annotation.SuppressLint("ViewConstructor") // Explicit controller injection; intended for code/AndroidView, not XML inflation.
class SpectrumView(context: Context, val controller: SpectrumController, samples: Int = 0) : FrameLayout(context) {
    private class RenderSurface(context: Context) : GLSurfaceView(context) {
        override fun performClick(): Boolean { super.performClick(); return true }
    }
    private val surface = RenderSurface(context)
    private var singleTap = false
    var gesturesEnabled = true
    private val model = controller.model
    private var labels: List<AxisLabel> = emptyList()
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xff9ba4b0.toInt(); textSize = android.util.TypedValue.applyDimension(android.util.TypedValue.COMPLEX_UNIT_SP, 9f, resources.displayMetrics)
    }
    private val overlay = object : View(context) {
        override fun onDraw(canvas: Canvas) {
            for (label in labels) {
                paint.color = 0xff9ba4b0.toInt(); paint.alpha = (label.alpha.coerceIn(0f, 1f) * 255).toInt()
                canvas.drawText(label.text, label.x, label.y - paint.fontMetrics.ascent, paint)
            }
            if (model.peakLabels) {
                paint.color = 0xfff0bb68.toInt(); paint.alpha = 255
                val text = model.peakText
                canvas.drawText(text, (width - paint.measureText(text)) / 2, 18f * resources.displayMetrics.density, paint)
            }
        }
    }
    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent) = true
        override fun onSingleTapUp(e: MotionEvent): Boolean { singleTap = true; return true }
        override fun onDoubleTap(e: MotionEvent): Boolean { model.chooseView(if (model.waterfall) "perspective" else "front"); return true }
        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
            if (e2.pointerCount > 1) {
                model.panX += distanceX / 600; model.panY -= distanceY / 600
            } else if (model.waterfall) {
                model.yaw += distanceX * .3f
                model.pitch = (model.pitch - distanceY * .3f).coerceIn(model.appearance.pitchMin, model.appearance.pitchMax)
                model.fov = model.appearance.freeFov; model.view = "free"
            }
            return true
        }
    })
    private val scale = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScale(detector: ScaleGestureDetector): Boolean {
            model.zoom = (model.zoom / detector.scaleFactor).coerceIn(.25f, 3f); model.view = "free"; return true
        }
    })
    init {
        require(samples == 0 || samples == 4)
        setBackgroundColor(android.graphics.Color.BLACK)
        surface.setEGLContextClientVersion(3)
        surface.setEGLConfigChooser(SpectrumEglConfig(samples))
        surface.preserveEGLContextOnPause = true
        surface.setRenderer(SpectrumRenderer(model))
        surface.renderMode = GLSurfaceView.RENDERMODE_CONTINUOUSLY
        surface.setOnTouchListener { touched, event ->
            if (!gesturesEnabled) false else {
                parent?.requestDisallowInterceptTouchEvent(true)
                scale.onTouchEvent(event); gestures.onTouchEvent(event)
                if (singleTap) { singleTap = false; touched.performClick() }
                true
            }
        }
        addView(surface, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        addView(overlay, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
    }
    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        controller.attach(this)
        labels = model.positions
        model.onLabelsChanged = { labels = it; overlay.invalidate() }
        overlay.invalidate()
    }
    override fun onDetachedFromWindow() {
        if (controller.detach(this)) model.onLabelsChanged = null
        super.onDetachedFromWindow()
    }
    /** Forward the host lifecycle. Pausing drawing leaves the explicitly selected audio source alone. */
    fun onResume() { if (!controller.isDisposed) surface.onResume() }
    fun onPause() = surface.onPause()
}
