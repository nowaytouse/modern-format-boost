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
    private let lock = NSLock()

    private init() {
        language = UserDefaults.standard.string(forKey: languagePreferenceKey)
            .flatMap(AppLanguage.init(rawValue:)) ?? .system
        bundle = Self.bundle(for: language)
    }

    func select(_ language: AppLanguage) {
        lock.lock()
        self.language = language
        bundle = Self.bundle(for: language)
        lock.unlock()
        UserDefaults.standard.set(language.rawValue, forKey: languagePreferenceKey)
    }

    func text(_ key: String) -> String {
        lock.lock()
        let selectedBundle = bundle
        lock.unlock()
        return selectedBundle.localizedString(forKey: key, value: key, table: nil)
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
    case compare
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
        case .compare: "compare"
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

    var capabilities: OperationCapabilities {
        switch self {
        case .adjacent:
            OperationCapabilities(
                usesProcessingSelection: true,
                supportsUltimate: true,
                supportsResume: true,
                supportsArchive: true,
                supportsStandardOptions: true
            )
        case .fastImgJxl, .fastImgAvif:
            OperationCapabilities(
                fixedProcessingMode: .imagesOnly,
                supportsUltimate: true,
                supportsShortestPath: true,
                supportsResume: true,
                supportsArchive: true,
                supportsRetry: true
            )
        case .fastVid:
            OperationCapabilities(fixedProcessingMode: .videosOnly, supportsShortestPath: true)
        case .restoreJpeg:
            OperationCapabilities(fixedProcessingMode: .imagesOnly)
        case .collect, .compare, .mergeXmp, .iCloudImport, .diagnostic, .cacheClean, .databaseManager:
            OperationCapabilities()
        }
    }
}

private struct OperationCapabilities {
    var usesProcessingSelection = false
    var fixedProcessingMode: ProcessingMode? = nil
    var supportsUltimate = false
    var supportsShortestPath = false
    var supportsResume = false
    var supportsArchive = false
    var supportsRetry = false
    var supportsStandardOptions = false

    func resolvedProcessingMode(_ selected: ProcessingMode) -> ProcessingMode? {
        fixedProcessingMode ?? (usesProcessingSelection ? selected : nil)
    }
}

private struct ProcessorRequest {
    let targetPath: String
    let processingMode: ProcessingMode
    let operationMode: OperationMode
    var backupPath: String? = nil
    var ultimate = true
    var verbose = false
    var shortestPath = false
    var resume = false
    var fresh = false
    var archive = false
    var retry = false
    var force = false
    var dryRun = false
    var plain = false
    var inPlace = false
    var watch = false
    var photosContainer: PhotosAuditContainer?
}

private enum PhotosAuditContainerKind: String, Decodable {
    case folder
    case album

    var argument: String {
        switch self {
        case .folder: "--photos-folder-id"
        case .album: "--photos-album-id"
        }
    }
}

private struct PhotosAuditContainer: Decodable, Equatable {
    let kind: PhotosAuditContainerKind
    let id: String
    let name: String
    let parentID: String?
    let path: [String]

    enum CodingKeys: String, CodingKey {
        case kind, id, name, path
        case parentID = "parent_id"
    }
}

private enum ProcessorCommand {
    static func arguments(from request: ProcessorRequest) throws -> [String] {
        guard !request.targetPath.isEmpty else {
            throw HostError(message: localized("error.select_target"))
        }

        let capabilities = request.operationMode.capabilities
        guard !request.shortestPath || capabilities.supportsShortestPath else {
            throw HostError(message: localized("error.option_unavailable"))
        }
        guard !request.ultimate || capabilities.supportsUltimate else {
            throw HostError(message: localized("error.option_unavailable"))
        }
        guard (!request.resume && !request.fresh) || capabilities.supportsResume else {
            throw HostError(message: localized("error.option_unavailable"))
        }
        guard !((request.resume || request.retry) && request.fresh) else {
            throw HostError(message: localized("error.resume_conflict"))
        }
        guard !request.archive || capabilities.supportsArchive,
              !request.retry || capabilities.supportsRetry,
              !(request.force || request.plain || request.inPlace) || capabilities.supportsStandardOptions
        else { throw HostError(message: localized("error.option_unavailable")) }
        if request.watch {
            var isDirectory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: request.targetPath, isDirectory: &isDirectory),
                  isDirectory.boolValue
            else { throw HostError(message: localized("error.watch_directory")) }
        }
        if request.photosContainer != nil {
            guard request.operationMode == .restoreJpeg,
                  isPhotosLibraryPackagePath(request.targetPath)
            else {
                throw HostError(message: localized("error.photos_scope_unavailable"))
            }
        }
        if request.operationMode == .collect || request.operationMode == .compare {
            guard let backup = request.backupPath, !backup.isEmpty else {
                throw HostError(message: localized("error.select_backup"))
            }
            guard isPhotosLibraryPackagePath(backup)
                == isPhotosLibraryPackagePath(request.targetPath)
            else {
                throw HostError(message: localized("error.backup_kind_mismatch"))
            }
            if request.operationMode == .compare {
                guard isPhotosLibraryPackagePath(request.targetPath),
                      isPhotosLibraryPackagePath(backup)
                else {
                    throw HostError(message: localized("error.photos_compare_required"))
                }
            }
        } else if request.backupPath != nil {
            throw HostError(message: localized("error.option_unavailable"))
        }

