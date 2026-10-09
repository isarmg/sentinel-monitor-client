import SwiftUI

@main
struct XcocCameraApp: App {
    @StateObject private var camera = CameraStore()
    @Environment(\.scenePhase) private var phase
    var body: some Scene {
        WindowGroup {
            CameraView().environmentObject(camera)
                .onChange(of: phase) { value in if value == .background { camera.stop() } }
        }
    }
}

struct CameraView: View {
    @EnvironmentObject private var camera: CameraStore
    @State private var server = ""
    @State private var name = "iPhone camera"
    @State private var code = ""
    @State private var front = false
    var body: some View {
        NavigationStack {
            Form {
                Section("配对") {
                    TextField("Server HTTPS 地址", text: $server).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                    TextField("摄像头名称", text: $name)
                    TextField("实例授权码", text: $code).textInputAutocapitalization(.never).autocorrectionDisabled()
                    Button("配对") { camera.pair(server: server, code: code, name: name); code = "" }.disabled(camera.running || camera.stopping)
                }
                Section("摄像头") {
                    Toggle("使用前置摄像头", isOn: $front).disabled(camera.running)
                    Button(camera.running ? "停止摄像头" : "启动摄像头") {
                        if camera.running { camera.stop() } else { camera.start(front: front) }
                    }.disabled(!camera.paired || camera.stopping)
                    Text(camera.status)
                    Text("请保持应用在前台。进入后台会停止采集；视频在 Server 录像。").font(.footnote)
                }
            }.navigationTitle("Xcoc Camera")
        }
    }
}
