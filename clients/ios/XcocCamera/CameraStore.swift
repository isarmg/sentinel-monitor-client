import AVFoundation
import Combine
import Foundation
import UIKit

final class CameraStore: ObservableObject {
    @Published var status = "先在 Server 创建实例并填写授权码"
    @Published var paired = false
    @Published var running = false
    @Published var stopping = false
    private let control = DispatchQueue(label: "xcoc.control")
    private let lock = NSLock()
    private let encoder = CameraEncoder()
    private var handle: UInt64 = 0
    private var sps = Data()
    private var pps = Data()
    private var width = 1280
    private var height = 720
    private var timer: DispatchSourceTimer?

    init() {
        paired = (try? KeychainPairing.read("pairing")) != nil
        if paired { status = "已配对，可启动摄像头" }
        encoder.onFrame = { [weak self] data, timestamp, sps, pps, width, height in
            guard let self else { return false }
            self.lock.lock()
            self.sps = sps; self.pps = pps; self.width = width; self.height = height
            let handle = self.handle
            self.lock.unlock()
            return handle != 0 && RustBridge.frame(handle, data, timestamp: timestamp)
        }
        encoder.onFailure = { [weak self] in DispatchQueue.main.async { self?.stop(); self?.status = "摄像头不可用，请核对权限" } }
    }
    func pair(server: String, code: String, name: String) {
        guard !running, !stopping else { status = "请先停止摄像头再配对"; return }
        status = "正在配对"
        control.async {
            do {
                let result = try RustBridge.call(0, ["operation": "pair", "server": server.trimmingCharacters(in: .whitespacesAndNewlines), "authorization_code": code.trimmingCharacters(in: .whitespacesAndNewlines), "name": name.trimmingCharacters(in: .whitespacesAndNewlines), "installation_id": try KeychainPairing.installationId()])
                try KeychainPairing.save("pairing", data: JSONSerialization.data(withJSONObject: result))
                DispatchQueue.main.async { self.paired = true; self.status = "已配对，可启动摄像头" }
            } catch { DispatchQueue.main.async { self.status = "配对失败，请核对地址、授权码和网络" } }
        }
    }
    func start(front: Bool) {
        guard !running, !stopping, paired else { return }
        AVCaptureDevice.requestAccess(for: .video) { allowed in
            DispatchQueue.main.async {
                guard allowed else { self.status = "请在系统设置中允许摄像头权限"; return }
                guard !self.running, !self.stopping, UIApplication.shared.applicationState == .active else { return }
                self.running = true; self.status = "正在启动摄像头"
                self.control.async {
                    do {
                        guard let data = try KeychainPairing.read("pairing"), let pairing = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw RustBridge.NativeError.failed }
                        let result = try RustBridge.call(0, ["operation": "open", "pairing": pairing])
                        guard let value = result["value"] as? NSNumber, value.uint64Value != 0 else { throw RustBridge.NativeError.failed }
                        self.lock.lock(); self.handle = value.uint64Value; self.sps = Data(); self.pps = Data(); self.lock.unlock()
                        self.encoder.start(front: front)
                        let timer = DispatchSource.makeTimerSource(queue: self.control)
                        timer.schedule(deadline: .now() + 1, repeating: 3)
                        timer.setEventHandler { [weak self] in self?.poll(active: true) }
                        self.timer = timer; timer.resume()
                    } catch { DispatchQueue.main.async { self.running = false; self.status = "配对不可读，请重新配对" } }
                }
            }
        }
    }
    private func poll(active: Bool) {
        lock.lock()
        let current = handle, sps = self.sps, pps = self.pps, width = self.width, height = self.height
        lock.unlock()
        guard current != 0 else { return }
        do {
            let metadata: [String: Any] = ["platform": "ios", "model": UIDevice.current.model, "width": width, "height": height, "frame_rate": 30, "active": active && !sps.isEmpty && !pps.isEmpty]
            let result = try RustBridge.call(current, ["operation": "poll", "metadata": metadata, "sps": sps.base64EncodedString(), "pps": pps.base64EncodedString()])
            if active { DispatchQueue.main.async { if self.running { self.status = result["publishing"] as? Bool == true ? "正在向 Server 发布视频" : "正在连接 Server" } } }
        } catch { if active { DispatchQueue.main.async { if self.running { self.status = "连接中断，正在重试" } } } }
    }
    func stop() {
        guard running else { return }
        // The encoder stops before the handle is destroyed; no camera frames survive stop.
        running = false; stopping = true; status = "正在停止摄像头"
        control.async { self.timer?.cancel(); self.timer = nil }
        encoder.stop { [weak self] in
            guard let self else { return }
            self.control.async {
                self.lock.lock(); let current = self.handle; self.lock.unlock()
                if current != 0 {
                    _ = try? RustBridge.call(current, ["operation": "stop"])
                    self.poll(active: false)
                    _ = try? RustBridge.call(current, ["operation": "close"])
                }
                self.lock.lock(); self.handle = 0; self.lock.unlock()
                DispatchQueue.main.async { self.stopping = false; self.status = "摄像头已停止" }
            }
        }
    }
}
