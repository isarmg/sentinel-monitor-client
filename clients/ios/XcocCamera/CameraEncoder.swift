import AVFoundation
import VideoToolbox

final class CameraEncoder: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    private let queue = DispatchQueue(label: "xcoc.camera")
    private let session = AVCaptureSession()
    private let lifecycle = CaptureGeneration()
    private var compressor: VTCompressionSession?
    private var running = false
    private var forceKeyframe = true
    var onFrame: ((Data, UInt64, Data, Data, Int, Int) -> Bool)?
    var onFailure: (() -> Void)?

    func start(front: Bool, owner: CaptureGeneration, generation: UUID) {
        let captureGeneration = lifecycle.begin()
        queue.async {
            do {
                guard !self.running, self.lifecycle.isCurrent(captureGeneration), owner.isCurrent(generation) else { return }
                try self.configureCapture(front: front)
                guard self.lifecycle.isCurrent(captureGeneration), owner.isCurrent(generation) else { return }
                self.running = true
                // Configuration has committed. Keep physical start in this
                // queue operation so a queued stop cannot be overtaken by it.
                self.session.startRunning()
            } catch {
                self.running = false
                if self.lifecycle.isCurrent(captureGeneration), owner.isCurrent(generation) { self.onFailure?() }
            }
        }
    }
    private func configureCapture(front: Bool) throws {
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        self.session.inputs.forEach { self.session.removeInput($0) }
        self.session.outputs.forEach { self.session.removeOutput($0) }
        guard self.session.canSetSessionPreset(.hd1280x720), let camera = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: front ? .front : .back) else { throw RustBridge.NativeError.failed }
        self.session.sessionPreset = .hd1280x720
        let input = try AVCaptureDeviceInput(device: camera)
        guard self.session.canAddInput(input) else { throw RustBridge.NativeError.failed }
        self.session.addInput(input)
        if camera.activeFormat.videoSupportedFrameRateRanges.contains(where: { $0.minFrameRate <= 30 && $0.maxFrameRate >= 30 }) {
            try camera.lockForConfiguration()
            camera.activeVideoMinFrameDuration = CMTime(value: 1, timescale: 30)
            camera.activeVideoMaxFrameDuration = CMTime(value: 1, timescale: 30)
            camera.unlockForConfiguration()
        }
        let output = AVCaptureVideoDataOutput()
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
        output.alwaysDiscardsLateVideoFrames = true
        output.setSampleBufferDelegate(self, queue: self.queue)
        guard self.session.canAddOutput(output) else { throw RustBridge.NativeError.failed }
        self.session.addOutput(output)
    }
    func stop(completion: (() -> Void)? = nil) {
        lifecycle.cancel()
        queue.async {
            self.running = false
            self.session.stopRunning()
            if let compressor = self.compressor { VTCompressionSessionCompleteFrames(compressor, untilPresentationTimeStamp: .invalid); VTCompressionSessionInvalidate(compressor) }
            self.compressor = nil; self.forceKeyframe = true
            completion?()
        }
    }
    private func configure(_ width: Int, _ height: Int) throws {
        let callback: VTCompressionOutputCallback = { reference, _, status, _, sample in
            guard status == noErr, let reference, let sample else { return }
            let owner = Unmanaged<CameraEncoder>.fromOpaque(reference).takeUnretainedValue()
            owner.encoded(sample)
        }
        guard VTCompressionSessionCreate(allocator: nil, width: Int32(width), height: Int32(height), codecType: kCMVideoCodecType_H264, encoderSpecification: nil, imageBufferAttributes: nil, compressedDataAllocator: nil, outputCallback: callback, refcon: Unmanaged.passUnretained(self).toOpaque(), compressionSessionOut: &compressor) == noErr, let compressor else { throw RustBridge.NativeError.failed }
        let properties: [(CFString, Any)] = [(kVTCompressionPropertyKey_RealTime, true), (kVTCompressionPropertyKey_AllowFrameReordering, false), (kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Baseline_AutoLevel), (kVTCompressionPropertyKey_AverageBitRate, 2_000_000), (kVTCompressionPropertyKey_ExpectedFrameRate, 30), (kVTCompressionPropertyKey_MaxKeyFrameInterval, 60)]
        for (key, value) in properties { guard VTSessionSetProperty(compressor, key: key, value: value as CFTypeRef) == noErr else { throw RustBridge.NativeError.failed } }
        guard VTCompressionSessionPrepareToEncodeFrames(compressor) == noErr else { throw RustBridge.NativeError.failed }
    }
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard running, let image = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        do {
            if compressor == nil { try configure(CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image)) }
            guard let compressor else { return }
            let options: CFDictionary? = forceKeyframe ? [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary : nil
            let status = VTCompressionSessionEncodeFrame(compressor, imageBuffer: image, presentationTimeStamp: CMSampleBufferGetPresentationTimeStamp(sampleBuffer), duration: CMTime(value: 1, timescale: 30), frameProperties: options, sourceFrameRefcon: nil, infoFlagsOut: nil)
            if status != noErr { throw RustBridge.NativeError.failed }
        } catch { running = false; onFailure?() }
    }
    private func encoded(_ sample: CMSampleBuffer) {
        guard let format = CMSampleBufferGetFormatDescription(sample), let block = CMSampleBufferGetDataBuffer(sample) else { return }
        var sets = [Data]()
        var headerLength: Int32 = 4
        for index in 0..<2 {
            var pointer: UnsafePointer<UInt8>?
            var size = 0
            guard CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: index, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: &headerLength) == noErr, let pointer else { return }
            sets.append(Data(bytes: pointer, count: size))
        }
        var data = Data(count: CMBlockBufferGetDataLength(block))
        let status = data.withUnsafeMutableBytes { CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: $0.count, destination: $0.baseAddress!) }
        guard status == noErr, let annex = try? H264.annexB(data, lengthBytes: Int(headerLength)) else { return }
        let timestamp = CMTimeConvertScale(CMSampleBufferGetPresentationTimeStamp(sample), timescale: 1_000_000, method: .default).value
        guard timestamp >= 0 else { return }
        let dimensions = CMVideoFormatDescriptionGetDimensions(format)
        let accepted = onFrame?(annex, UInt64(timestamp), sets[0], sets[1], Int(dimensions.width), Int(dimensions.height)) ?? false
        queue.async { self.forceKeyframe = !accepted }
    }
}
