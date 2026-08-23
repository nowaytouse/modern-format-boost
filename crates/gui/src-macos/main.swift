import AppKit
import CoreServices
import Darwin
import Foundation

private let processorName = "drag_and_drop_processor"
private let maxProcessLogChunkBytes = 64 * 1024
private let maxProcessLogBatchBytes = 256 * 1024
private let maxProcessLogBatchEntries = 256
private let languagePreferenceKey = "MFBGuiLanguage"
private let appearancePreferenceKey = "MFBGuiAppearance"
private let mainWindowContentSize = NSSize(width: 980, height: 720)
private let mainWindowStyleMask: NSWindow.StyleMask = [
    .titled, .closable, .miniaturizable, .fullSizeContentView,
]

private enum AppLanguage: String, CaseIterable {
    case system
    case english
    case simplifiedChinese
    case japanese

    var resourceName: String? {
        switch self {
        case .system: nil
        case .english: "en"
        case .simplifiedChinese: "zh-Hans"
        case .japanese: "ja"
        }
    }

    var nativeTitle: String {
        switch self {
        case .system: localized("language.system")
        case .english: "English"
        case .simplifiedChinese: "简体中文"
        case .japanese: "日本語"
        }
    }
}

private enum AppAppearance: String, CaseIterable {
    case system
    case light
    case dark

    var localizedTitle: String { localized("appearance.\(rawValue)") }

    func apply() {
        switch self {
        case .system: NSApp.appearance = nil
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
    }
}

private final class LocalizationCatalog {
    static let shared = LocalizationCatalog()

    private(set) var language: AppLanguage
    private var bundle: Bundle

    private init() {
        language = UserDefaults.standard.string(forKey: languagePreferenceKey)
            .flatMap(AppLanguage.init(rawValue:)) ?? .system
        bundle = Self.bundle(for: language)
    }

    func select(_ language: AppLanguage) {
        self.language = language
        bundle = Self.bundle(for: language)
        UserDefaults.standard.set(language.rawValue, forKey: languagePreferenceKey)
    }

    func text(_ key: String) -> String {
        bundle.localizedString(forKey: key, value: key, table: nil)
    }

    private static func bundle(for language: AppLanguage) -> Bundle {
        guard let resourceName = language.resourceName,
              let path = Bundle.main.path(forResource: resourceName, ofType: "lproj"),
              let localizedBundle = Bundle(path: path)
        else { return .main }
        return localizedBundle
    }
}

private func localized(_ key: String, _ arguments: CVarArg...) -> String {
    let format = LocalizationCatalog.shared.text(key)
    guard !arguments.isEmpty else { return format }
    return String(format: format, locale: Locale.current, arguments: arguments)
}

private struct HostError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

private enum ProcessingMode: String, CaseIterable {
    case both
    case imagesOnly
    case videosOnly

    var argument: String? {
        switch self {
        case .both: nil
        case .imagesOnly: "--images-only"
        case .videosOnly: "--videos-only"
        }
    }
}

private enum OperationMode: String, CaseIterable {
    case adjacent
    case fastImgJxl
    case fastImgAvif
    case fastVid
    case restoreJpeg
    case collect
    case mergeXmp
    case iCloudImport
    case diagnostic
    case cacheClean
    case databaseManager

    var backendMode: String? {
        switch self {
        case .adjacent: nil
        case .fastImgJxl, .fastImgAvif: "fast-img"
        case .fastVid: "fast-vid"
        case .restoreJpeg: "restore-jpeg"
        case .collect: "collect"
        case .mergeXmp: "merge-xmp"
        case .iCloudImport: "icloud-import"
        case .diagnostic: "diagnostic"
        case .cacheClean: "cache-clean"
        case .databaseManager: "database-manager"
        }
    }

    var strategy: String? {
        switch self {
        case .fastImgJxl: "jxl"
        case .fastImgAvif: "avif"
        default: nil
        }
    }

    var supportsShortestPath: Bool {
        switch self {
        case .fastImgJxl, .fastImgAvif, .fastVid: true
        default: false
        }
    }
}

private struct ProcessorRequest {
    let targetPath: String
    let processingMode: ProcessingMode
    let operationMode: OperationMode
    var ultimate = true
    var verbose = true
    var shortestPath = false
    var resume = false
    var fresh = false
}

private enum ProcessorCommand {
    static func arguments(from request: ProcessorRequest) throws -> [String] {
        guard !request.targetPath.isEmpty else {
            throw HostError(message: localized("error.select_target"))
        }

        var arguments: [String] = []
        if let mode = request.processingMode.argument { arguments.append(mode) }
        if let mode = request.operationMode.backendMode { arguments += ["--mode", mode] }
        if let strategy = request.operationMode.strategy {
            arguments += ["--strategy", strategy]
        }
        if request.ultimate { arguments.append("--ultimate") }
        if request.verbose { arguments.append("--verbose") }
        if request.shortestPath, request.operationMode.supportsShortestPath {
            arguments.append("--shortest-path")
        }
        if request.resume {
            arguments.append("--resume")
            if request.operationMode.backendMode == "fast-img" { arguments.append("--retry") }
        } else if request.fresh {
            arguments.append("--no-resume")
        }
        arguments.append(request.targetPath)
        return arguments
    }

