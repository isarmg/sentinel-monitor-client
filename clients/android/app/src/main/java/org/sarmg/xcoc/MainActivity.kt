package org.sarmg.xcoc

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.widget.*
import org.json.JSONObject
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private val worker = Executors.newSingleThreadExecutor()
    private lateinit var status: TextView
    private lateinit var front: Switch
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val store = SecurePairing(this)
        val layout = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(32, 32, 32, 32) }
        fun text(hint: String) = EditText(this).apply { this.hint = hint; setSingleLine(true); layout.addView(this) }
        layout.addView(TextView(this).apply { text = "Xcoc 手机摄像头"; textSize = 24f })
        val server = text("Server HTTPS 地址")
        val name = text("摄像头名称")
        val code = text("实例授权码")
        status = TextView(this).apply { text = "读取配对状态" }
        try {
            store.load()?.let { saved -> JSONObject(saved).let { server.setText(it.getString("server")); name.setText(it.getString("name")) }; status.text = "已配对，可启动摄像头" }
                ?: run { status.text = "先在 Server 创建实例并填写授权码" }
        } catch (_: Exception) { status.text = "配对数据不可读，请重新配对" }
        layout.addView(Button(this).apply {
            text = "配对"
            setOnClickListener {
                isEnabled = false
                val input = JSONObject().put("operation", "pair").put("server", server.text.toString().trim())
                    .put("name", name.text.toString().trim()).put("authorization_code", code.text.toString().trim())
                    .put("installation_id", store.installationId()).toString()
                worker.execute {
                    val message = try { store.save(NativeBridge.call(0, input)); "已配对，可启动摄像头" } catch (_: Exception) { "配对失败，请核对地址、授权码和网络" }
                    runOnUiThread { code.setText(""); status.text = message; isEnabled = true }
                }
            }
        })
        front = Switch(this).apply { text = "使用前置摄像头" }; layout.addView(front)
        layout.addView(Button(this).apply { text = "启动摄像头"; setOnClickListener {
            if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) requestPermissions(arrayOf(Manifest.permission.CAMERA), 1)
            else startCamera()
        } })
        layout.addView(Button(this).apply { text = "停止摄像头"; setOnClickListener { stopService(Intent(this@MainActivity, CameraService::class.java)); status.text = "摄像头已停止" } })
        layout.addView(status)
        layout.addView(TextView(this).apply { text = "后台采集由常驻通知显示；可随时停止。视频在 Server 录像。" })
        setContentView(layout)
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 2)
    }
    private fun startCamera() {
        try {
            if (SecurePairing(this).load() == null) { status.text = "请先配对"; return }
            startForegroundService(Intent(this, CameraService::class.java).putExtra("front", front.isChecked))
            status.text = "摄像头已启动，连接状态见通知"
        } catch (_: Exception) { status.text = "摄像头启动失败，请核对权限和配对状态" }
    }
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == 1 && grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) startCamera()
    }
    override fun onDestroy() { worker.shutdown(); super.onDestroy() }
}
