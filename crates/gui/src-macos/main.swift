import AppKit
import Darwin
import Foundation
import WebKit

private let processorName = "drag_and_drop_processor"

private struct HostError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

private struct TrustedWebContent {
    private let rootURL: URL

    init(rootURL: URL) throws {
        if rootURL.isFileURL {
            guard (rootURL.host ?? "").isEmpty else {
                throw HostError(message: "Bundled web content must use a local file URL")
            }
            self.rootURL = rootURL.resolvingSymlinksInPath().standardizedFileURL
            return
        }

        guard let scheme = rootURL.scheme?.lowercased(),
              ["http", "https"].contains(scheme),
              rootURL.host?.isEmpty == false,
              rootURL.user == nil,
              rootURL.password == nil
        else {
            throw HostError(message: "MFB_DEV_URL must be an HTTP(S) URL without credentials")
        }
        self.rootURL = rootURL
    }

    func allows(_ candidate: URL) -> Bool {
        if rootURL.isFileURL {
            guard candidate.isFileURL, (candidate.host ?? "").isEmpty else { return false }
            let candidate = candidate.resolvingSymlinksInPath().standardizedFileURL
            let rootPath = rootURL.path.hasSuffix("/") ? rootURL.path : rootURL.path + "/"
            return candidate.path == rootURL.path || candidate.path.hasPrefix(rootPath)
        }

        return candidate.scheme?.lowercased() == rootURL.scheme?.lowercased()
            && candidate.host?.lowercased() == rootURL.host?.lowercased()
            && Self.effectivePort(for: candidate) == Self.effectivePort(for: rootURL)
    }

    func allowsMessage(from candidate: URL?, isMainFrame: Bool) -> Bool {
        isMainFrame && candidate.map(allows) == true
    }

    private static func effectivePort(for url: URL) -> Int? {
        if let port = url.port { return port }
        switch url.scheme?.lowercased() {
        case "http": return 80
        case "https": return 443
        default: return nil
        }
    }
}

private let maxProcessLogChunkBytes = 64 * 1024

private func drainProcessLogChunks(_ buffer: inout Data, flush: Bool) -> [String] {
    var chunks: [String] = []
    while !buffer.isEmpty {
        if let newline = buffer.firstIndex(of: 0x0A),
           buffer.distance(from: buffer.startIndex, to: newline) <= maxProcessLogChunkBytes
        {
            chunks.append(String(decoding: buffer[..<newline], as: UTF8.self))
            buffer.removeSubrange(...newline)
            continue
        }
        guard buffer.count >= maxProcessLogChunkBytes else { break }
        let end = buffer.index(buffer.startIndex, offsetBy: maxProcessLogChunkBytes)
        chunks.append(String(decoding: buffer[..<end], as: UTF8.self))
        buffer.removeSubrange(..<end)
    }
    if flush, !buffer.isEmpty {
        chunks.append(String(decoding: buffer, as: UTF8.self))
        buffer.removeAll(keepingCapacity: false)
    }
    return chunks
}

private enum ProcessorLocator {
    static func candidates(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        executable: URL? = Bundle.main.executableURL,
        currentDirectory: URL = URL(fileURLWithPath: FileManager.default.currentDirectoryPath),
    ) -> [URL] {
        var candidates: [URL] = []
        if let configured = environment["MFB_PROCESSOR_BINARY"], !configured.isEmpty {
            candidates.append(URL(fileURLWithPath: configured))
        }

        if let executable {
            let executableDirectory = executable.deletingLastPathComponent()
            if executableDirectory.lastPathComponent == "MacOS" {
                let contents = executableDirectory.deletingLastPathComponent()
                let resources = contents.appendingPathComponent("Resources", isDirectory: true)
                candidates.append(resources.appendingPathComponent(processorName))
                candidates.append(resources.appendingPathComponent("bin/\(processorName)"))
                candidates.append(executableDirectory.appendingPathComponent(processorName))
                return candidates
            }

            candidates.append(executableDirectory.appendingPathComponent(processorName))
            if ["debug", "release"].contains(executableDirectory.lastPathComponent) {
                let target = executableDirectory.deletingLastPathComponent()
                candidates.append(target.appendingPathComponent("release/\(processorName)"))
                candidates.append(target.appendingPathComponent("debug/\(processorName)"))
            }

            var ancestor = executableDirectory
            while ancestor.path != "/" {
                candidates.append(ancestor.appendingPathComponent("target/release/\(processorName)"))
                candidates.append(ancestor.appendingPathComponent("target/debug/\(processorName)"))
                ancestor.deleteLastPathComponent()
            }
        }

        candidates.append(currentDirectory.appendingPathComponent(processorName))
        if let path = environment["PATH"] {
            candidates.append(contentsOf: path.split(separator: ":").map {
                URL(fileURLWithPath: String($0)).appendingPathComponent(processorName)
            })
        }
        return candidates
    }

