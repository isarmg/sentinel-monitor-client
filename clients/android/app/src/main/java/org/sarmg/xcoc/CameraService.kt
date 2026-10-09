package org.sarmg.xcoc

import android.Manifest
import android.app.*
import android.content.Intent
import android.content.pm.PackageManager
import android.hardware.camera2.*
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.*
import android.util.Base64
import android.view.Surface
import org.json.JSONObject
import java.nio.ByteBuffer
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class CameraService : Service() {
    private val control = Executors.newSingleThreadScheduledExecutor()
    private val cameraThread = HandlerThread("xcoc-capture")
    private lateinit var handler: Handler
    private var camera: CameraDevice? = null
    private var capture: CameraCaptureSession? = null
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    @Volatile private var handle = 0L
    @Volatile private var closing = false
    @Volatile private var active = false
    @Volatile private var sps = ByteArray(0)
    @Volatile private var pps = ByteArray(0)
    private var width = 1280
    private var height = 720
    private var started = false
    private var lastKeyframeRequest = 0L

    override fun onBind(intent: Intent?) = null
    override fun onCreate() {
        super.onCreate()
        getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel("capture", "摄像头采集", NotificationManager.IMPORTANCE_LOW))
        startForeground(1, notification("正在启动摄像头"))
        cameraThread.start(); handler = Handler(cameraThread.looper)
    }
    private fun notification(message: String): Notification {
        val stop = PendingIntent.getService(this, 1, Intent(this, CameraService::class.java).setAction("stop"), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return Notification.Builder(this, "capture").setSmallIcon(android.R.drawable.ic_menu_camera)
            .setContentTitle("Xcoc Camera").setContentText(message).setContentIntent(open).setOngoing(true)
            .addAction(Notification.Action.Builder(null, "停止摄像头", stop).build()).build()
    }
    private fun report(message: String) { getSystemService(NotificationManager::class.java).notify(1, notification(message)) }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") { stopSelf(); return START_NOT_STICKY }
        if (started) return START_NOT_STICKY
        started = true
        val front = intent?.getBooleanExtra("front", false) ?: false
        control.execute {
            try {
                val pairing = SecurePairing(this).load() ?: error("pairing missing")
                handle = JSONObject(NativeBridge.call(0, JSONObject().put("operation", "open").put("pairing", JSONObject(pairing)).toString())).getLong("value")
                handler.post { try { startCapture(front) } catch (_: Exception) { report("采集失败，请核对摄像头权限"); stopSelf() } }
                control.scheduleWithFixedDelay({ poll() }, 1, 3, TimeUnit.SECONDS)
            } catch (_: Exception) { report("配对不可读，请重新配对"); stopSelf() }
        }
        return START_NOT_STICKY
    }

    private fun metadata(isActive: Boolean) = JSONObject().put("platform", "android").put("model", Build.MODEL.take(128))
        .put("width", width).put("height", height).put("frame_rate", 30).put("active", isActive)
    private fun poll() {
        if (closing || handle == 0L) return
        try {
            val ready = active && sps.isNotEmpty() && pps.isNotEmpty()
            val input = JSONObject().put("operation", "poll").put("metadata", metadata(ready))
                .put("sps", Base64.encodeToString(sps, Base64.NO_WRAP)).put("pps", Base64.encodeToString(pps, Base64.NO_WRAP))
            val result = JSONObject(NativeBridge.call(handle, input.toString()))
            report(if (result.optBoolean("publishing")) "正在向 Server 发布视频" else if (ready) "正在连接 Server" else "等待摄像头画面")
        } catch (_: Exception) { report("连接中断，正在重试") }
    }
    private fun bytes(buffer: ByteBuffer): ByteArray = ByteArray(buffer.remaining()).also { buffer.duplicate().get(it) }
    private fun startCapture(front: Boolean) {
        if (closing) return
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) throw SecurityException("camera permission missing")
        val manager = getSystemService(CameraManager::class.java)
        val facing = if (front) CameraCharacteristics.LENS_FACING_FRONT else CameraCharacteristics.LENS_FACING_BACK
        val id = manager.cameraIdList.firstOrNull { manager.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING) == facing }
            ?: error("selected camera unavailable")
        val characteristics = manager.getCameraCharacteristics(id)
        val sizes = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)!!.getOutputSizes(MediaCodec::class.java)
        val size = sizes.firstOrNull { it.width == 1280 && it.height == 720 }
            ?: sizes.filter { it.width <= 1280 && it.height <= 720 && it.width % 2 == 0 && it.height % 2 == 0 }.maxByOrNull { it.width * it.height }
            ?: error("capture size unavailable")
        width = size.width; height = size.height
        val format = MediaFormat.createVideoFormat("video/avc", width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, 2_000_000); setInteger(MediaFormat.KEY_FRAME_RATE, 30)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
        }
        val encoder = MediaCodec.createEncoderByType("video/avc"); codec = encoder
        encoder.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {}
            override fun onError(codec: MediaCodec, error: MediaCodec.CodecException) { active = false; report("视频编码失败"); stopSelf() }
            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
                val configuration = listOfNotNull(format.getByteBuffer("csd-0"), format.getByteBuffer("csd-1")).flatMap { H264.units(bytes(it)) }
                sps = configuration.firstOrNull { it[0].toInt() and 31 == 7 } ?: ByteArray(0)
                pps = configuration.firstOrNull { it[0].toInt() and 31 == 8 } ?: ByteArray(0)
            }
            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                try {
                    if (!closing && info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                        val buffer = codec.getOutputBuffer(index)!!
                        buffer.position(info.offset); buffer.limit(info.offset + info.size)
                        val accepted = NativeBridge.frame(handle, H264.annexB(bytes(buffer)), info.presentationTimeUs)
                        if (!accepted && SystemClock.elapsedRealtime() - lastKeyframeRequest >= 2000) {
                            lastKeyframeRequest = SystemClock.elapsedRealtime()
                            codec.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
                        }
                    }
                } catch (_: Exception) { /* Native failures retry at the next poll; private data is never logged. */ }
                finally { codec.releaseOutputBuffer(index, false) }
            }
        }, handler)
        encoder.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        val target = encoder.createInputSurface(); surface = target; encoder.start()
        manager.openCamera(id, object : CameraDevice.StateCallback() {
            override fun onOpened(device: CameraDevice) {
                if (closing) { device.close(); return }
                camera = device
                @Suppress("DEPRECATION")
                device.createCaptureSession(listOf(target), object : CameraCaptureSession.StateCallback() {
                    override fun onConfigured(session: CameraCaptureSession) {
                        if (closing) { session.close(); return }
                        capture = session
                        val request = device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
                            addTarget(target)
                            characteristics.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)?.filter { it.lower <= 30 && it.upper >= 30 }
                                ?.minByOrNull { it.upper - it.lower }?.let { set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, it) }
                            set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
                        }.build()
                        session.setRepeatingRequest(request, null, handler); active = true
                    }
                    override fun onConfigureFailed(session: CameraCaptureSession) { report("摄像头不支持所选采集格式"); stopSelf() }
                }, handler)
            }
            override fun onDisconnected(device: CameraDevice) { device.close(); active = false; stopSelf() }
            override fun onError(device: CameraDevice, error: Int) { device.close(); active = false; stopSelf() }
        }, handler)
    }
    override fun onDestroy() {
        closing = true; active = false
        handler.post {
            capture?.close(); camera?.close()
            try { codec?.stop() } catch (_: Exception) {}
            codec?.release(); surface?.release(); cameraThread.quitSafely()
        }
        control.execute {
            if (handle != 0L) {
                try {
                    NativeBridge.call(handle, JSONObject().put("operation", "stop").toString())
                    NativeBridge.call(handle, JSONObject().put("operation", "poll").put("metadata", metadata(false)).put("sps", "").put("pps", "").toString())
                } catch (_: Exception) {}
                try { NativeBridge.call(handle, "{\"operation\":\"close\"}") } catch (_: Exception) {}
                handle = 0
            }
        }
        control.shutdown(); stopForeground(STOP_FOREGROUND_REMOVE); super.onDestroy()
    }
}
