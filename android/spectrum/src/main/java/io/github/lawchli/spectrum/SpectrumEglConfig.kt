package io.github.lawchli.spectrum

import android.opengl.GLSurfaceView
import javax.microedition.khronos.egl.EGL10
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.egl.EGLDisplay

/** 0x by default. A 4x request falls back to 0x when the GPU offers no such config. Adreno 540 (driver 2020-06) faults with 4x once the surface widens, so 4x is opt-in. */
class SpectrumEglConfig(private val requestedSamples: Int) : GLSurfaceView.EGLConfigChooser {
    override fun chooseConfig(egl: EGL10, display: EGLDisplay): EGLConfig {
        for (samples in if (requestedSamples > 0) intArrayOf(requestedSamples, 0) else intArrayOf(0)) for (depth in intArrayOf(24, 16)) {
            val attributes = intArrayOf(
                EGL10.EGL_RENDERABLE_TYPE, 0x40,
                EGL10.EGL_SURFACE_TYPE, EGL10.EGL_WINDOW_BIT,
                EGL10.EGL_RED_SIZE, 8, EGL10.EGL_GREEN_SIZE, 8, EGL10.EGL_BLUE_SIZE, 8,
                EGL10.EGL_ALPHA_SIZE, 8, EGL10.EGL_DEPTH_SIZE, depth,
                EGL10.EGL_SAMPLE_BUFFERS, if (samples > 0) 1 else 0, EGL10.EGL_SAMPLES, samples,
                EGL10.EGL_NONE,
            )
            val count = IntArray(1)
            if (!egl.eglChooseConfig(display, attributes, null, 0, count) || count[0] == 0) continue
            val configs = arrayOfNulls<EGLConfig>(count[0])
            if (egl.eglChooseConfig(display, attributes, configs, configs.size, count)) return configs.first { it != null }!!
        }
        error("No RGBA8 GLES3 EGL configuration with a depth buffer")
    }
}