    static func resolve() -> URL? {
        candidates().first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    static func missingError() -> String {
        let checked = candidates().map(\.path).joined(separator: "; ")
        return "Backend processor binary not found. Build it with `cargo build --release -p dev --bin drag_and_drop_processor` or set MFB_PROCESSOR_BINARY. Checked: \(checked)"
    }
}

private enum ProcessorCommand {
    static func arguments(from values: [String: Any]) throws -> [String] {
        guard let target = values["targetPath"] as? String, !target.isEmpty else {
            throw HostError(message: "process_media requires a non-empty targetPath")
        }
        guard let processingMode = values["processingMode"] as? String,
              ["both", "images_only", "videos_only"].contains(processingMode)
        else {
            throw HostError(message: "process_media received an invalid processingMode")
        }
        guard let outputMode = values["outputMode"] as? String else {
            throw HostError(message: "process_media requires outputMode")
        }

        var arguments: [String] = []
        if processingMode == "images_only" {
            arguments.append("--images-only")
        } else if processingMode == "videos_only" {
            arguments.append("--videos-only")
        }

        let outputModes: [String: String] = [
            "fast_img": "fast-img",
            "fast_vid": "fast-vid",
            "restore_jpeg": "restore-jpeg",
            "collect": "collect",
            "merge_xmp": "merge-xmp",
            "icloud_import": "icloud-import",
            "diagnostic": "diagnostic",
            "cache_clean": "cache-clean",
            "database_manager": "database-manager",
        ]
        if outputMode != "adjacent" {
            guard let mode = outputModes[outputMode] else {
                throw HostError(message: "process_media received an invalid outputMode")
            }
            arguments += ["--mode", mode]
        }

        if let strategy = values["strategy"] as? String, !strategy.isEmpty {
            guard ["avif", "jxl"].contains(strategy) else {
                throw HostError(message: "process_media received an invalid strategy")
            }
            arguments += ["--strategy", strategy]
        }
        for (key, flag) in [
            ("ultimate", "--ultimate"),
            ("verbose", "--verbose"),
            ("shortestPath", "--shortest-path"),
        ] where values[key] as? Bool == true {
            arguments.append(flag)
        }
        if values["resume"] as? Bool == true {
            arguments.append("--resume")
            if outputMode == "fast_img" {
                arguments.append("--retry")
            }
        } else if values["fresh"] as? Bool == true {
            arguments.append("--no-resume")
        }
        arguments.append(target)
        return arguments
    }

    static func terminalShellCommand(binary: URL, arguments: [String]) throws -> String {
        guard let target = arguments.last else {
            throw HostError(message: "terminal command requires a target path")
        }
        let workingDirectory = URL(fileURLWithPath: target).deletingLastPathComponent().path
        let command = ([binary.path] + arguments)
            .map { shellQuote($0) }
            .joined(separator: " ")
        return "cd \(shellQuote(workingDirectory)) && \(command)"
    }

    private static func shellQuote(_ value: String) -> String {
        "'\(value.replacingOccurrences(of: "'", with: "'\"'\"'"))'"
    }
}

private final class AppWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}

@MainActor
private final class NativeHost: NSObject, WKNavigationDelegate, WKScriptMessageHandlerWithReply {
    weak var webView: WKWebView?
    weak var window: NSWindow?
    private var activeProcess: Process?
    private let trustedContent: TrustedWebContent