        var arguments: [String] = []
        if let processing = capabilities.resolvedProcessingMode(request.processingMode),
           let mode = processing.argument
        {
            arguments.append(mode)
        }
        if let mode = request.operationMode.backendMode { arguments += ["--mode", mode] }
        if let strategy = request.operationMode.strategy {
            arguments += ["--strategy", strategy]
        }
        if request.ultimate { arguments.append("--ultimate") }
        if request.verbose { arguments.append("--verbose") }
        if request.archive { arguments.append("--archive") }
        if request.retry { arguments.append("--retry") }
        if request.force { arguments.append("--force") }
        if request.dryRun { arguments.append("--dry-run") }
        if request.plain { arguments.append("--plain") }
        if request.inPlace { arguments.append("--in-place") }
        if request.watch { arguments.append("--watch") }
        if request.shortestPath {
            arguments.append("--shortest-path")
        }
        if request.resume {
            arguments.append("--resume")
        } else if request.fresh {
            arguments.append("--no-resume")
        }
        if let container = request.photosContainer {
            arguments += [container.kind.argument, container.id]
        }
        if let backup = request.backupPath { arguments += ["--backup", backup] }
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

private func conciseProcessLog(_ text: String) -> String? {
    let signals = [
        "error", "fail", "warn", "complete", "finished", "summary", "processed", "converted",
        "skipped", "progress", "MFB_RESUME_DECISION_REQUIRED", "[ERROR]", "[WARN]", "[SUMMARY]",
        "[PROGRESS]", "[SUCCESS]", "✓", "✗", "▶︎",
        "失败", "错误", "警告", "完成", "进度", "失敗", "エラー", "警告", "完了", "進捗",
    ]
    let lines = text.split(separator: "\n", omittingEmptySubsequences: true).map(String.init)
    let kept = lines.filter { line in
        let lowered = line.lowercased()
        if lowered.range(of: #"^(?:err:\s*)?\[?(?:debug|trace)(?:\s|\]|:)"#, options: .regularExpression) != nil {
            return false
        }
        return signals.contains { lowered.contains($0.lowercased()) }
            || lowered.range(of: #"\b\d{1,3}%(?!\w)"#, options: .regularExpression) != nil
            || lowered.range(of: #"^(?:err:\s*)?\[(?:scan|copy|encode|meme mode|verify|import|skip|retain|done|restore|resume|final)\s*\]"#, options: .regularExpression) != nil
            // Keep unclassified stderr diagnostics, but not routine INFO chatter.
            || (lowered.hasPrefix("err:")
                && lowered.range(of: #"^err:\s*\[?info(?:\s|\]|:)"#, options: .regularExpression) == nil)
    }
    return kept.isEmpty ? nil : kept.joined(separator: "\n")
}

private func isPhotosLibraryPath(_ path: String) -> Bool {
    URL(fileURLWithPath: path)
        .resolvingSymlinksInPath()
        .standardized.pathComponents.contains { component in
        let component = component.lowercased()
        return component.hasSuffix(".photoslibrary") || component.hasSuffix(".photolibrary")
    }
}

private func isPhotosLibraryPackagePath(_ path: String) -> Bool {
    let component = URL(fileURLWithPath: path)
        .resolvingSymlinksInPath()
        .standardized.lastPathComponent.lowercased()
    return component.hasSuffix(".photoslibrary") || component.hasSuffix(".photolibrary")
}

private func isPhotosUUID(_ value: String) -> Bool {
    let parts = value.split(separator: "-", omittingEmptySubsequences: false)
    guard parts.map(\.count) == [8, 4, 4, 4, 12] else { return false }
    return parts.joined().allSatisfy { $0.isHexDigit }
}

private func processingRequiresPhotosAutomation(_ request: ProcessorRequest) -> Bool {
    !request.dryRun && (request.operationMode == .iCloudImport
        || (request.shortestPath && request.operationMode.capabilities.supportsShortestPath)
        || (request.operationMode == .restoreJpeg && isPhotosLibraryPath(request.targetPath)))
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

private func readBoundedProcessOutput(
    _ handle: FileHandle,
    limit: Int
) throws -> (data: Data, exceeded: Bool) {
    precondition(limit >= 0 && limit < Int.max)
    let sentinelLimit = limit + 1
    var data = Data()
    while let chunk = try handle.read(upToCount: maxProcessLogChunkBytes), !chunk.isEmpty {
        if data.count < sentinelLimit {
            data.append(chunk.prefix(sentinelLimit - data.count))
        }
    }
    let exceeded = data.count > limit
    if exceeded {
        data.removeSubrange(limit...)
    }
    return (data, exceeded)
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

    static func resolveTool(named name: String) -> URL? {
        var seen = Set<String>()
        for processor in candidates() {
            let directory = processor.deletingLastPathComponent()
            for candidate in [
                directory.appendingPathComponent(name),
                directory.deletingLastPathComponent().appendingPathComponent(name),
            ] where seen.insert(candidate.path).inserted
                && FileManager.default.isExecutableFile(atPath: candidate.path)
            {
                return candidate
            }
        }
        let bundleURL = Bundle.main.bundleURL.standardized
        let isAppBundle = bundleURL.pathExtension == "app"
        let isBuildBundle = bundleURL.path.contains("/target/release/bundle/macos/")
        if var ancestor = Bundle.main.executableURL?.deletingLastPathComponent() {
            while ancestor.path != "/" {
                let isReleaseRoot = ancestor.lastPathComponent == "release"
                    && ancestor.deletingLastPathComponent().lastPathComponent == "target"
                if !isAppBundle || (isBuildBundle && isReleaseRoot) {
                    let candidate = ancestor.appendingPathComponent(name)
                    if seen.insert(candidate.path).inserted,
                       FileManager.default.isExecutableFile(atPath: candidate.path)
                    {
                        return candidate
                    }
                }
                ancestor.deleteLastPathComponent()
            }
        }
        return nil
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

private func queryPhotosAuditContainers(binary: URL, library: String) throws -> [PhotosAuditContainer] {
    let process = Process()
    process.executableURL = binary
    process.arguments = ["photos-albums", library, "--json"]
    let combined = Pipe()
    process.standardOutput = combined
    process.standardError = combined
    try process.run()
    let capture = try readBoundedProcessOutput(
        combined.fileHandleForReading,
        limit: 16 * 1024 * 1024
    )
    process.waitUntilExit()
    guard !capture.exceeded else {
        throw HostError(message: localized("error.photos_scope_too_large"))
    }
    guard process.terminationStatus == 0 else {
        let detail = String(decoding: capture.data.prefix(8_192), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        throw HostError(message: localized("error.photos_scope_load", detail))
    }
    let containers = try JSONDecoder().decode([PhotosAuditContainer].self, from: capture.data)
    let ids = Set(containers.map(\.id))
    guard ids.count == containers.count,
          containers.allSatisfy({
              isPhotosUUID($0.id)
                  && !$0.name.isEmpty
                  && !$0.path.isEmpty
                  && $0.path.last == $0.name
                  && $0.parentID.map { isPhotosUUID($0) && ids.contains($0) } ?? true
          })
    else {
        throw HostError(message: localized("error.photos_scope_invalid"))
    }
    return containers
}

@MainActor
private final class NativeHost {
    var onLog: ((String) -> Void)?
    var onCompletion: ((Result<String, Error>) -> Void)?
    private var activeProcess: Process?
    private let processLogs = ProcessLogBackpressure()
    private var pendingProcessCompletion: (() -> Void)?

    var isRunning: Bool { activeProcess != nil }

    func loadPhotosAuditContainers(
        library: String,
        completion: @escaping (Result<[PhotosAuditContainer], Error>) -> Void
    ) {
        guard activeProcess == nil else {
            completion(.failure(HostError(message: localized("error.task_running"))))
            return
        }
        guard let binary = ProcessorLocator.resolveTool(named: "img") else {
            completion(.failure(HostError(message: localized("error.img_backend_missing"))))
            return
        }
        requestPhotosAutomationPermission { result in
            switch result {
            case let .failure(error):
                completion(.failure(error))
            case .success:
                DispatchQueue.global(qos: .userInitiated).async {
                    let result = Result {
                        try queryPhotosAuditContainers(binary: binary, library: library)
                    }
                    DispatchQueue.main.async { completion(result) }
                }
            }
        }
    }

    nonisolated func checkVersionAlignment() -> String {
        guard let binary = ProcessorLocator.resolve() else {
            return localized("status.processor_unavailable")
        }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["--help"]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = output
        do {
            try process.run()
            let watchdog = DispatchWorkItem {
                if process.isRunning { kill(process.processIdentifier, SIGKILL) }
            }
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 10, execute: watchdog)
            defer { watchdog.cancel() }
            let capture = try readBoundedProcessOutput(
                output.fileHandleForReading,
                limit: maxProcessLogChunkBytes
            )
            process.waitUntilExit()
            if process.terminationStatus == 0 {
                return localized("status.processor_ready")
            }
            var diagnostic = String(decoding: capture.data, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if capture.exceeded {
                if !diagnostic.isEmpty { diagnostic.append("\n") }
                diagnostic.append(localized("log.omitted", UInt64(1)))
            }
            return diagnostic.isEmpty
                ? localized("status.processor_failed")
                : "\(localized("status.processor_failed")): \(diagnostic)"
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
            DispatchQueue.main.async { [weak self] in self?.flushProcessLogs() }
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
    var acceptsDrops = true

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        registerForDraggedTypes([.fileURL])
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        registerForDraggedTypes([.fileURL])
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        acceptsDrops && sender.draggingPasteboard.canReadObject(forClasses: [NSURL.self]) ? .copy : []
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard acceptsDrops, let urls = sender.draggingPasteboard.readObjects(
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
    private let photosScopeLabel = NSTextField(labelWithString: "")
    private let metadataSafetyLabel = NSTextField(wrappingLabelWithString: "")
    private let languageLabel = NSTextField(labelWithString: "")
    private let appearanceLabel = NSTextField(labelWithString: "")
    private let targetField = NSTextField()
    private let backupLabel = NSTextField(labelWithString: "")
    private let backupField = NSTextField()
    private let processingPopup = NSPopUpButton()
    private let operationPopup = NSPopUpButton()
    private let languagePopup = NSPopUpButton()
    private let appearancePopup = NSPopUpButton()
    private let ultimateCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let verboseCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let shortestPathCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let resumeCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let freshCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let archiveCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let retryCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let forceCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let dryRunCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let plainCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let inPlaceCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let watchCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let commandField = NSTextField()
    private let logView = NSTextView()
    private let statusLabel = NSTextField(labelWithString: "")
    private let progressIndicator = NSProgressIndicator()
    private let chooseButton = NSButton(title: "", target: nil, action: nil)
    private let backupButton = NSButton(title: "", target: nil, action: nil)
    private let backupRow = NSStackView()
    private let photosScopeButton = NSButton(title: "", target: nil, action: nil)
    private let photosScopeRow = NSStackView()
    private let openButton = NSButton(title: "", target: nil, action: nil)
    private let copyButton = NSButton(title: "", target: nil, action: nil)
    private let runButton = NSButton(title: "", target: nil, action: nil)
    private var lastRequest: ProcessorRequest?
    private var sawResumeDecision = false
    private var configurationControlsEnabled = true
    private var processorStatus = ""
    private var selectedPhotosContainer: PhotosAuditContainer?
    private var processingStartedAt: TimeInterval?
    private var refreshTimer: Timer?
    private var processingActivity: NSObjectProtocol?

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
        statusLabel.stringValue = localized("status.checking_processor")
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else { return }
            let status = self.host.checkVersionAlignment()
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.processorStatus = status
                if self.configurationControlsEnabled { self.statusLabel.stringValue = status }
            }
        }
    }

    func windowWillClose(_ notification: Notification) {
        host.terminateActiveProcess()
        setProcessing(false)
        NSApp.terminate(nil)
    }

    func windowDidBecomeKey(_ notification: Notification) {
        refreshProcessingStatus()
    }

    @objc private func refreshProcessingStatus() {
        guard let startedAt = processingStartedAt else { return }
        let elapsed = max(0, Int(ProcessInfo.processInfo.systemUptime - startedAt))
        statusLabel.stringValue = localized("status.running_elapsed", elapsed / 60, elapsed % 60)
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
        targetField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        chooseButton.target = self
        chooseButton.action = #selector(chooseTarget)
        let targetRow = NSStackView(views: [targetField, chooseButton])
        targetRow.orientation = .horizontal
        targetRow.spacing = 8
        targetField.setContentHuggingPriority(.defaultLow, for: .horizontal)

        backupField.isEditable = false
        backupField.isSelectable = true
        backupField.lineBreakMode = .byTruncatingMiddle
        backupField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        backupButton.target = self
        backupButton.action = #selector(chooseBackup)
        backupLabel.alignment = .right
        backupLabel.widthAnchor.constraint(equalToConstant: 120).isActive = true
        backupRow.addArrangedSubview(backupLabel)
        backupRow.addArrangedSubview(backupField)
        backupRow.addArrangedSubview(backupButton)
        backupRow.orientation = .horizontal
        backupRow.alignment = .centerY
        backupRow.spacing = 8
        backupField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        backupRow.isHidden = true

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

        photosScopeButton.target = self
        photosScopeButton.action = #selector(choosePhotosScope)
        photosScopeButton.lineBreakMode = .byTruncatingMiddle
        photosScopeButton.setContentHuggingPriority(.defaultLow, for: .horizontal)
        photosScopeButton.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        photosScopeLabel.alignment = .right
        photosScopeLabel.widthAnchor.constraint(equalToConstant: 120).isActive = true
        photosScopeRow.addArrangedSubview(photosScopeLabel)
        photosScopeRow.addArrangedSubview(photosScopeButton)
        photosScopeRow.orientation = .horizontal
        photosScopeRow.alignment = .centerY
        photosScopeRow.spacing = 12
        photosScopeRow.isHidden = true

        metadataSafetyLabel.font = .systemFont(ofSize: 11)
        metadataSafetyLabel.textColor = .secondaryLabelColor
        metadataSafetyLabel.maximumNumberOfLines = 3

        ultimateCheck.state = .on
        for control in [
            ultimateCheck, verboseCheck, shortestPathCheck, archiveCheck,
            forceCheck, dryRunCheck, plainCheck, inPlaceCheck, watchCheck,
        ] {
            control.target = self
            control.action = #selector(configurationChanged)
        }
        for control in [resumeCheck, freshCheck, retryCheck] {
            control.target = self
            control.action = #selector(resumeChoiceChanged(_:))
        }
        let options = NSGridView(views: [
            [ultimateCheck, shortestPathCheck, verboseCheck, dryRunCheck],
            [resumeCheck, freshCheck, archiveCheck, retryCheck],
            [forceCheck, plainCheck, inPlaceCheck, watchCheck],
        ])
        options.rowSpacing = 5
        options.columnSpacing = 12
        for column in 0 ..< 4 { options.column(at: column).xPlacement = .leading }

        commandField.isEditable = false
        commandField.isSelectable = true
        commandField.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        commandField.textColor = .secondaryLabelColor
        commandField.lineBreakMode = .byTruncatingMiddle
        commandField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

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
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
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
            header, targetRow, grid, backupRow, photosScopeRow, metadataSafetyLabel, options, commandField, actionRow,
            logScroll, statusRow,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        for view in [
            header, targetRow, grid, backupRow, photosScopeRow, metadataSafetyLabel, options, commandField, actionRow,
            logScroll, statusRow,
        ] {
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

    @objc private func chooseBackup() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.resolvesAliases = true
        panel.title = localized("panel.backup.title")
        if panel.runModal() == .OK, let path = panel.url?.path {
            backupField.stringValue = path
            configurationChanged()
        }
    }

    private func acceptTarget(_ path: String) {
        guard configurationControlsEnabled else {
            appendLog("✗ \(localized("error.task_running"))")
            return
        }
        selectedPhotosContainer = nil
        targetField.stringValue = path
        configurationChanged()
    }

    @objc private func choosePhotosScope() {
        let library = targetField.stringValue
        guard selectedOperation == .restoreJpeg,
              isPhotosLibraryPackagePath(library),
              !host.isRunning
        else {
            present(HostError(message: localized("error.photos_scope_unavailable")))
            return
        }
        photosScopeButton.isEnabled = false
        statusLabel.stringValue = localized("status.photos_scope_loading")
        host.loadPhotosAuditContainers(library: library) { [weak self] result in
            guard let self, self.targetField.stringValue == library,
                  self.selectedOperation == .restoreJpeg, self.configurationControlsEnabled
            else { return }
            self.applyCapabilityState()
            switch result {
            case let .success(containers):
                self.presentPhotosScopePicker(containers)
            case let .failure(error):
                self.present(error)
            }
        }
    }

    private func presentPhotosScopePicker(_ containers: [PhotosAuditContainer]) {
        let alert = NSAlert()
        alert.messageText = localized("photos.scope.picker.title")
        alert.informativeText = localized("photos.scope.picker.info", containers.count)
        alert.addButton(withTitle: localized("button.select"))
        alert.addButton(withTitle: localized("alert.cancel"))
        let popup = NSPopUpButton(frame: NSRect(x: 0, y: 0, width: 560, height: 26))
        popup.addItem(withTitle: localized("photos.scope.entire_library"))
        for container in containers {
            let kind = localized("photos.scope.\(container.kind.rawValue)")
            popup.addItem(withTitle: "\(kind)  \(container.path.joined(separator: " › "))")
        }
        if let selectedPhotosContainer,
           let index = containers.firstIndex(of: selectedPhotosContainer)
        {
            popup.selectItem(at: index + 1)
        }
        alert.accessoryView = popup
        if alert.runModal() == .alertFirstButtonReturn {
            selectedPhotosContainer = popup.indexOfSelectedItem == 0
                ? nil
                : containers[safe: popup.indexOfSelectedItem - 1]
            configurationChanged()
        } else {
            statusLabel.stringValue = localized("status.ready")
        }
    }

    @objc private func configurationChanged() {
        applyCapabilityState()
        updateMetadataSafetyNotice()
        guard configurationControlsEnabled else { return }
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

    @objc private func resumeChoiceChanged(_ sender: NSButton) {
        if selectedOperation.backendMode == "fast-img", sender !== freshCheck {
            // The fast-img backend aliases resume and retry; show both effective flags.
            resumeCheck.state = sender.state
            retryCheck.state = sender.state
        }
        if sender.state == .on {
            if sender === freshCheck {
                resumeCheck.state = .off
                retryCheck.state = .off
            } else {
                freshCheck.state = .off
            }
        }
        configurationChanged()
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
            appendLog(verboseCheck.state == .on
                ? "▶︎ \(try host.terminalCommand(for: request))"
                : "▶︎ \(localized("log.task_start"))")
            host.startProcessing(request)
        } catch {
            setProcessing(false)
            present(error)
        }
    }

    private var selectedOperation: OperationMode {
        OperationMode.allCases[safe: operationPopup.indexOfSelectedItem] ?? .adjacent
    }

    private func applyCapabilityState() {
        let capabilities = selectedOperation.capabilities
        if let fixed = capabilities.fixedProcessingMode {
            processingPopup.selectItem(
                at: ProcessingMode.allCases.firstIndex { $0.rawValue == fixed.rawValue } ?? 0,
            )
        }
        processingPopup.isEnabled = configurationControlsEnabled && capabilities.usesProcessingSelection
        // The launcher always enables --ultimate for supported operations.
        // Show the effective policy instead of an off switch it cannot honor.
        ultimateCheck.isEnabled = false
        ultimateCheck.state = capabilities.supportsUltimate ? .on : .off
        shortestPathCheck.isEnabled = configurationControlsEnabled && capabilities.supportsShortestPath
        resumeCheck.isEnabled = configurationControlsEnabled && capabilities.supportsResume
        freshCheck.isEnabled = configurationControlsEnabled && capabilities.supportsResume
        archiveCheck.isEnabled = configurationControlsEnabled && capabilities.supportsArchive
        retryCheck.isEnabled = configurationControlsEnabled && capabilities.supportsRetry
        if selectedOperation.backendMode == "fast-img", resumeCheck.state == .on {
            retryCheck.state = .on
        }
        for control in [forceCheck, plainCheck, inPlaceCheck] {
            control.isEnabled = configurationControlsEnabled && capabilities.supportsStandardOptions
        }
        dryRunCheck.isEnabled = configurationControlsEnabled
        watchCheck.isEnabled = configurationControlsEnabled
        let backupAvailable = selectedOperation == .collect || selectedOperation == .compare
        backupRow.isHidden = !backupAvailable
        backupButton.isEnabled = configurationControlsEnabled && backupAvailable
        let photosScopeAvailable = selectedOperation == .restoreJpeg
            && isPhotosLibraryPackagePath(targetField.stringValue)
        if !photosScopeAvailable { selectedPhotosContainer = nil }
        photosScopeRow.isHidden = !photosScopeAvailable
        photosScopeButton.isEnabled = configurationControlsEnabled && photosScopeAvailable
        photosScopeButton.title = selectedPhotosContainer.map {
            let kind = localized("photos.scope.\($0.kind.rawValue)")
            return "\(kind)  \($0.path.joined(separator: " › "))"
        } ?? localized("photos.scope.entire_library")
        // Disabling controls while a job runs must not discard the user's
        // selections; only unsupported options should be cleared.
        if !capabilities.supportsUltimate { ultimateCheck.state = .off }
        if !capabilities.supportsShortestPath { shortestPathCheck.state = .off }
        if !capabilities.supportsResume { resumeCheck.state = .off }
        if !capabilities.supportsResume { freshCheck.state = .off }
        if !capabilities.supportsArchive { archiveCheck.state = .off }
        if !capabilities.supportsRetry { retryCheck.state = .off }
        if !capabilities.supportsStandardOptions {
            for control in [forceCheck, plainCheck, inPlaceCheck] { control.state = .off }
        }
    }

    private func updateMetadataSafetyNotice() {
        let key: String?
        switch selectedOperation {
        case .adjacent, .fastImgJxl, .mergeXmp:
            key = "metadata.jxl_overlay"
        case .restoreJpeg:
            key = "metadata.restore_auto"
        case .collect:
            key = "metadata.recovery_collect"
        case .compare:
            key = "metadata.backup_compare"
        default:
            key = nil
        }
        metadataSafetyLabel.isHidden = key == nil
        metadataSafetyLabel.stringValue = key.map { localized($0) } ?? ""
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
            backupPath: selectedOperation == .collect || selectedOperation == .compare
                ? backupField.stringValue : nil,
            ultimate: ultimateCheck.state == .on,
            verbose: verboseCheck.state == .on,
            shortestPath: shortestPathCheck.state == .on,
            resume: resumeCheck.state == .on,
            fresh: freshCheck.state == .on,
            archive: archiveCheck.state == .on,
            retry: retryCheck.state == .on,
            force: forceCheck.state == .on,
            dryRun: dryRunCheck.state == .on,
            plain: plainCheck.state == .on,
            inPlace: inPlaceCheck.state == .on,
            watch: watchCheck.state == .on,
            photosContainer: selectedPhotosContainer,
        )
    }

    private func appendLog(_ text: String) {
        if text.contains("MFB_RESUME_DECISION_REQUIRED") { sawResumeDecision = true }
        guard let visible = verboseCheck.state == .on ? text : conciseProcessLog(text) else { return }
        let next = logView.string.isEmpty ? visible : "\(logView.string)\n\(visible)"
        let lines = next.split(separator: "\n", omittingEmptySubsequences: false)
        logView.string = lines.count > 3_000 ? lines.suffix(3_000).joined(separator: "\n") : next
        logView.scrollToEndOfDocument(nil)
    }

    private func applyResumeDecision(fresh: Bool, to request: inout ProcessorRequest) {
        request.resume = !fresh
        request.fresh = fresh
        request.retry = !fresh && (request.retry || request.operationMode.backendMode == "fast-img")
        resumeCheck.state = request.resume ? .on : .off
        freshCheck.state = request.fresh ? .on : .off
        retryCheck.state = request.retry ? .on : .off
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
                    applyResumeDecision(fresh: false, to: &retry)
                case .alertSecondButtonReturn:
                    applyResumeDecision(fresh: true, to: &retry)
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
        backupField.placeholderString = localized("field.backup.placeholder")
        backupLabel.stringValue = localized("field.backup")
        mediaLabel.stringValue = localized("field.media")
        operationLabel.stringValue = localized("field.operation")
        photosScopeLabel.stringValue = localized("field.photos_scope")
        languageLabel.stringValue = localized("field.language")
        appearanceLabel.stringValue = localized("field.appearance")
        chooseButton.title = localized("button.choose")
        backupButton.title = localized("button.choose")
        openButton.title = localized("button.open_terminal")
        copyButton.title = localized("button.copy_command")
        runButton.title = localized("button.run")
        ultimateCheck.title = localized("option.ultimate")
        verboseCheck.title = localized("option.verbose")
        shortestPathCheck.title = localized("option.shortest_path")
        resumeCheck.title = localized("option.resume")
        freshCheck.title = localized("option.fresh")
        archiveCheck.title = localized("option.archive")
        retryCheck.title = localized("option.retry")
        forceCheck.title = localized("option.force")
        dryRunCheck.title = localized("option.dry_run")
        plainCheck.title = localized("option.plain")
        inPlaceCheck.title = localized("option.in_place")
        watchCheck.title = localized("option.watch")
        for (control, key) in [
            (ultimateCheck, "option.ultimate.help"), (verboseCheck, "option.verbose.help"),
            (shortestPathCheck, "option.shortest_path.help"), (resumeCheck, "option.resume.help"),
            (freshCheck, "option.fresh.help"), (archiveCheck, "option.archive.help"),
            (retryCheck, "option.retry.help"), (forceCheck, "option.force.help"),
            (dryRunCheck, "option.dry_run.help"), (plainCheck, "option.plain.help"),
            (inPlaceCheck, "option.in_place.help"), (watchCheck, "option.watch.help"),
        ] { control.toolTip = localized(key) }
        commandField.placeholderString = localized("command.placeholder")
        replaceTitles(processingPopup, with: [
            localized("media.both"), localized("media.images"), localized("media.videos"),
        ])
        replaceTitles(operationPopup, with: [
            localized("operation.adjacent"), localized("operation.fast_jxl"),
            localized("operation.fast_avif"), localized("operation.fast_video"),
            localized("operation.restore_jpeg"), localized("operation.collect"), localized("operation.compare"),
            localized("operation.merge_xmp"), localized("operation.icloud_import"),
            localized("operation.diagnostic"), localized("operation.cache_clean"),
            localized("operation.database"),
        ])
        replaceTitles(languagePopup, with: AppLanguage.allCases.map(\.nativeTitle))
        replaceTitles(appearancePopup, with: AppAppearance.allCases.map(\.localizedTitle))
        refreshProcessingStatus()
    }

    private func setProcessing(_ processing: Bool) {
        configurationControlsEnabled = !processing
        (window.contentView as? NativeDropView)?.acceptsDrops = !processing
        for control in [
            chooseButton, backupButton, operationPopup, openButton, copyButton, runButton,
        ] {
            control.isEnabled = !processing
        }
        applyCapabilityState()
        if processing {
            if processingStartedAt == nil {
                processingStartedAt = ProcessInfo.processInfo.systemUptime
                processingActivity = ProcessInfo.processInfo.beginActivity(
                    options: .userInitiatedAllowingIdleSystemSleep,
                    reason: "Processing user-selected media"
                )
                let timer = Timer(timeInterval: 1, target: self,
                                  selector: #selector(refreshProcessingStatus), userInfo: nil, repeats: true)
                RunLoop.main.add(timer, forMode: .common)
                refreshTimer = timer
            }
            progressIndicator.startAnimation(nil)
            refreshProcessingStatus()
        } else {
            refreshTimer?.invalidate()
            refreshTimer = nil
            processingStartedAt = nil
            if let activity = processingActivity { ProcessInfo.processInfo.endActivity(activity) }
            processingActivity = nil
            progressIndicator.stopAnimation(nil)
        }
    }

    func validateInterfaceForSelfTest() throws {
        guard let content = window.contentView else { throw HostError(message: "Missing content view") }
        let originalFrame = window.frame
        let longPath = "/tmp/" + String(repeating: "long folder name/", count: 160)
        targetField.stringValue = longPath + "test.photoslibrary"
        backupField.stringValue = longPath + "backup"
        commandField.stringValue = longPath + "command"
        statusLabel.stringValue = longPath
        photosScopeButton.title = longPath
        for operation in [OperationMode.adjacent, .collect, .restoreJpeg] {
            operationPopup.selectItem(at: OperationMode.allCases.firstIndex(of: operation)!)
            applyCapabilityState()
            updateMetadataSafetyNotice()
            photosScopeButton.title = longPath
            content.layoutSubtreeIfNeeded()
            let visibleFields: [NSView] = [targetField, commandField, statusLabel]
                + (backupRow.isHidden ? [] : [backupField])
                + (photosScopeRow.isHidden ? [] : [photosScopeButton])
            guard window.frame.size == originalFrame.size,
                  content.frame.size == mainWindowContentSize,
                  visibleFields.allSatisfy({
                      let frame = $0.convert($0.bounds, to: content)
                      return frame.minX >= 0 && frame.maxX <= content.bounds.maxX + 1
                  })
            else {
                let fields = visibleFields.map { NSStringFromRect($0.convert($0.bounds, to: content)) }
                throw HostError(message: "Long-path layout (\(operation)): window \(originalFrame.size) -> \(window.frame.size), content \(content.frame.size), fields \(fields)")
            }
        }

        operationPopup.selectItem(at: OperationMode.allCases.firstIndex(of: .fastImgJxl)!)
        applyCapabilityState()
        guard ultimateCheck.state == .on, !ultimateCheck.isEnabled,
              archiveCheck.isEnabled, retryCheck.isEnabled, !forceCheck.isEnabled,
              verboseCheck.state == .off
        else { throw HostError(message: "Incorrect option capabilities or defaults") }
        retryCheck.state = .on
        freshCheck.state = .on
        resumeChoiceChanged(freshCheck)
        guard retryCheck.state == .off, resumeCheck.state == .off else {
            throw HostError(message: "Fresh run left conflicting retry options enabled")
        }
        var recovery = ProcessorRequest(
            targetPath: "/tmp/media", processingMode: .imagesOnly, operationMode: .fastImgJxl,
            resume: true, retry: true
        )
        applyResumeDecision(fresh: true, to: &recovery)
        let freshArguments = try ProcessorCommand.arguments(from: recovery)
        guard freshArguments.contains("--no-resume"), !freshArguments.contains("--retry"),
              freshCheck.state == .on, resumeCheck.state == .off, retryCheck.state == .off
        else { throw HostError(message: "Fresh recovery decision disagrees with option state") }
        applyResumeDecision(fresh: false, to: &recovery)
        let resumeArguments = try ProcessorCommand.arguments(from: recovery)
        guard resumeArguments.contains("--resume"), !resumeArguments.contains("--no-resume"),
              resumeArguments.contains("--retry"), resumeCheck.state == .on,
              retryCheck.state == .on, freshCheck.state == .off
        else { throw HostError(message: "Resume recovery decision changed retry policy") }
        retryCheck.state = .off
        resumeChoiceChanged(retryCheck)
        guard resumeCheck.state == .off, retryCheck.state == .off else {
            throw HostError(message: "Fast-img resume alias stayed active after deselection")
        }
        setProcessing(true)
        defer { setProcessing(false) }
        processingStartedAt = ProcessInfo.processInfo.systemUptime - 65
        refreshProcessingStatus()
        let runningStatus = statusLabel.stringValue
        let runningTarget = targetField.stringValue
        acceptTarget("/tmp/another-job")
        verboseCheck.state = .on
        configurationChanged()
        guard statusLabel.stringValue == runningStatus,
              runningStatus == localized("status.running_elapsed", 1, 5),
              targetField.stringValue == runningTarget,
              (window.contentView as? NativeDropView)?.acceptsDrops == false,
              verboseCheck.isEnabled, !runButton.isEnabled, !dryRunCheck.isEnabled
        else { throw HostError(message: "Background status or running controls lost their state") }
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
        if Bundle.main.bundleURL.pathExtension == "app",
           ProcessorLocator.resolveTool(named: "img") == nil
        {
            fputs("native-host self-test bundled img lookup failed\n", stderr)
            return 1
        }
        let request = ProcessorRequest(
            targetPath: "/tmp/media", processingMode: .imagesOnly, operationMode: .fastImgJxl,
            ultimate: true, verbose: true, shortestPath: true, resume: true,
        )
        let expected = [
            "--images-only", "--mode", "fast-img", "--strategy", "jxl", "--ultimate",
            "--verbose", "--shortest-path", "--resume", "/tmp/media",
        ]
        guard try ProcessorCommand.arguments(from: request) == expected else {
            fputs("native-host self-test argument mapping failed\n", stderr)
            return 1
        }
        let standard = ProcessorRequest(
            targetPath: "/tmp", processingMode: .imagesOnly, operationMode: .adjacent,
            archive: true, force: true, dryRun: true, plain: true, inPlace: true, watch: true
        )
        guard try ProcessorCommand.arguments(from: standard) == [
            "--images-only", "--ultimate", "--archive", "--force", "--dry-run",
            "--plain", "--in-place", "--watch", "/tmp",
        ] else {
            fputs("native-host self-test standard flags or quiet default failed\n", stderr)
            return 1
        }
        do {
            _ = try ProcessorCommand.arguments(from: ProcessorRequest(
                targetPath: "/tmp/media", processingMode: .imagesOnly, operationMode: .fastImgJxl,
                fresh: true, retry: true
            ))
            fputs("native-host self-test retry/fresh conflict was accepted\n", stderr)
            return 1
        } catch {}
        guard conciseProcessLog("DEBUG codec internals") == nil,
              conciseProcessLog("ERR: DEBUG codec internals") == nil,
              conciseProcessLog("ERR: INFO codec internals") == nil,
              conciseProcessLog("[ENCODE] 1/20 synthetic.jpg") != nil,
              conciseProcessLog("ERROR: could not open debug image") != nil,
              conciseProcessLog("ERR: Permission denied") == "ERR: Permission denied",
              conciseProcessLog("Progress 50% ") == "Progress 50% ",
              conciseProcessLog("50% ") == "50% ",
              conciseProcessLog("处理失败：权限不足") != nil
        else {
            fputs("native-host self-test concise logs dropped a failure or progress\n", stderr)
            return 1
        }
        let restore = ProcessorRequest(
            targetPath: "/tmp/archive", processingMode: .videosOnly, operationMode: .restoreJpeg,
            ultimate: false, verbose: true,
        )
        let restoreExpected = [
            "--images-only", "--mode", "restore-jpeg", "--verbose", "/tmp/archive",
        ]
        guard try ProcessorCommand.arguments(from: restore) == restoreExpected else {
            fputs("native-host self-test restore capability mapping failed\n", stderr)
            return 1
        }
        let collect = ProcessorRequest(
            targetPath: "/tmp/audited",
            processingMode: .both,
            operationMode: .collect,
            backupPath: "/tmp/backup",
            ultimate: false,
            verbose: true
        )
        guard try ProcessorCommand.arguments(from: collect) == [
            "--mode", "collect", "--verbose", "--backup", "/tmp/backup", "/tmp/audited",
        ] else {
            fputs("native-host self-test recovery backup mapping failed\n", stderr)
            return 1
        }
        let compare = ProcessorRequest(
            targetPath: "/tmp/current.photoslibrary",
            processingMode: .both,
            operationMode: .compare,
            backupPath: "/tmp/backup.photoslibrary",
            ultimate: false,
            verbose: true
        )
        guard try ProcessorCommand.arguments(from: compare) == [
            "--mode", "compare", "--verbose", "--backup", "/tmp/backup.photoslibrary", "/tmp/current.photoslibrary",
        ] else {
            fputs("native-host self-test backup comparison mapping failed\n", stderr)
            return 1
        }
        let album = PhotosAuditContainer(
            kind: .album,
            id: "11111111-1111-1111-1111-111111111111",
            name: "Archive",
            parentID: nil,
            path: ["Archive"]
        )
        let scopedRestore = ProcessorRequest(
            targetPath: "/tmp/debug.photoslibrary",
            processingMode: .imagesOnly,
            operationMode: .restoreJpeg,
            ultimate: false,
            verbose: true,
            photosContainer: album
        )
        guard try ProcessorCommand.arguments(from: scopedRestore) == [
            "--images-only", "--mode", "restore-jpeg", "--verbose",
            "--photos-album-id", album.id, "/tmp/debug.photoslibrary",
        ] else {
            fputs("native-host self-test Photos album scope mapping failed\n", stderr)
            return 1
        }
        do {
            _ = try ProcessorCommand.arguments(from: ProcessorRequest(
                targetPath: "/tmp/video", processingMode: .imagesOnly,
                operationMode: .fastVid, ultimate: true, verbose: false,
            ))
            fputs("native-host self-test unsupported FastVid ultimate flag was accepted\n", stderr)
            return 1
        } catch {}
        guard processingRequiresPhotosAutomation(request),
              processingRequiresPhotosAutomation(ProcessorRequest(targetPath: "/tmp/media", processingMode: .both, operationMode: .iCloudImport)),
              processingRequiresPhotosAutomation(ProcessorRequest(targetPath: "/tmp/media", processingMode: .videosOnly, operationMode: .fastVid, shortestPath: true)),
               !processingRequiresPhotosAutomation(restore),
               !processingRequiresPhotosAutomation(ProcessorRequest(targetPath: "/tmp/media", processingMode: .both, operationMode: .iCloudImport, dryRun: true)),
              processingRequiresPhotosAutomation(ProcessorRequest(targetPath: "/tmp/debug.photoslibrary", processingMode: .imagesOnly, operationMode: .restoreJpeg, ultimate: false)),
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
        guard shell == "cd '/tmp' && '/tmp/processor' '--images-only' '--mode' 'fast-img' '--strategy' 'jxl' '/tmp/media'\"'\"'$(id)'" else {
            fputs("native-host self-test shell quoting failed: \(shell)\n", stderr)
            return 1
        }
        var oversized = Data(repeating: 0x61, count: maxProcessLogChunkBytes + 17)
        let chunks = drainProcessLogChunks(&oversized, flush: false)
        guard chunks.count == 1, chunks[0].utf8.count == maxProcessLogChunkBytes, oversized.count == 17 else {
            fputs("native-host self-test log chunk bound failed\n", stderr)
            return 1
        }
        let boundedPipe = Pipe()
        boundedPipe.fileHandleForWriting.write(Data(repeating: 0x61, count: 128))
        try boundedPipe.fileHandleForWriting.close()
        let boundedCapture = try readBoundedProcessOutput(
            boundedPipe.fileHandleForReading,
            limit: 32
        )
        guard boundedCapture.exceeded, boundedCapture.data.count == 32 else {
            fputs("native-host self-test process output bound failed\n", stderr)
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
        try MainActor.assumeIsolated {
            _ = NSApplication.shared
            try AppController().validateInterfaceForSelfTest()
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
