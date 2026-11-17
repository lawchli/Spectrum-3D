package io.github.lawchli.spectrum

import android.app.*
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.projection.MediaProjectionManager
import android.os.*

/** Optional demo integration. The reusable library declares no service or recording permissions. */
class CaptureService : Service() {
    private var ownedProjection: android.media.projection.MediaProjection? = null
    override fun onBind(intent: Intent?) = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // The notification's stop action arrives through startService, so it must not call startForeground.
        if (intent?.action == "stop") { DemoSession.active?.stop(); stopSelf(); return START_NOT_STICKY }
        // Every other start came from startForegroundService: promote first, then decide, or the
        // system kills the process with ForegroundServiceDidNotStartInTimeException.
        try { goForeground() } catch (e: Exception) {
            DemoSession.active?.let { it.stop(); it.error = e.message ?: e.toString() }
            stopSelf(); return START_NOT_STICKY
        }
        val model = DemoSession.active ?: run { stopSelf(); return START_NOT_STICKY }
        try {
            @Suppress("DEPRECATION") val data = intent?.getParcelableExtra<Intent>("data") ?: error("Missing projection token")
            val p = getSystemService(MediaProjectionManager::class.java).getMediaProjection(Activity.RESULT_OK, data) ?: error("Projection was not granted")
            ownedProjection = p
            p.registerCallback(object : android.media.projection.MediaProjection.Callback() {
                override fun onStop() {
                    if (model.projection === p) { model.stop(); stopSelf(startId) }
                }
            }, Handler(Looper.getMainLooper()))
            model.startPlayback(p)
            if (model.source != "system") stopSelf()
        } catch (e: Exception) {
            model.stop(); model.error = e.message ?: e.toString(); stopSelf()
        }
        return START_NOT_STICKY
    }
    private fun goForeground() {
        val text = NativeStrings(this)
        val channel = "capture"
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(channel, text.text("captureChannelName"), NotificationManager.IMPORTANCE_LOW),
        )
        val stop = PendingIntent.getService(this, 1, Intent(this, CaptureService::class.java).setAction("stop"), PendingIntent.FLAG_IMMUTABLE)
        // MainActivity is singleTask, so tapping the notification brings back the one existing instance.
        val open = PendingIntent.getActivity(this, 2, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, channel)
            .setContentTitle(text.text("captureNotificationTitle"))
            .setContentText(text.text("captureNotificationText"))
            .setSmallIcon(R.drawable.ic_capture).setOngoing(true).setContentIntent(open)
            .addAction(Notification.Action.Builder(null, text.text("stop"), stop).build()).build()
        if (Build.VERSION.SDK_INT >= 29) startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        else startForeground(1, notification)
    }
    override fun onDestroy() {
        if (ownedProjection != null && DemoSession.active?.projection === ownedProjection) DemoSession.active?.stop()
        ownedProjection = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
}

internal object DemoSession {
    private var reference = java.lang.ref.WeakReference<SpectrumModel>(null)
    var active: SpectrumModel?
        get() = reference.get()
        set(value) { reference = java.lang.ref.WeakReference(value) }
}