    static func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
    }

    static func terminalShellCommand(binary: URL, arguments: [String]) throws -> String {
        guard binary.isFileURL else { throw HostError(message: localized("error.backend_local")) }
        let target = arguments.last.map(URL.init(fileURLWithPath:))
        let workingDirectory: URL
        if let target,
           (try? target.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
        {
            workingDirectory = target.deletingLastPathComponent()
        } else {
            workingDirectory = target?.deletingLastPathComponent()
                ?? binary.deletingLastPathComponent()
        }
        return "cd \(shellQuote(workingDirectory.path)) && "
            + ([binary.path] + arguments).map(shellQuote).joined(separator: " ")
    }
}

private func processingRequiresPhotosAutomation(_ request: ProcessorRequest) -> Bool {
    request.operationMode == .iCloudImport
        || (request.shortestPath && request.operationMode.supportsShortestPath)
}

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

private final class ProcessLogBackpressure: @unchecked Sendable {
    private let lock = NSLock()
    private let maxBytes: Int
    private let maxEntries: Int
    private var pending = ""
    private var pendingBytes = 0
    private var pendingEntries = 0
    private var omittedEntries: UInt64 = 0
    private var deliveryInFlight = false

    init(maxBytes: Int = maxProcessLogBatchBytes, maxEntries: Int = maxProcessLogBatchEntries) {
        self.maxBytes = maxBytes
        self.maxEntries = maxEntries
    }

    func enqueue(_ entry: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let separatorBytes = pendingEntries == 0 ? 0 : 1
        let entryBytes = entry.utf8.count
        if pendingEntries >= maxEntries
            || pendingBytes + separatorBytes + entryBytes > maxBytes
        {
            if omittedEntries < UInt64.max { omittedEntries += 1 }
        } else {
            if pendingEntries > 0 { pending.append("\n") }
            pending.append(entry)
            pendingBytes += separatorBytes + entryBytes
            pendingEntries += 1
        }
        guard !deliveryInFlight else { return false }
        deliveryInFlight = true
        return true
    }

    func takeDelivery() -> String? {
        lock.lock()
        defer { lock.unlock() }
        guard deliveryInFlight else { return nil }
        var payload = pending
        if omittedEntries > 0 {
            if pendingEntries > 0 { payload.append("\n") }
            payload.append(localized("log.omitted", omittedEntries))
        }
        pending.removeAll(keepingCapacity: true)
        pendingBytes = 0
        pendingEntries = 0
        omittedEntries = 0
        return payload
    }

    func finishDelivery() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let hasPending = pendingEntries > 0 || omittedEntries > 0
        if !hasPending { deliveryInFlight = false }
        return hasPending
    }

    var isIdle: Bool {
        lock.lock()
        defer { lock.unlock() }
        return !deliveryInFlight && pendingEntries == 0 && omittedEntries == 0
    }
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
        return localized("error.backend_missing", checked)
    }
}

private enum PhotosAutomationPreflightError: LocalizedError {
    case photosUnavailable
    case permissionDenied(OSStatus)
    case checkFailed(OSStatus)

    var shouldOpenSettings: Bool {
        if case .permissionDenied = self { true } else { false }
    }

    var errorDescription: String? {
        switch self {
        case .photosUnavailable:
            localized("error.photos_unavailable")
        case let .permissionDenied(status):
            localized("error.photos_denied", status)
        case let .checkFailed(status):
            localized("error.photos_check", status)
        }
    }
}

@MainActor
private final class NativeHost {
    var onLog: ((String) -> Void)?
    var onCompletion: ((Result<String, Error>) -> Void)?
    private var activeProcess: Process?
    private let processLogs = ProcessLogBackpressure()
    private var pendingProcessCompletion: (() -> Void)?

    var isRunning: Bool { activeProcess != nil }

    func checkVersionAlignment() -> String {
        guard let binary = ProcessorLocator.resolve() else {
            return localized("status.processor_unavailable")
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
                ? localized("status.processor_ready")
                : localized("status.processor_failed")
        } catch {
            return "\(localized("status.processor_failed")): \(error.localizedDescription)"
        }
    }

