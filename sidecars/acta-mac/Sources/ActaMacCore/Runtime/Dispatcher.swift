import ActaMacProtocol
import Foundation

/// Despacha cada comando en su propia tarea: los trabajos largos
/// (`asr.transcribe`, `diarize.run`) no bloquean la lectura de stdin y las
/// respuestas pueden llegar fuera de orden (se correlacionan por `id`).
public actor Dispatcher {
    private let output: Output
    private let capture: CaptureService
    private let detection: DetectionService
    private let speakers: SpeakersService
    private let screen: ScreenService
    private let clipboard: ClipboardService
    private let transcription: TranscriptionService
    private let permissionsMonitor: PermissionsMonitor
    private let exit: @Sendable (Int32) -> Void
    private var shuttingDown = false

    public init(output: Output, modelsBase: URL? = nil, exit: (@Sendable (Int32) -> Void)? = nil) {
        self.output = output
        capture = CaptureService(output: output)
        detection = DetectionService(output: output)
        speakers = SpeakersService(output: output)
        screen = ScreenService(output: output)
        clipboard = ClipboardService(output: output)
        transcription = TranscriptionService(output: output, modelsBase: modelsBase)
        permissionsMonitor = PermissionsMonitor(output: output)
        self.exit = exit ?? { code in Foundation.exit(code) }
    }

    static var capabilities: [String] {
        var caps = ["mic", "detection", "ax_teams", "screen_capture", "clipboard_import", "asr", "diarization", "voiceprint", "live_asr", "permissions"]
        if #available(macOS 14.2, *) { caps.insert("process_tap", at: 0) }
        return caps
    }

    /// Procesa una línea de stdin.
    public func handle(line: String) async {
        let request: Request
        do {
            request = try ProtocolCoding.decoder().decode(Request.self, from: Data(line.utf8))
        } catch RequestDecodingError.unknownCommand(let id, let cmd) {
            await output.fail(id, .unsupported, "unknown command \(cmd)")
            return
        } catch {
            // Si al menos hay id, respondemos invalid_args para no dejar a Rust esperando.
            if let id = Self.extractId(line) {
                await output.fail(id, .invalidArgs, "bad request: \(error)")
            } else {
                Log.error("bad request line: \(error)")
            }
            return
        }
        if shuttingDown, request.command != .shutdown {
            await output.fail(request.id, .cancelled, "shutting down")
            return
        }
        do {
            try await route(request)
        } catch let e as ProtocolError {
            await output.fail(request.id, e.code, e.message)
        } catch is CancellationError {
            await output.fail(request.id, .cancelled, "cancelled")
        } catch {
            await output.fail(request.id, .internalError, "\(error)")
        }
    }

    static func extractId(_ line: String) -> UInt64? {
        struct IdOnly: Decodable { let id: UInt64 }
        return try? JSONDecoder().decode(IdOnly.self, from: Data(line.utf8)).id
    }

    private func ok<R: Encodable & Sendable>(_ id: UInt64, _ r: R) async {
        await output.respond(.success(id, r))
    }

    private func route(_ request: Request) async throws {
        let id = request.id
        switch request.command {
        case .hello(let a):
            let os = ProcessInfo.processInfo.operatingSystemVersion
            await permissionsMonitor.start()
            if a.protocolVersion != protocolVersion {
                Log.warn("host speaks protocol \(a.protocolVersion), sidecar \(protocolVersion)")
            }
            await ok(id, HelloResult(
                sidecarVersion: "0.1.0",
                protocolVersion: protocolVersion,
                macosVersion: "\(os.majorVersion).\(os.minorVersion).\(os.patchVersion)",
                capabilities: Self.capabilities
            ))
        case .shutdown:
            await shutdown()
            await ok(id, Empty())
            exit(0)

        case .permissionsStatus:
            await ok(id, PermissionsStatusResult(permissions: PermissionProbe.all()))
        case .permissionsRequest(let a):
            await ok(id, PermissionsRequestResult(status: await PermissionProbe.request(a.kind, target: a.target)))

        case .devicesList:
            await ok(id, AudioInventory.devices())
        case .appsList:
            await ok(id, AudioInventory.apps())

        case .detectStart(let a):
            await detection.start(a)
            await ok(id, Empty())
        case .detectStop:
            await detection.stop()
            await ok(id, Empty())

        case .captureStart(let a):
            await ok(id, try await capture.start(a))
        case .capturePause:
            await capture.pause()
            await ok(id, Empty())
        case .captureResume:
            await capture.resume()
            await ok(id, Empty())
        case .captureStop:
            await capture.stop()
            await ok(id, Empty())
        case .captureRetarget(let a):
            try await capture.retarget(a)
            await ok(id, Empty())

        case .speakersWatch(let a):
            await speakers.watch(a)
            await ok(id, Empty())
        case .speakersUnwatch:
            await speakers.unwatch()
            await ok(id, Empty())

        case .screenStart(let a):
            try await screen.start(a)
            await ok(id, Empty())
        case .screenSnapshot(let a):
            await ok(id, try await screen.snapshot(a))
        case .screenStop:
            await screen.stop()
            await ok(id, Empty())
        case .clipboardStart(let a):
            try await clipboard.start(a)
            await ok(id, Empty())
        case .clipboardStop:
            await clipboard.stop()
            await ok(id, Empty())

        case .asrTranscribe(let a):
            await ok(id, try await transcription.transcribe(a))
        case .asrLiveStart(let a):
            let sink = try await transcription.liveStart(a)
            await capture.setLiveSink(sink)
            await ok(id, Empty())
        case .asrLiveStop:
            await capture.setLiveSink(nil)
            await transcription.liveStop()
            await ok(id, Empty())
        case .diarizeRun(let a):
            await ok(id, try await transcription.diarize(a))
        case .voiceprintEmbed(let a):
            await ok(id, try await transcription.embed(a))
        case .modelsStatus:
            await ok(id, await transcription.modelsStatus())
        case .modelsDownload(let a):
            try await transcription.download(a.name)
            await ok(id, Empty())
        case .jobCancel(let a):
            try await transcription.cancel(a.jobId)
            await ok(id, Empty())
        case .meetingTitles(let a):
            await ok(id, await MeetingTitles.read(pid: a.pid))
        case .calendarEvents(let a):
            await ok(id, await CalendarReader.events(a))
        case .systemProfile:
            await ok(id, SystemProfile.read())
        case .appsInstalled(let a):
            await ok(id, await InstalledApps.list(a))
        }
    }

    /// Para todo y cierra ficheros antes de salir (también en EOF de stdin).
    public func shutdown() async {
        guard !shuttingDown else { return }
        shuttingDown = true
        await detection.stop()
        await speakers.unwatch()
        await screen.stop()
        await clipboard.stop()
        await capture.setLiveSink(nil)
        await transcription.liveStop()
        await transcription.cancelAll()
        await capture.stop()
        await permissionsMonitor.stop()
    }
}