    init(trustedContent: TrustedWebContent) {
        self.trustedContent = trustedContent
        super.init()
    }

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping (Any?, String?) -> Void,
    ) {
        guard let registeredWebView = webView,
              message.webView === registeredWebView,
              trustedContent.allowsMessage(
                  from: message.frameInfo.request.url,
                  isMainFrame: message.frameInfo.isMainFrame,
              )
        else {
            NSLog("MFB_SECURITY: rejected native bridge message from untrusted frame")
            replyHandler(nil, "Native bridge is unavailable to this page")
            return
        }
        guard let request = message.body as? [String: Any],
              let command = request["command"] as? String
        else {
            replyHandler(nil, "Native bridge request is malformed")
            return
        }
        let arguments = request["args"] as? [String: Any] ?? [:]

        switch command {
        case "get_processor_binary_path":
            if let binary = ProcessorLocator.resolve() {
                replyHandler(binary.path, nil)
            } else {
                replyHandler(nil, ProcessorLocator.missingError())
            }
        case "check_version_alignment":
            replyHandler(checkVersionAlignment(), nil)
        case "open_in_terminal":
            guard let binary = ProcessorLocator.resolve() else {
                replyHandler(nil, ProcessorLocator.missingError())
                return
            }
            do {
                let commandArguments = try ProcessorCommand.arguments(from: arguments)
                replyHandler(
                    try openInTerminal(binary: binary, arguments: commandArguments),
                    nil,
                )
            } catch {
                replyHandler(nil, error.localizedDescription)
            }
        case "process_media":
            startProcessing(arguments, replyHandler: replyHandler)
        case "open_folder":
            replyHandler(openFolder(arguments), nil)
        case "window_minimize":
            window?.miniaturize(nil)
            replyHandler(NSNull(), nil)
        case "window_is_maximized":
            replyHandler(window?.isZoomed ?? false, nil)
        case "window_maximize":
            if window?.isZoomed == false { window?.zoom(nil) }
            replyHandler(NSNull(), nil)
        case "window_unmaximize":
            if window?.isZoomed == true { window?.zoom(nil) }
            replyHandler(NSNull(), nil)
        case "window_close":
            window?.performClose(nil)
            replyHandler(NSNull(), nil)
        case "window_show":
            window?.makeKeyAndOrderFront(nil)
            replyHandler(NSNull(), nil)
        case "window_start_drag":
            if let event = NSApp.currentEvent { window?.performDrag(with: event) }
            replyHandler(NSNull(), nil)
        case "report_ui_error":
            let detail = arguments["message"] as? String ?? "unknown JavaScript error"
            NSLog("MFB_UI_ERROR: %@", detail)
            replyHandler(NSNull(), nil)
        default:
            replyHandler(nil, "Unknown native bridge command: \(command)")
        }
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void,
    ) {
        guard let url = navigationAction.request.url else {
            decisionHandler(.cancel)
            return
        }
        if trustedContent.allows(url) {
            decisionHandler(.allow)
        } else if navigationAction.navigationType == .linkActivated,
                  let scheme = url.scheme?.lowercased(),
                  ["http", "https", "mailto"].contains(scheme)
        {
            NSWorkspace.shared.open(url)
            decisionHandler(.cancel)
        } else {
            decisionHandler(.cancel)
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        webView.evaluateJavaScript("document.querySelectorAll('button').length") { result, error in
            if let error {
                NSLog("MFB_UI_ERROR: failed to inspect rendered UI: %@", error.localizedDescription)
            } else {
                NSLog("MFB_UI_READY: buttons=%@", String(describing: result ?? 0))
            }
        }
    }

    func emit(_ name: String, payload: Any) {
        guard JSONSerialization.isValidJSONObject(["name": name, "payload": payload]),
              let data = try? JSONSerialization.data(withJSONObject: [
                  "name": name,
                  "payload": payload,
              ]),
              let json = String(data: data, encoding: .utf8)
        else {
            return
        }
        webView?.evaluateJavaScript("window.__MFB_NATIVE_EVENT__?.(\(json))")
    }

    func terminateActiveProcess() {
        if let process = activeProcess, process.isRunning {
            process.terminate()
        }
    }

    private func openFolder(_ arguments: [String: Any]) -> Any {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.resolvesAliases = true
        panel.title = arguments["title"] as? String ?? "Select folder"
        return panel.runModal() == .OK ? (panel.url?.path ?? NSNull()) : NSNull()
    }

    private func checkVersionAlignment() -> String {
        guard let binary = ProcessorLocator.resolve() else {
            return "Version Alignment Check Skipped: drag_and_drop_processor binary not found; processing will fail until it is built or MFB_PROCESSOR_BINARY is set"
        }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["--help"]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus == 0
                ? "Version Alignment Confirmed: Rust Processor OK"
                : "Version Alignment Check Warning: Processor --help failed"
        } catch {
            return "Version Alignment Check Warning: \(error.localizedDescription)"
        }
    }

    private func startProcessing(
        _ values: [String: Any],
        replyHandler: @escaping (Any?, String?) -> Void,
    ) {
        guard activeProcess == nil else {
            replyHandler(nil, "A media processing task is already running")
            return
        }
        guard let binary = ProcessorLocator.resolve() else {
            replyHandler(nil, ProcessorLocator.missingError())
            return
        }

        let commandArguments: [String]
        do {
            commandArguments = try ProcessorCommand.arguments(from: values)
        } catch {
            replyHandler(nil, error.localizedDescription)
            return
        }

        let process = Process()
        process.executableURL = binary
        process.arguments = commandArguments
        var environment = ProcessInfo.processInfo.environment
        environment["MFB_USE_LEGACY_PY"] = "0"
        environment["FROM_APP"] = "1"
        environment["LC_ALL"] = "en_US.UTF-8"
        environment["LANG"] = "en_US.UTF-8"
        process.environment = environment

        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        emit("process-log", payload: "Starting processor backend at: \(binary.path)")

        do {
            try process.run()
        } catch {
            replyHandler(nil, "Failed to start drag_and_drop_processor at \(binary.path): \(error.localizedDescription)")
            return
        }
        activeProcess = process

        let readers = DispatchGroup()
        stream(stdout.fileHandleForReading, prefix: "", group: readers)
        stream(stderr.fileHandleForReading, prefix: "ERR: ", group: readers)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            process.waitUntilExit()
            readers.wait()
            let status = process.terminationStatus
            DispatchQueue.main.async {
                guard let self else { return }
                self.activeProcess = nil
                if status == 0 {
                    replyHandler("Completed successfully", nil)
                } else {
                    replyHandler(nil, "Process exited with status: \(status)")
                }
            }
        }
    }

    private func stream(_ handle: FileHandle, prefix: String, group: DispatchGroup) {
        group.enter()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            defer { group.leave() }
            var buffer = Data()
            while true {
                let chunk = handle.availableData
                if chunk.isEmpty { break }
                buffer.append(chunk)
                for line in drainProcessLogChunks(&buffer, flush: false) {
                    DispatchQueue.main.async { self?.emit("process-log", payload: prefix + line) }
                }
            }
            for line in drainProcessLogChunks(&buffer, flush: true) {
                DispatchQueue.main.async { self?.emit("process-log", payload: prefix + line) }
            }
        }
    }

    private func openInTerminal(binary: URL, arguments: [String]) throws -> String {
        let command = try ProcessorCommand.terminalShellCommand(
            binary: binary,
            arguments: arguments,
        )
        let shellCommand = "\(command); exec /bin/sh"
        for (name, executable, arguments) in [
            ("Ghostty", "/Applications/Ghostty.app/Contents/MacOS/ghostty", ["-e", "/bin/sh", "-c", shellCommand]),
            ("kitty", "/Applications/kitty.app/Contents/MacOS/kitty", ["/bin/sh", "-c", shellCommand]),
        ] where FileManager.default.isExecutableFile(atPath: executable) {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            if (try? process.run()) != nil { return "Opened in \(name)" }
        }

        let scripts: [(String, String)] = [
            ("iTerm", """
            on run argv
                tell application "iTerm"
                    activate
                    if (count of windows) = 0 then create window with default profile
                    tell current window
                        create tab with default profile
                        tell current session to write text (item 1 of argv)
                    end tell
                end tell
            end run
            """),
            ("Terminal", """
            on run argv
                tell application "Terminal"
                    activate
                    do script (item 1 of argv)
                end tell
            end run
            """),
        ]
        for (name, script) in scripts where name != "iTerm" || FileManager.default.fileExists(atPath: "/Applications/iTerm.app") {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
            process.arguments = ["-e", script, shellCommand]
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            do {
                try process.run()
                process.waitUntilExit()
                if process.terminationStatus == 0 { return "Opened in \(name)" }
            } catch {
                continue
            }
        }
        throw HostError(message: "Failed to open any terminal")
    }
}