    func startProcessing(_ request: ProcessorRequest, photosAutomationAuthorized: Bool = false) {
        guard activeProcess == nil else {
            onCompletion?(.failure(HostError(message: localized("error.task_running"))))
            return
        }
        guard let binary = ProcessorLocator.resolve() else {
            onCompletion?(.failure(HostError(message: ProcessorLocator.missingError())))
            return
        }
        if processingRequiresPhotosAutomation(request), !photosAutomationAuthorized {
            requestPhotosAutomationPermission { [weak self] result in
                guard let self else { return }
                switch result {
                case .success:
                    self.startProcessing(request, photosAutomationAuthorized: true)
                case let .failure(error):
                    if error.shouldOpenSettings,
                       let settings = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
                    {
                        NSWorkspace.shared.open(settings)
                    }
                    self.onLog?(localized("log.photos_preflight", error.localizedDescription))
                    self.onCompletion?(.failure(error))
                }
            }
            return
        }

        let arguments: [String]
        do { arguments = try ProcessorCommand.arguments(from: request) }
        catch { onCompletion?(.failure(error)); return }

        let process = Process()
        process.executableURL = binary
        process.arguments = arguments
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
        onLog?(localized("log.backend_start", binary.path))
        do { try process.run() }
        catch {
            onCompletion?(.failure(HostError(message: localized("error.backend_start", error.localizedDescription))))
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
            DispatchQueue.main.async { self?.completeProcessingAfterLogs(status: status) }
        }
    }

    func terminalCommand(for request: ProcessorRequest) throws -> String {
        guard let binary = ProcessorLocator.resolve() else {
            throw HostError(message: ProcessorLocator.missingError())
        }
        return try ProcessorCommand.terminalShellCommand(
            binary: binary,
            arguments: ProcessorCommand.arguments(from: request),
        )
    }

    func openInTerminal(_ request: ProcessorRequest) throws -> String {
        let command = try terminalCommand(for: request)
        let shellCommand = "\(command); exec /bin/sh"
        for (name, executable, arguments) in [
            ("Ghostty", "/Applications/Ghostty.app/Contents/MacOS/ghostty", ["-e", "/bin/sh", "-c", shellCommand]),
            ("kitty", "/Applications/kitty.app/Contents/MacOS/kitty", ["/bin/sh", "-c", shellCommand]),
        ] where FileManager.default.isExecutableFile(atPath: executable) {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            if (try? process.run()) != nil { return localized("status.opened_terminal", name) }
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
        for (name, script) in scripts
        where name != "iTerm" || FileManager.default.fileExists(atPath: "/Applications/iTerm.app") {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
            process.arguments = ["-e", script, shellCommand]
            do {
                try process.run()
                process.waitUntilExit()
                if process.terminationStatus == 0 { return localized("status.opened_terminal", name) }
            } catch { continue }
        }
        throw HostError(message: localized("error.no_terminal"))
    }

    func terminateActiveProcess() {
        if let process = activeProcess, process.isRunning { process.terminate() }
    }

    private func requestPhotosAutomationPermission(
        completion: @escaping (Result<Void, PhotosAutomationPreflightError>) -> Void,
    ) {
        let photosBundleIdentifier = "com.apple.Photos"
        let checkPermission = {
            let target = NSAppleEventDescriptor(bundleIdentifier: photosBundleIdentifier)
            guard let descriptor = target.aeDesc else {
                DispatchQueue.main.async { completion(.failure(.photosUnavailable)) }
                return
            }
            let status = AEDeterminePermissionToAutomateTarget(descriptor, typeWildCard, typeWildCard, true)
            DispatchQueue.main.async {
                if status == noErr {
                    completion(.success(()))
                } else if status == OSStatus(errAEEventNotPermitted)
                    || status == OSStatus(errAEEventWouldRequireUserConsent)
                {
                    completion(.failure(.permissionDenied(status)))
                } else {
                    completion(.failure(.checkFailed(status)))
                }
            }
        }

        if NSRunningApplication.runningApplications(withBundleIdentifier: photosBundleIdentifier).isEmpty {
            guard let photosURL = NSWorkspace.shared.urlForApplication(withBundleIdentifier: photosBundleIdentifier) else {
                completion(.failure(.photosUnavailable))
                return
            }
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = false
            let completed = NSLock()
            var didComplete = false
            let failWithTimeout = {
                completed.lock()
                let alreadyDone = didComplete
                if !alreadyDone { didComplete = true }
                completed.unlock()
                guard !alreadyDone else { return }
                DispatchQueue.main.async { completion(.failure(.photosUnavailable)) }
            }
            DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 10) {
                failWithTimeout()
            }
            NSWorkspace.shared.openApplication(at: photosURL, configuration: configuration) { _, error in
                completed.lock()
                let alreadyDone = didComplete
                if !alreadyDone { didComplete = true }
                completed.unlock()
                guard !alreadyDone else { return }
                if error != nil {
                    DispatchQueue.main.async { completion(.failure(.photosUnavailable)) }
                } else {
                    DispatchQueue.global(qos: .userInitiated).async { checkPermission() }
                }
            }
        } else {
            DispatchQueue.global(qos: .userInitiated).async { checkPermission() }
        }
    }

    private func completeProcessingAfterLogs(status: Int32) {
        let finish = { [weak self] in
            guard let self else { return }
            self.activeProcess = nil
            if status == 0 {
                self.onCompletion?(.success(localized("status.completed")))
            } else {
                self.onCompletion?(.failure(HostError(message: localized("status.process_exit", status))))
            }
        }
        if processLogs.isIdle { finish() } else { pendingProcessCompletion = finish }
    }

    private func flushProcessLogs() {
        guard let payload = processLogs.takeDelivery() else { return }
        onLog?(payload)
        if processLogs.finishDelivery() {
            flushProcessLogs()
        } else if let completion = pendingProcessCompletion {
            pendingProcessCompletion = nil
            completion()
        }
    }

    private func stream(_ handle: FileHandle, prefix: String, group: DispatchGroup) {
        let processLogs = processLogs
        group.enter()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            defer { group.leave() }
            var buffer = Data()
            while true {
                let chunk = handle.availableData
                if chunk.isEmpty { break }
                buffer.append(chunk)
                for line in drainProcessLogChunks(&buffer, flush: false) {
                    if processLogs.enqueue(prefix + line) {
                        DispatchQueue.main.async { self?.flushProcessLogs() }
                    }
                }
            }
            for line in drainProcessLogChunks(&buffer, flush: true) {
                if processLogs.enqueue(prefix + line) {
                    DispatchQueue.main.async { self?.flushProcessLogs() }
                }
            }
        }
    }
}

@MainActor
private final class NativeDropView: NSVisualEffectView {
    var onDrop: ((String) -> Void)?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        registerForDraggedTypes([.fileURL])
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        registerForDraggedTypes([.fileURL])
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        sender.draggingPasteboard.canReadObject(forClasses: [NSURL.self]) ? .copy : []
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard let urls = sender.draggingPasteboard.readObjects(
            forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true],
        ) as? [URL], let first = urls.first else { return false }
        onDrop?(first.path)
        return true
    }
}

@MainActor
private final class AppController: NSObject, NSWindowDelegate {
    private let host = NativeHost()
    private let window: NSWindow
    private let titleLabel = NSTextField(labelWithString: "Modern Format Boost")
    private let subtitleLabel = NSTextField(labelWithString: "")
    private let mediaLabel = NSTextField(labelWithString: "")
    private let operationLabel = NSTextField(labelWithString: "")
    private let languageLabel = NSTextField(labelWithString: "")
    private let appearanceLabel = NSTextField(labelWithString: "")
    private let targetField = NSTextField()
    private let processingPopup = NSPopUpButton()
    private let operationPopup = NSPopUpButton()
    private let languagePopup = NSPopUpButton()
    private let appearancePopup = NSPopUpButton()
    private let ultimateCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let verboseCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let shortestPathCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let resumeCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let commandField = NSTextField()
    private let logView = NSTextView()
    private let statusLabel = NSTextField(labelWithString: "")
    private let progressIndicator = NSProgressIndicator()
    private let chooseButton = NSButton(title: "", target: nil, action: nil)
    private let openButton = NSButton(title: "", target: nil, action: nil)
    private let copyButton = NSButton(title: "", target: nil, action: nil)
    private let runButton = NSButton(title: "", target: nil, action: nil)
    private var lastRequest: ProcessorRequest?
    private var sawResumeDecision = false
    private var configurationControlsEnabled = true
    private var processorStatus = ""