@MainActor
private final class NativeWebView: WKWebView {
    weak var nativeHost: NativeHost?

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let options: [NSPasteboard.ReadingOptionKey: Any] = [.urlReadingFileURLsOnly: true]
        let paths = (sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: options) ?? [])
            .compactMap { ($0 as? NSURL)?.filePathURL?.path }
        guard !paths.isEmpty else { return false }
        nativeHost?.emit("file-drop", payload: paths)
        return true
    }
}

@MainActor
private final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: AppWindow?
    private var webView: NativeWebView?
    private var host: NativeHost?

    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
        true
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        installMainMenu()

        let contentURL: URL
        let readAccessRoot: URL?
        if let configured = ProcessInfo.processInfo.environment["MFB_DEV_URL"], !configured.isEmpty {
            guard let url = URL(string: configured) else {
                presentFatalError("MFB_DEV_URL is not a valid URL")
                return
            }
            contentURL = url
            readAccessRoot = nil
        } else if let resources = Bundle.main.resourceURL {
            let dist = resources.appendingPathComponent("dist", isDirectory: true)
            let index = dist.appendingPathComponent("index.html")
            guard FileManager.default.fileExists(atPath: index.path) else {
                presentFatalError("Bundled Vue entry point is missing: \(index.path)")
                return
            }
            contentURL = index
            readAccessRoot = dist
        } else {
            presentFatalError("App resource directory is unavailable")
            return
        }

        let trustedContent: TrustedWebContent
        do {
            trustedContent = try TrustedWebContent(rootURL: readAccessRoot ?? contentURL)
        } catch {
            presentFatalError(error.localizedDescription)
            return
        }

        let host = NativeHost(trustedContent: trustedContent)
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.userContentController.addScriptMessageHandler(
            host,
            contentWorld: .page,
            name: "mfb",
        )
        configuration.userContentController.addUserScript(
            WKUserScript(
                source: """
                window.addEventListener("error", event => {
                  void window.webkit.messageHandlers.mfb.postMessage({
                    command: "report_ui_error",
                    args: { message: `${event.message} at ${event.filename}:${event.lineno}` }
                  });
                });
                window.addEventListener("unhandledrejection", event => {
                  void window.webkit.messageHandlers.mfb.postMessage({
                    command: "report_ui_error",
                    args: { message: `Unhandled promise rejection: ${String(event.reason)}` }
                  });
                });
                """,
                injectionTime: .atDocumentStart,
                forMainFrameOnly: true,
            ),
        )

        let webView = NativeWebView(frame: .zero, configuration: configuration)
        webView.nativeHost = host
        webView.navigationDelegate = host
        webView.registerForDraggedTypes([.fileURL])

        let frame = NSRect(x: 0, y: 0, width: 1100, height: 680)
        let window = AppWindow(
            contentRect: frame,
            styleMask: [.borderless, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false,
        )
        window.title = "Modern Format Boost"
        window.contentView = webView
        window.minSize = NSSize(width: 1100, height: 680)
        window.isOpaque = false
        window.backgroundColor = .clear
        window.isMovableByWindowBackground = true
        window.center()

        host.webView = webView
        host.window = window
        self.host = host
        self.webView = webView
        self.window = window

        if let readAccessRoot {
            webView.loadFileURL(contentURL, allowingReadAccessTo: readAccessRoot)
        } else {
            webView.load(URLRequest(url: contentURL))
            if #available(macOS 13.3, *) {
                webView.isInspectable = true
            }
        }

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        host?.terminateActiveProcess()
    }

    private func presentFatalError(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Modern Format Boost could not start"
        alert.informativeText = message
        alert.alertStyle = .critical
        alert.runModal()
        NSApp.terminate(nil)
    }

    private func installMainMenu() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit Modern Format Boost", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        main.addItem(appItem)

        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
        for (title, action, key) in [
            ("Cut", #selector(NSText.cut(_:)), "x"),
            ("Copy", #selector(NSText.copy(_:)), "c"),
            ("Paste", #selector(NSText.paste(_:)), "v"),
            ("Select All", #selector(NSText.selectAll(_:)), "a"),
        ] {
            editMenu.addItem(withTitle: title, action: action, keyEquivalent: key)
        }
        editItem.submenu = editMenu
        main.addItem(editItem)
        NSApp.mainMenu = main
    }
}

private func runSelfTest() -> Int32 {
    do {
        let bundledRoot = URL(fileURLWithPath: "/tmp/mfb-app/dist", isDirectory: true)
        let bundledContent = try TrustedWebContent(rootURL: bundledRoot)
        let bundledIndex = bundledRoot.appendingPathComponent("index.html")
        let outsideBundle = URL(fileURLWithPath: "/tmp/mfb-app/private.html")
        guard bundledContent.allows(bundledIndex),
              bundledContent.allowsMessage(from: bundledIndex, isMainFrame: true),
              !bundledContent.allowsMessage(from: bundledIndex, isMainFrame: false),
              !bundledContent.allows(outsideBundle)
        else {
            fputs("native-host self-test bundled content trust failed\n", stderr)
            return 1
        }
        guard let developmentRoot = URL(string: "http://127.0.0.1:5173/app"),
              let sameOrigin = URL(string: "http://127.0.0.1:5173/assets/app.js"),
              let wrongPort = URL(string: "http://127.0.0.1:5174/app"),
              let invalidScheme = URL(string: "data:text/html,untrusted")
        else {
            fputs("native-host self-test URL fixture creation failed\n", stderr)
            return 1
        }
        let developmentContent = try TrustedWebContent(rootURL: developmentRoot)
        guard developmentContent.allows(sameOrigin),
              !developmentContent.allows(wrongPort),
              (try? TrustedWebContent(rootURL: invalidScheme)) == nil
        else {
            fputs("native-host self-test development origin trust failed\n", stderr)
            return 1
        }
        let arguments = try ProcessorCommand.arguments(from: [
            "targetPath": "/tmp/media",
            "processingMode": "images_only",
            "outputMode": "fast_img",
            "strategy": "jxl",
            "ultimate": true,
            "verbose": true,
            "resume": true,
            "shortestPath": true,
        ])
        let expected = [
            "--images-only", "--mode", "fast-img", "--strategy", "jxl", "--ultimate",
            "--verbose", "--shortest-path", "--resume", "--retry", "/tmp/media",
        ]
        guard arguments == expected else {
            fputs("native-host self-test argument mismatch: \(arguments)\n", stderr)
            return 1
        }
        let freshArguments = try ProcessorCommand.arguments(from: [
            "targetPath": "/tmp/media",
            "processingMode": "both",
            "outputMode": "fast_img",
            "fresh": true,
        ])
        guard freshArguments == ["--mode", "fast-img", "--no-resume", "/tmp/media"] else {
            fputs("native-host self-test fresh argument mismatch: \(freshArguments)\n", stderr)
            return 1
        }
        let hostileArguments = try ProcessorCommand.arguments(from: [
            "targetPath": "/tmp/media'$(id)",
            "processingMode": "both",
            "outputMode": "fast_img",
        ])
        let terminalCommand = try ProcessorCommand.terminalShellCommand(
            binary: URL(fileURLWithPath: "/tmp/processor"),
            arguments: hostileArguments,
        )
        let expectedTerminalCommand =
            "cd '/tmp' && '/tmp/processor' '--mode' 'fast-img' '/tmp/media'\"'\"'$(id)'"
        guard terminalCommand == expectedTerminalCommand else {
            fputs("native-host self-test shell quoting mismatch: \(terminalCommand)\n", stderr)
            return 1
        }
        var oversizedLog = Data(repeating: 0x61, count: maxProcessLogChunkBytes + 17)
        let boundedChunks = drainProcessLogChunks(&oversizedLog, flush: false)
        guard boundedChunks.count == 1,
              boundedChunks[0].utf8.count == maxProcessLogChunkBytes,
              oversizedLog.count == 17
        else {
            fputs("native-host self-test log chunk bound failed\n", stderr)
            return 1
        }
        let finalChunks = drainProcessLogChunks(&oversizedLog, flush: true)
        guard finalChunks.count == 1, finalChunks[0].utf8.count == 17, oversizedLog.isEmpty else {
            fputs("native-host self-test log tail flush failed\n", stderr)
            return 1
        }
        print("native-host self-test passed")
        return 0
    } catch {
        fputs("native-host self-test failed: \(error.localizedDescription)\n", stderr)
        return 1
    }
}

if CommandLine.arguments.contains("--self-test") {
    exit(runSelfTest())
}

MainActor.assumeIsolated {
    let application = NSApplication.shared
    let delegate = AppDelegate()
    application.delegate = delegate
    application.run()
}