    override init() {
        window = NSWindow(
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: mainWindowContentSize.width,
                height: mainWindowContentSize.height,
            ),
            styleMask: mainWindowStyleMask,
            backing: .buffered,
            defer: false,
        )
        super.init()
        configureWindow()
        configureHost()
    }

    func show() {
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        processorStatus = host.checkVersionAlignment()
        statusLabel.stringValue = processorStatus
    }

    func windowWillClose(_ notification: Notification) {
        host.terminateActiveProcess()
        NSApp.terminate(nil)
    }

    private func configureWindow() {
        window.title = "Modern Format Boost"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .visible
        window.setContentSize(mainWindowContentSize)
        window.contentMinSize = mainWindowContentSize
        window.contentMaxSize = mainWindowContentSize
        window.standardWindowButton(.zoomButton)?.isEnabled = false
        window.tabbingMode = .disallowed
        window.delegate = self

        let root = NativeDropView()
        root.material = .underWindowBackground
        root.blendingMode = .behindWindow
        root.state = .active
        root.onDrop = { [weak self] path in self?.acceptTarget(path) }
        window.contentView = root

        let icon = NSImageView()
        icon.image = NSImage(
            systemSymbolName: "photo.stack.fill",
            accessibilityDescription: "Modern Format Boost",
        )
        icon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 30, weight: .medium)
        icon.contentTintColor = .controlAccentColor
        icon.setContentHuggingPriority(.required, for: .horizontal)
        titleLabel.font = .systemFont(ofSize: 26, weight: .bold)
        subtitleLabel.textColor = .secondaryLabelColor
        let titleStack = NSStackView(views: [titleLabel, subtitleLabel])
        titleStack.orientation = .vertical
        titleStack.alignment = .leading
        titleStack.spacing = 2
        let identity = NSStackView(views: [icon, titleStack])
        identity.orientation = .horizontal
        identity.alignment = .centerY
        identity.spacing = 12

        languagePopup.target = self
        languagePopup.action = #selector(languageChanged)
        appearancePopup.target = self
        appearancePopup.action = #selector(appearanceChanged)
        let preferenceGrid = NSGridView(views: [
            [languageLabel, languagePopup],
            [appearanceLabel, appearancePopup],
        ])
        preferenceGrid.rowSpacing = 5
        preferenceGrid.columnSpacing = 8
        preferenceGrid.column(at: 0).xPlacement = .trailing
        preferenceGrid.column(at: 1).xPlacement = .fill
        let headerSpacer = NSView()
        headerSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let header = NSStackView(views: [identity, headerSpacer, preferenceGrid])
        header.orientation = .horizontal
        header.alignment = .centerY
        header.spacing = 16

        targetField.isEditable = false
        targetField.isSelectable = true
        targetField.lineBreakMode = .byTruncatingMiddle
        chooseButton.target = self
        chooseButton.action = #selector(chooseTarget)
        let targetRow = NSStackView(views: [targetField, chooseButton])
        targetRow.orientation = .horizontal
        targetRow.spacing = 8
        targetField.setContentHuggingPriority(.defaultLow, for: .horizontal)

        processingPopup.target = self
        processingPopup.action = #selector(configurationChanged)
        operationPopup.target = self
        operationPopup.action = #selector(configurationChanged)
        let grid = NSGridView(views: [
            [mediaLabel, processingPopup],
            [operationLabel, operationPopup],
        ])
        grid.rowSpacing = 8
        grid.columnSpacing = 12
        grid.column(at: 0).xPlacement = .trailing
        grid.column(at: 1).xPlacement = .fill

        ultimateCheck.state = .on
        verboseCheck.state = .on
        for control in [ultimateCheck, verboseCheck, shortestPathCheck, resumeCheck] {
            control.target = self
            control.action = #selector(configurationChanged)
        }
        let options = NSStackView(views: [ultimateCheck, verboseCheck, shortestPathCheck, resumeCheck])
        options.orientation = .horizontal
        options.spacing = 18

        commandField.isEditable = false
        commandField.isSelectable = true
        commandField.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        commandField.textColor = .secondaryLabelColor
        commandField.lineBreakMode = .byTruncatingMiddle

        openButton.target = self
        openButton.action = #selector(openInTerminal)
        copyButton.target = self
        copyButton.action = #selector(copyCommand)
        runButton.target = self
        runButton.action = #selector(runHere)
        runButton.keyEquivalent = "\r"
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let actionRow = NSStackView(views: [openButton, copyButton, spacer, runButton])
        actionRow.orientation = .horizontal
        actionRow.spacing = 8

        logView.isEditable = false
        logView.isSelectable = true
        logView.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        logView.textContainerInset = NSSize(width: 8, height: 8)
        logView.backgroundColor = .textBackgroundColor.withAlphaComponent(0.72)
        let logScroll = NSScrollView()
        logScroll.documentView = logView
        logScroll.hasVerticalScroller = true
        logScroll.borderType = .bezelBorder
        logScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 220).isActive = true

        statusLabel.textColor = .secondaryLabelColor
        statusLabel.lineBreakMode = .byTruncatingTail
        progressIndicator.style = .spinning
        progressIndicator.controlSize = .small
        progressIndicator.isDisplayedWhenStopped = false
        let statusSpacer = NSView()
        statusSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let statusRow = NSStackView(views: [progressIndicator, statusLabel, statusSpacer])
        statusRow.orientation = .horizontal
        statusRow.alignment = .centerY
        statusRow.spacing = 8
        let stack = NSStackView(views: [
            header, targetRow, grid, options, commandField, actionRow, logScroll, statusRow,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        for view in [header, targetRow, grid, options, commandField, actionRow, logScroll, statusRow] {
            view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -28),
            stack.topAnchor.constraint(equalTo: root.safeAreaLayoutGuide.topAnchor, constant: 24),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -24),
        ])
        applyLocalization()
        selectSavedPreferences()
        configurationChanged()
    }

    private func configureHost() {
        host.onLog = { [weak self] text in self?.appendLog(text) }
        host.onCompletion = { [weak self] result in self?.processingCompleted(result) }
    }

    @objc private func chooseTarget() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.resolvesAliases = true
        panel.title = localized("panel.select.title")
        if panel.runModal() == .OK, let path = panel.url?.path { acceptTarget(path) }
    }

    private func acceptTarget(_ path: String) {
        targetField.stringValue = path
        configurationChanged()
    }

    @objc private func configurationChanged() {
        shortestPathCheck.isEnabled = configurationControlsEnabled && selectedOperation.supportsShortestPath
        if !shortestPathCheck.isEnabled { shortestPathCheck.state = .off }
        guard !targetField.stringValue.isEmpty else {
            commandField.stringValue = ""
            statusLabel.stringValue = processorStatus.isEmpty ? localized("status.ready") : processorStatus
            return
        }
        do {
            commandField.stringValue = try host.terminalCommand(for: request())
            statusLabel.stringValue = localized("status.ready")
        } catch {
            commandField.stringValue = ""
            statusLabel.stringValue = error.localizedDescription
        }
    }

    @objc private func openInTerminal() {
        do { statusLabel.stringValue = try host.openInTerminal(request()) }
        catch { present(error) }
    }

    @objc private func copyCommand() {
        do {
            let command = try host.terminalCommand(for: request())
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(command, forType: .string)
            statusLabel.stringValue = localized("status.command_copied")
        } catch { present(error) }
    }

    @objc private func runHere() {
        guard !host.isRunning else { return }
        do {
            let request = try request()
            lastRequest = request
            sawResumeDecision = false
            setProcessing(true)
            appendLog("▶︎ \(try host.terminalCommand(for: request))")
            host.startProcessing(request)
        } catch { present(error) }
    }

    private var selectedOperation: OperationMode {
        OperationMode.allCases[safe: operationPopup.indexOfSelectedItem] ?? .adjacent
    }

    private func request() throws -> ProcessorRequest {
        guard !targetField.stringValue.isEmpty else {
            throw HostError(message: localized("error.select_target"))
        }
        let processing = ProcessingMode.allCases[safe: processingPopup.indexOfSelectedItem] ?? .both
        return ProcessorRequest(
            targetPath: targetField.stringValue,
            processingMode: processing,
            operationMode: selectedOperation,
            ultimate: ultimateCheck.state == .on,
            verbose: verboseCheck.state == .on,
            shortestPath: shortestPathCheck.state == .on,
            resume: resumeCheck.state == .on,
        )
    }

    private func appendLog(_ text: String) {
        if text.contains("MFB_RESUME_DECISION_REQUIRED") { sawResumeDecision = true }
        let next = logView.string.isEmpty ? text : "\(logView.string)\n\(text)"
        let lines = next.split(separator: "\n", omittingEmptySubsequences: false)
        logView.string = lines.count > 3_000 ? lines.suffix(3_000).joined(separator: "\n") : next
        logView.scrollToEndOfDocument(nil)
    }

    private func processingCompleted(_ result: Result<String, Error>) {
        switch result {
        case let .success(message):
            setProcessing(false)
            statusLabel.stringValue = message
            appendLog("✓ \(message)")
        case let .failure(error):
            appendLog("✗ \(error.localizedDescription)")
            if sawResumeDecision, var retry = lastRequest, !retry.resume, !retry.fresh {
                let alert = NSAlert()
                alert.messageText = localized("alert.resume.title")
                alert.informativeText = localized("alert.resume.info")
                alert.addButton(withTitle: localized("alert.resume.resume"))
                alert.addButton(withTitle: localized("alert.resume.fresh"))
                alert.addButton(withTitle: localized("alert.cancel"))
                switch alert.runModal() {
                case .alertFirstButtonReturn:
                    retry.resume = true
                    resumeCheck.state = .on
                case .alertSecondButtonReturn:
                    retry.fresh = true
                default:
                    setProcessing(false)
                    statusLabel.stringValue = error.localizedDescription
                    return
                }
                lastRequest = retry
                sawResumeDecision = false
                setProcessing(true)
                host.startProcessing(retry)
            } else {
                setProcessing(false)
                statusLabel.stringValue = error.localizedDescription
            }
        }
    }

    @objc private func languageChanged() {
        let language = AppLanguage.allCases[safe: languagePopup.indexOfSelectedItem] ?? .system
        LocalizationCatalog.shared.select(language)
        applyLocalization()
        (NSApp.delegate as? AppDelegate)?.configureMenus()
        configurationChanged()
    }

    @objc private func appearanceChanged() {
        let appearance = AppAppearance.allCases[safe: appearancePopup.indexOfSelectedItem] ?? .system
        UserDefaults.standard.set(appearance.rawValue, forKey: appearancePreferenceKey)
        appearance.apply()
    }

    private func selectSavedPreferences() {
        let selectedLanguage = LocalizationCatalog.shared.language
        languagePopup.selectItem(at: AppLanguage.allCases.firstIndex(of: selectedLanguage) ?? 0)
        let appearance = UserDefaults.standard.string(forKey: appearancePreferenceKey)
            .flatMap(AppAppearance.init(rawValue:)) ?? .system
        appearancePopup.selectItem(at: AppAppearance.allCases.firstIndex(of: appearance) ?? 0)
        appearance.apply()
    }

    private func replaceTitles(_ popup: NSPopUpButton, with titles: [String]) {
        let selected = max(0, popup.indexOfSelectedItem)
        popup.removeAllItems()
        popup.addItems(withTitles: titles)
        popup.selectItem(at: min(selected, max(0, titles.count - 1)))
    }

    private func applyLocalization() {
        subtitleLabel.stringValue = localized("app.subtitle")
        targetField.placeholderString = localized("field.target.placeholder")
        mediaLabel.stringValue = localized("field.media")
        operationLabel.stringValue = localized("field.operation")
        languageLabel.stringValue = localized("field.language")
        appearanceLabel.stringValue = localized("field.appearance")
        chooseButton.title = localized("button.choose")
        openButton.title = localized("button.open_terminal")
        copyButton.title = localized("button.copy_command")
        runButton.title = localized("button.run")
        ultimateCheck.title = localized("option.ultimate")
        verboseCheck.title = localized("option.verbose")
        shortestPathCheck.title = localized("option.shortest_path")
        resumeCheck.title = localized("option.resume")
        commandField.placeholderString = localized("command.placeholder")
        replaceTitles(processingPopup, with: [
            localized("media.both"), localized("media.images"), localized("media.videos"),
        ])
        replaceTitles(operationPopup, with: [
            localized("operation.adjacent"), localized("operation.fast_jxl"),
            localized("operation.fast_avif"), localized("operation.fast_video"),
            localized("operation.restore_jpeg"), localized("operation.collect"),
            localized("operation.merge_xmp"), localized("operation.icloud_import"),
            localized("operation.diagnostic"), localized("operation.cache_clean"),
            localized("operation.database"),
        ])
        replaceTitles(languagePopup, with: AppLanguage.allCases.map(\.nativeTitle))
        replaceTitles(appearancePopup, with: AppAppearance.allCases.map(\.localizedTitle))
        if host.isRunning { statusLabel.stringValue = localized("status.running") }
    }

    private func setProcessing(_ processing: Bool) {
        configurationControlsEnabled = !processing
        for control in [
            chooseButton, processingPopup, operationPopup, ultimateCheck, verboseCheck,
            resumeCheck, openButton, copyButton, runButton,
        ] {
            control.isEnabled = !processing
        }
        shortestPathCheck.isEnabled = !processing && selectedOperation.supportsShortestPath
        if processing {
            progressIndicator.startAnimation(nil)
            statusLabel.stringValue = localized("status.running")
        } else {
            progressIndicator.stopAnimation(nil)
        }
    }

    private func present(_ error: Error) {
        statusLabel.stringValue = error.localizedDescription
        NSAlert(error: error).runModal()
    }
}

private extension Collection {
    subscript(safe index: Index) -> Element? { indices.contains(index) ? self[index] : nil }
}

@MainActor
private final class AppDelegate: NSObject, NSApplicationDelegate {
    private var controller: AppController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let appearance = UserDefaults.standard.string(forKey: appearancePreferenceKey)
            .flatMap(AppAppearance.init(rawValue:)) ?? .system
        appearance.apply()
        configureMenus()
        let controller = AppController()
        self.controller = controller
        controller.show()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func configureMenus() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: localized("menu.about"), action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: localized("menu.quit"), action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        main.addItem(appItem)

        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: localized("menu.edit"))
        editMenu.addItem(withTitle: localized("menu.undo"), action: Selector(("undo:")), keyEquivalent: "z")
        let redo = NSMenuItem(title: localized("menu.redo"), action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(redo)
        editMenu.addItem(.separator())
        for (title, action, key) in [
            (localized("menu.cut"), #selector(NSText.cut(_:)), "x"),
            (localized("menu.copy"), #selector(NSText.copy(_:)), "c"),
            (localized("menu.paste"), #selector(NSText.paste(_:)), "v"),
            (localized("menu.select_all"), #selector(NSText.selectAll(_:)), "a"),
        ] {
            editMenu.addItem(withTitle: title, action: action, keyEquivalent: key)
        }
        editItem.submenu = editMenu
        main.addItem(editItem)

        let windowItem = NSMenuItem()
        let windowMenu = NSMenu(title: localized("menu.window"))
        windowMenu.addItem(withTitle: localized("menu.minimize"), action: #selector(NSWindow.miniaturize(_:)), keyEquivalent: "m")
        windowMenu.addItem(.separator())
        windowMenu.addItem(withTitle: localized("menu.front"), action: #selector(NSApplication.arrangeInFront(_:)), keyEquivalent: "")
        windowItem.submenu = windowMenu
        main.addItem(windowItem)
        NSApp.windowsMenu = windowMenu
        NSApp.mainMenu = main
    }
}

private func runSelfTest() -> Int32 {
    do {
        guard !mainWindowStyleMask.contains(.resizable),
              mainWindowContentSize == NSSize(width: 980, height: 720)
        else {
            fputs("native-host self-test fixed window sizing failed\n", stderr)
            return 1
        }
        let request = ProcessorRequest(
            targetPath: "/tmp/media", processingMode: .imagesOnly, operationMode: .fastImgJxl,
            ultimate: true, verbose: true, shortestPath: true, resume: true,
        )
        let expected = [
            "--images-only", "--mode", "fast-img", "--strategy", "jxl", "--ultimate",
            "--verbose", "--shortest-path", "--resume", "--retry", "/tmp/media",
        ]
        guard try ProcessorCommand.arguments(from: request) == expected else {
            fputs("native-host self-test argument mapping failed\n", stderr)
            return 1
        }
        guard processingRequiresPhotosAutomation(request),
              processingRequiresPhotosAutomation(ProcessorRequest(targetPath: "/tmp/media", processingMode: .both, operationMode: .iCloudImport)),
              processingRequiresPhotosAutomation(ProcessorRequest(targetPath: "/tmp/media", processingMode: .videosOnly, operationMode: .fastVid, shortestPath: true)),
              !processingRequiresPhotosAutomation(ProcessorRequest(targetPath: "/tmp/media", processingMode: .both, operationMode: .fastImgJxl))
        else {
            fputs("native-host self-test Photos Automation routing failed\n", stderr)
            return 1
        }
        for localization in ["en", "zh-Hans", "ja"] {
            guard let path = Bundle.main.path(forResource: localization, ofType: "lproj"),
                  let bundle = Bundle(path: path),
                  bundle.localizedString(forKey: "button.run", value: "button.run", table: nil)
                      != "button.run"
            else {
                fputs("native-host self-test missing localization: \(localization)\n", stderr)
                return 1
            }
        }
        let hostile = ProcessorRequest(
            targetPath: "/tmp/media'$(id)", processingMode: .both, operationMode: .fastImgJxl,
            ultimate: false, verbose: false,
        )
        let shell = try ProcessorCommand.terminalShellCommand(
            binary: URL(fileURLWithPath: "/tmp/processor"),
            arguments: ProcessorCommand.arguments(from: hostile),
        )
        guard shell == "cd '/tmp' && '/tmp/processor' '--mode' 'fast-img' '--strategy' 'jxl' '/tmp/media'\"'\"'$(id)'" else {
            fputs("native-host self-test shell quoting failed: \(shell)\n", stderr)
            return 1
        }
        var oversized = Data(repeating: 0x61, count: maxProcessLogChunkBytes + 17)
        let chunks = drainProcessLogChunks(&oversized, flush: false)
        guard chunks.count == 1, chunks[0].utf8.count == maxProcessLogChunkBytes, oversized.count == 17 else {
            fputs("native-host self-test log chunk bound failed\n", stderr)
            return 1
        }
        let backpressure = ProcessLogBackpressure(maxBytes: 32, maxEntries: 2)
        let expectedBackpressure = "first\nsecond\n\(localized("log.omitted", UInt64(1)))"
        guard backpressure.enqueue("first"), !backpressure.enqueue("second"),
              !backpressure.enqueue("omitted"),
              backpressure.takeDelivery() == expectedBackpressure,
              !backpressure.finishDelivery(), backpressure.isIdle
        else {
            fputs("native-host self-test log backpressure failed\n", stderr)
            return 1
        }
        let lock = NSLock()
        var completed = false
        var fired = 0
        let once = {
            lock.lock()
            let alreadyDone = completed
            if !alreadyDone { completed = true }
            lock.unlock()
            if !alreadyDone { fired += 1 }
        }
        once(); once()
        guard fired == 1 else {
            fputs("native-host self-test watchdog exactly-once failed\n", stderr)
            return 1
        }
        print("native-host self-test passed")
        return 0
    } catch {
        fputs("native-host self-test failed: \(error.localizedDescription)\n", stderr)
        return 1
    }
}

if CommandLine.arguments.contains("--self-test") { exit(runSelfTest()) }

MainActor.assumeIsolated {
    let application = NSApplication.shared
    let delegate = AppDelegate()
    application.delegate = delegate
    application.run()
}
