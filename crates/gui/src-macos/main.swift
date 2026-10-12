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
private let developerPreferenceKey = "MFBGuiDeveloperMode"
private let historyPreferenceKey = "MFBGuiHistoryDirectory"
private let mainWindowContentSize = NSSize(width: 980, height: 720)
private let mainWindowStyleMask: NSWindow.StyleMask = [
    .titled, .closable, .miniaturizable, .fullSizeContentView,
]

private var appVersion: String {
    Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development"
}

private var historyDirectory: URL {
    FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".modern_format_boost/logs", isDirectory: true)
}

private func initialHistoryDirectory(
    preferences: UserDefaults,
    environment: [String: String] = ProcessInfo.processInfo.environment
) -> URL {
    if let path = environment["MFB_LOG_DIR"], path.hasPrefix("/") {
        return URL(fileURLWithPath: path, isDirectory: true)
    }
    if let path = environment["MFB_HOME_ROOT"], path.hasPrefix("/") {
        return URL(fileURLWithPath: path, isDirectory: true).appendingPathComponent("logs", isDirectory: true)
    }
    if let path = preferences.string(forKey: historyPreferenceKey), path.hasPrefix("/") {
        return URL(fileURLWithPath: path, isDirectory: true)
    }
    return historyDirectory
}

private func bundledLicenseText() throws -> String {
    guard let project = Bundle.main.url(forResource: "LICENSE", withExtension: nil),
          let thirdParty = Bundle.main.url(forResource: "LICENSES", withExtension: "json") else {
        throw HostError(message: localized("error.licenses_missing"))
    }
    let data = try Data(contentsOf: thirdParty)
    guard let document = try JSONSerialization.jsonObject(with: data) as? [String: Any],
          let licenses = document["licenses"] as? [[String: Any]], !licenses.isEmpty else {
        throw HostError(message: localized("error.licenses_missing"))
    }
    var sections = ["Modern Format Boost · \(appVersion)", try String(contentsOf: project, encoding: .utf8)]
    for license in licenses {
        guard let name = license["name"] as? String, let text = license["text"] as? String,
              let users = license["used_by"] as? [[String: Any]] else {
            throw HostError(message: localized("error.licenses_missing"))
        }
        let crates = users.compactMap { ($0["crate"] as? [String: Any])?["name"] as? String }.sorted()
        sections.append("\(name)\n\(crates.joined(separator: ", "))\n\n\(text)")
    }
    return sections.joined(separator: "\n\n────────────────────────\n\n")
}

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

func localized(_ key: String, _ arguments: CVarArg...) -> String {
    let format = LocalizationCatalog.shared.text(key)
    guard !arguments.isEmpty else { return format }
    return String(format: format, locale: Locale.current, arguments: arguments)
}

struct HostError: LocalizedError {
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

    var developerOnly: Bool {
        [.collect, .compare, .iCloudImport, .diagnostic, .cacheClean, .databaseManager].contains(self)
    }

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
    var mediaSettings = MediaSettings()
}

private enum MediaSetting: String, CaseIterable {
    case imgConfig, imgFallback, imgJpegEffort, imgHeuristic, imgDatabase, imgSizeTolerance, imgSizeToleranceBytes, imgErrorMode, imgToolPolicy
    case vidCodec, vidErrorMode, vidConfig
    case fastConfig, fastFallback, fastJpegEffort, fastHeuristic, fastDatabase, fastToolPolicy
    case photosBackend, photosNativeBatch, photosAppleScriptBatch, photosVerificationBatch
    case photosAdaptive, photosMinimumBatch, photosMaximumBatch, photosTargetSeconds
    case photosRoot, photosAlbum, photosPreserveTree
    case performance
    case cacheMaxBytes, cacheTtlSeconds

    var isImage: Bool { rawValue.hasPrefix("img") }
    var isVideo: Bool { rawValue.hasPrefix("vid") }
    var isFastImage: Bool { rawValue.hasPrefix("fast") }
    var isPhotos: Bool { rawValue.hasPrefix("photos") }
    var isFailurePolicy: Bool { self == .imgErrorMode || self == .vidErrorMode }
    var isCache: Bool { self == .cacheMaxBytes || self == .cacheTtlSeconds }
    var isShared: Bool { self == .performance || isCache }
    var isDeveloper: Bool { [.imgConfig, .fastConfig, .vidConfig, .imgErrorMode, .vidErrorMode, .imgToolPolicy, .fastToolPolicy].contains(self) }
    var section: String {
        if self == .performance { return "performance" }
        if isCache { return "cache" }
        if isDeveloper { return "developer" }
        if isPhotos { return "photos" }
        if isFastImage { return "fast" }
        return isImage ? "img" : "vid"
    }
    var flag: String {
        switch self {
        case .performance: "--performance"
        case .cacheMaxBytes: "--cache-max-bytes"
        case .cacheTtlSeconds: "--cache-ttl-seconds"
        case .imgConfig, .fastConfig: "--img-config"
        case .imgFallback, .fastFallback: "--img-fallback-policy"
        case .imgJpegEffort, .fastJpegEffort: "--img-jpeg-effort"
        case .imgHeuristic, .fastHeuristic: "--img-quality-heuristic"
        case .imgDatabase, .fastDatabase: "--img-allow-database"
        case .imgSizeTolerance: "--img-allow-size-tolerance"
        case .imgSizeToleranceBytes: "--img-size-tolerance-bytes"
        case .imgToolPolicy, .fastToolPolicy: "--img-tool-policy"
        case .imgErrorMode: "--img-error-mode"
        case .vidCodec: "--vid-codec"
        case .vidErrorMode: "--vid-error-mode"
        case .vidConfig: "--vid-config"
        case .photosBackend: "--photos-backend"
        case .photosNativeBatch: "--photos-native-batch-size"
        case .photosAppleScriptBatch: "--photos-import-batch-size"
        case .photosVerificationBatch: "--photos-verification-batch-size"
        case .photosAdaptive: "--photos-adaptive-batching"
        case .photosMinimumBatch: "--photos-native-min-batch-size"
        case .photosMaximumBatch: "--photos-native-max-batch-size"
        case .photosTargetSeconds: "--photos-target-batch-seconds"
        case .photosRoot: "--photos-import-root"
        case .photosAlbum: "--photos-album-name"
        case .photosPreserveTree: "--preserve-folder-structure"
        }
    }
    var choices: [String] {
        switch self {
        case .performance: ["adaptive", "relaxed", "balanced", "tight"]
        case .imgFallback, .fastFallback: ["strict", "same-semantics", "repair"]
        case .imgHeuristic, .imgDatabase, .imgSizeTolerance, .fastHeuristic, .fastDatabase,
             .photosAdaptive, .photosPreserveTree: ["true", "false"]
        case .imgErrorMode, .vidErrorMode: ["log-and-continue", "fail-fast"]
        case .vidCodec: ["hevc", "av1"]
        case .imgToolPolicy, .fastToolPolicy: ["fallback", "single"]
        case .photosBackend: ["auto", "native", "applescript"]
        default: []
        }
    }
    var range: ClosedRange<Int>? {
        switch self {
        case .imgJpegEffort, .fastJpegEffort: 1...11
        case .photosNativeBatch, .photosVerificationBatch, .photosMinimumBatch, .photosMaximumBatch: 1...1000
        case .photosAppleScriptBatch: 1...50
        case .photosTargetSeconds: 1...600
        case .cacheMaxBytes, .cacheTtlSeconds: 1...Int.max
        case .imgSizeToleranceBytes: 0...Int.max
        default: nil
        }
    }
    var runtimeKey: String? {
        switch self {
        case .performance: "performance.mode"
        case .cacheMaxBytes: "cache.path_tree_max_bytes"
        case .cacheTtlSeconds: "cache.path_tree_ttl_seconds"
        case .imgFallback, .fastFallback: "img.fallback_policy"
        case .imgJpegEffort, .fastJpegEffort: "img.jpeg_effort"
        case .imgHeuristic, .fastHeuristic: "img.quality_heuristic"
        case .imgDatabase, .fastDatabase: "img.allow_database"
        case .imgSizeTolerance: "img.allow_size_tolerance"
        case .imgSizeToleranceBytes: "img.size_tolerance_bytes"
        case .imgToolPolicy, .fastToolPolicy: "tools.policy"
        case .imgErrorMode, .vidErrorMode: "execution.error_mode"
        case .vidCodec: "vid.codec"
        case .photosBackend: "photos.backend"
        case .photosNativeBatch: "photos.native_batch_size"
        case .photosAppleScriptBatch: "photos.import_batch_size"
        case .photosVerificationBatch: "photos.verification_batch_size"
        case .photosAdaptive: "photos.adaptive_batching"
        case .photosMinimumBatch: "photos.native_min_batch_size"
        case .photosMaximumBatch: "photos.native_max_batch_size"
        case .photosTargetSeconds: "photos.target_batch_seconds"
        case .photosRoot: "photos.import_root"
        case .photosAlbum: "photos.album_name"
        case .photosPreserveTree: "photos.preserve_folder_structure"
        default: nil
        }
    }
    var preferenceKey: String { "MFBGuiMediaSettings.\(rawValue)" }
    var labelKey: String {
        switch self {
        case .fastConfig: "fastConfig"
        case .fastFallback: "imgFallback"
        case .fastJpegEffort: "imgJpegEffort"
        case .fastHeuristic: "imgHeuristic"
        case .fastDatabase: "imgDatabase"
        case .fastToolPolicy: "imgToolPolicy"
        default: rawValue
        }
    }
    var title: String { localized("settings.\(labelKey)") }

    func arguments(_ value: String) -> [String] {
        choices == ["true", "false"] ? ["\(flag)=\(value)"] : [flag, value]
    }
}

private enum ImageFallback: String { case strict, sameSemantics = "same-semantics", repair }
private enum FileFailurePolicy: String { case recordAndContinue = "log-and-continue", failFast = "fail-fast" }
private enum VideoCodec: String { case hevc, av1 }
private enum ToolSelectionPolicy: String { case fallback, single }
private enum PhotosImportBackend: String { case auto, native, applescript }

private struct ImageSettings {
    var configurationFile: String?
    var fallback: ImageFallback?
    var jpegEffort: Int?
    var qualityHeuristic: Bool?
    var allowDatabase: Bool?
    var toolPolicy: ToolSelectionPolicy?
}

private struct PhotosImportSettings {
    var backend: PhotosImportBackend?
    var nativeBatchSize: Int?
    var appleScriptBatchSize: Int?
    var verificationBatchSize: Int?
    var adaptive: Bool?
    var minimumBatchSize: Int?
    var maximumBatchSize: Int?
    var targetSeconds: Int?
    var rootFolder: String?
    var album: String?
    var preserveTree: Bool?
}

private struct MediaSettings {
    var image = ImageSettings()
    var fastImage = ImageSettings()
    var photos = PhotosImportSettings()
    var imageFailure: FileFailurePolicy?
    var imageSizeTolerance: Bool?
    var imageSizeToleranceBytes: Int?
    var videoFailure: FileFailurePolicy?
    var videoCodec: VideoCodec?
    var videoConfigurationFile: String?
    var performance: String?
    var cacheMaxBytes: Int?
    var cacheTtlSeconds: Int?
    private var invalidValues: [MediaSetting: String] = [:]

    // String values are the control/persistence boundary; processing uses typed groups.
    var values: [MediaSetting: String] {
        get {
            let stored: [MediaSetting: String?] = [
                .performance: performance,
                .cacheMaxBytes: cacheMaxBytes.map(String.init), .cacheTtlSeconds: cacheTtlSeconds.map(String.init),
                .imgConfig: image.configurationFile, .imgFallback: image.fallback?.rawValue,
                .imgJpegEffort: image.jpegEffort.map(String.init), .imgHeuristic: image.qualityHeuristic.map(String.init),
                .imgDatabase: image.allowDatabase.map(String.init), .imgErrorMode: imageFailure?.rawValue,
                .imgSizeTolerance: imageSizeTolerance.map(String.init),
                .imgSizeToleranceBytes: imageSizeToleranceBytes.map(String.init),
                .imgToolPolicy: image.toolPolicy?.rawValue,
                .fastConfig: fastImage.configurationFile, .fastFallback: fastImage.fallback?.rawValue,
                .fastJpegEffort: fastImage.jpegEffort.map(String.init), .fastHeuristic: fastImage.qualityHeuristic.map(String.init),
                .fastDatabase: fastImage.allowDatabase.map(String.init), .fastToolPolicy: fastImage.toolPolicy?.rawValue,
                .vidCodec: videoCodec?.rawValue, .vidConfig: videoConfigurationFile,
                .vidErrorMode: videoFailure?.rawValue, .photosBackend: photos.backend?.rawValue,
                .photosNativeBatch: photos.nativeBatchSize.map(String.init),
                .photosAppleScriptBatch: photos.appleScriptBatchSize.map(String.init),
                .photosVerificationBatch: photos.verificationBatchSize.map(String.init),
                .photosAdaptive: photos.adaptive.map(String.init), .photosMinimumBatch: photos.minimumBatchSize.map(String.init),
                .photosMaximumBatch: photos.maximumBatchSize.map(String.init), .photosTargetSeconds: photos.targetSeconds.map(String.init),
                .photosRoot: photos.rootFolder, .photosAlbum: photos.album, .photosPreserveTree: photos.preserveTree.map(String.init),
            ]
            var result = stored.compactMapValues { $0 }
            result.merge(invalidValues) { _, invalid in invalid }
            return result
        }
        set {
            self = MediaSettings()
            for (field, value) in newValue {
                if let range = field.range {
                    guard let number = Int(value), range.contains(number) else { invalidValues[field] = value; continue }
                } else if !field.choices.isEmpty, !field.choices.contains(value) {
                    invalidValues[field] = value
                    continue
                }
                switch field {
                case .performance: performance = value
                case .cacheMaxBytes: cacheMaxBytes = Int(value)
                case .cacheTtlSeconds: cacheTtlSeconds = Int(value)
                case .imgConfig: image.configurationFile = value
                case .imgFallback: image.fallback = ImageFallback(rawValue: value)
                case .imgJpegEffort: image.jpegEffort = Int(value)
                case .imgHeuristic: image.qualityHeuristic = Bool(value)
                case .imgDatabase: image.allowDatabase = Bool(value)
                case .imgSizeTolerance: imageSizeTolerance = Bool(value)
                case .imgSizeToleranceBytes: imageSizeToleranceBytes = Int(value)
                case .imgToolPolicy: image.toolPolicy = ToolSelectionPolicy(rawValue: value)
                case .imgErrorMode: imageFailure = FileFailurePolicy(rawValue: value)
                case .fastConfig: fastImage.configurationFile = value
                case .fastFallback: fastImage.fallback = ImageFallback(rawValue: value)
                case .fastJpegEffort: fastImage.jpegEffort = Int(value)
                case .fastHeuristic: fastImage.qualityHeuristic = Bool(value)
                case .fastDatabase: fastImage.allowDatabase = Bool(value)
                case .fastToolPolicy: fastImage.toolPolicy = ToolSelectionPolicy(rawValue: value)
                case .vidCodec: videoCodec = VideoCodec(rawValue: value)
                case .vidConfig: videoConfigurationFile = value
                case .vidErrorMode: videoFailure = FileFailurePolicy(rawValue: value)
                case .photosBackend: photos.backend = PhotosImportBackend(rawValue: value)
                case .photosNativeBatch: photos.nativeBatchSize = Int(value)
                case .photosAppleScriptBatch: photos.appleScriptBatchSize = Int(value)
                case .photosVerificationBatch: photos.verificationBatchSize = Int(value)
                case .photosAdaptive: photos.adaptive = Bool(value)
                case .photosMinimumBatch: photos.minimumBatchSize = Int(value)
                case .photosMaximumBatch: photos.maximumBatchSize = Int(value)
                case .photosTargetSeconds: photos.targetSeconds = Int(value)
                case .photosRoot: photos.rootFolder = value
                case .photosAlbum: photos.album = value
                case .photosPreserveTree: photos.preserveTree = Bool(value)
                }
            }
        }
    }

    init(preferences: UserDefaults? = nil) {
        if let preferences {
            var saved: [MediaSetting: String] = [:]
            for field in MediaSetting.allCases {
                if let value = preferences.string(forKey: field.preferenceKey) {
                    saved[field] = value
                }
            }
            values = saved
        }
    }

    func validate(_ fields: [MediaSetting] = MediaSetting.allCases) throws {
        for field in fields {
            guard let value = values[field] else { continue }
            let valid: Bool
            switch field {
            case .imgConfig, .fastConfig, .vidConfig:
                var isDirectory: ObjCBool = false
                valid = value.hasPrefix("/")
                    && FileManager.default.fileExists(atPath: value, isDirectory: &isDirectory)
                    && !isDirectory.boolValue && FileManager.default.isReadableFile(atPath: value)
            case .photosRoot, .photosAlbum:
                valid = !value.isEmpty && value != "." && value != ".."
                    && !value.contains("/") && !value.contains("\\")
                    && value.rangeOfCharacter(from: .controlCharacters) == nil
            default:
                valid = field.range.map { range in Int(value).map(range.contains) ?? false }
                    ?? field.choices.contains(value)
            }
            guard valid else { throw HostError(message: localized("settings.invalid", field.title, value)) }
        }
        if fields.contains(.photosMinimumBatch), let minimum = photos.minimumBatchSize,
           let maximum = photos.maximumBatchSize, minimum > maximum {
            throw HostError(message: localized("settings.bounds_invalid"))
        }
    }

    func save(to preferences: UserDefaults) throws {
        try validate()
        for field in MediaSetting.allCases {
            if let value = values[field] { preferences.set(value, forKey: field.preferenceKey) }
            else { preferences.removeObject(forKey: field.preferenceKey) }
        }
    }

    func arguments(operation: OperationMode, processing: ProcessingMode) throws -> [String] {
        let fastImages = operation.backendMode == "fast-img"
        let images = operation == .adjacent && processing != .videosOnly
        let videos = operation == .adjacent && processing != .imagesOnly
        let fields = MediaSetting.allCases.filter {
            if $0.isShared { return images || videos || fastImages || ($0.isCache && operation == .fastVid) }
            return $0.isImage ? images : ($0.isFastImage || $0.isPhotos ? fastImages : videos)
        }
        try validate(fields)
        return fields.flatMap { field -> [String] in
            guard let value = values[field] else { return [] }
            return field.arguments(value)
        }
    }

    func runtimeArguments(fast: Bool, inheritedOnly: Bool) -> [String] {
        MediaSetting.allCases.filter {
            ($0.isShared || (fast ? ($0.isFastImage || $0.isPhotos) : $0.isImage))
                && (!inheritedOnly || $0 == (fast ? .fastConfig : .imgConfig))
        }.flatMap { field -> [String] in
            guard let value = values[field] else { return [] }
            return field.arguments(value).map { argument in
                argument.hasPrefix("--img-") ? "--" + argument.dropFirst(6) : argument
            }
        }
    }

    func videoRuntimeArguments(inheritedOnly: Bool) -> [String] {
        var arguments = videoConfigurationFile.map { ["--config", $0] } ?? []
        if !inheritedOnly {
            if let performance { arguments += ["--performance", performance] }
            if let cacheMaxBytes { arguments += ["--cache-max-bytes", String(cacheMaxBytes)] }
            if let cacheTtlSeconds { arguments += ["--cache-ttl-seconds", String(cacheTtlSeconds)] }
            if let videoCodec { arguments += ["--codec", videoCodec.rawValue] }
            if let videoFailure { arguments += ["--error-mode", videoFailure.rawValue] }
        }
        return arguments
    }
}

private struct EffectiveRuntimeSettings {
    let values: [String: String]
    let sources: [String: String]
    let sourceChain: [String: [String]]
    subscript(key: String) -> String? { values[key] }

    static func decode(_ data: Data) throws -> EffectiveRuntimeSettings {
        guard let document = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let config = document["config"] as? [String: Any],
              let sources = document["sources"] as? [String: String],
              let chains = document["source_chain"] as? [String: [String]],
              !sources.isEmpty, Set(sources.keys) == Set(chains.keys),
              sources.allSatisfy({ key, source in
                  !source.isEmpty && chains[key]?.last == source
                      && chains[key]?.allSatisfy({ !$0.isEmpty }) == true
              }) else {
            throw HostError(message: localized("settings.config_invalid"))
        }
        var result: [String: String] = [:]
        for (section, object) in config {
            guard let fields = object as? [String: Any] else { continue }
            for (key, value) in fields {
                if value is NSNull { continue }
                if let number = value as? NSNumber {
                    result["\(section).\(key)"] = CFGetTypeID(number) == CFBooleanGetTypeID()
                        ? (number.boolValue ? "true" : "false") : number.stringValue
                } else if let text = value as? String { result["\(section).\(key)"] = text }
            }
        }
        guard result.keys.allSatisfy({ sources[$0] != nil }) else {
            throw HostError(message: localized("settings.config_invalid"))
        }
        return EffectiveRuntimeSettings(values: result, sources: sources, sourceChain: chains)
    }

    func originDescription(key: String, guiOverride: Bool) throws -> String {
        guard let chain = sourceChain[key], !chain.isEmpty, chain.last == sources[key],
              !guiOverride || chain.last == "CLI" else {
            throw HostError(message: localized("settings.config_invalid"))
        }
        let layers = chain.enumerated().map { index, source in
            let label = guiOverride && index == chain.count - 1 ? "CLI (GUI)" : source
            return "\(index + 1). \(label)"
        }.joined(separator: "\n")
        return "\(key) = \(values[key] ?? "null")\n\n\(localized("settings.sources.order"))\n\(layers)"
    }
}

private func settingsToolOutput(_ binary: URL, arguments: [String], timeout: TimeInterval = 10, acceptedExitCodes: Set<Int32> = [0]) throws -> Data {
    let process = Process()
    process.executableURL = binary
    process.arguments = arguments
    let output = Pipe()
    process.standardOutput = output
    process.standardError = output
    try process.run()
    let watchdog = DispatchWorkItem { if process.isRunning { kill(process.processIdentifier, SIGKILL) } }
    DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + timeout, execute: watchdog)
    defer { watchdog.cancel() }
    let capture = try readBoundedProcessOutput(output.fileHandleForReading, limit: 1024 * 1024)
    process.waitUntilExit()
    guard !capture.exceeded, process.terminationReason == .exit, acceptedExitCodes.contains(process.terminationStatus) else {
        let detail = String(decoding: capture.data.prefix(8192), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        throw HostError(message: detail.isEmpty ? localized("settings.backend_failed", process.terminationStatus) : detail)
    }
    return capture.data
}

private func queryRuntimeSettings(arguments: [String], tool: String = "img") throws -> EffectiveRuntimeSettings {
    guard let binary = ProcessorLocator.resolveTool(named: tool) else {
        throw HostError(message: localized(tool == "vid" ? "error.vid_backend_missing" : "error.img_backend_missing"))
    }
    let data = try settingsToolOutput(binary, arguments: ["config", "show", "--effective"] + arguments)
    return try EffectiveRuntimeSettings.decode(data)
}

private struct LocalCacheStatus: Decodable {
    struct Database: Decodable {
        let storeSchemaVersion: Int
        let sqliteVersion: String
        let journalMode: String
        let pageSize: UInt64
        let pageCount: UInt64
        let freePages: UInt64

        func validate() throws {
            let bytes = pageSize.multipliedReportingOverflow(by: pageCount)
            guard storeSchemaVersion > 0, !sqliteVersion.isEmpty,
                  ["delete", "truncate", "persist", "memory", "wal", "off"].contains(journalMode),
                  pageSize >= 512, pageSize <= 65536, pageSize.nonzeroBitCount == 1,
                  pageCount > 0, freePages <= pageCount, !bytes.overflow, bytes.partialValue <= UInt64(Int64.max) else {
                throw HostError(message: localized("settings.cache.invalid"))
            }
        }
    }
    struct Pruning: Decodable {
        struct Policy: Decodable {
            let pathTreeMaxBytes: UInt64
            let pathTreeTtlSeconds: UInt64
        }
        let policy: Policy
        let expiredRows: UInt64
        let evictedRows: UInt64
    }
    struct Integrity: Decodable {
        struct Issue: Decodable { let code: String; let message: String }
        enum Status: String, Decodable { case healthy, unhealthy, error, absent }
        enum Check: String, Decodable { case ok, failed, notChecked = "not_checked" }
        let schemaVersion: Int
        let status: Status
        let sqliteIntegrity: Check
        let foreignKeys: Check
        let blobScanComplete: Bool
        let checkedCacheRows: UInt64
        let checkedProtectedRows: UInt64
        let corruptCacheRows: UInt64
        let corruptProtectedRows: UInt64
        let issueCount: UInt64
        let issues: [Issue]

        func validate() throws {
            guard schemaVersion == 1, issues.count <= 20, UInt64(issues.count) <= issueCount,
                  corruptCacheRows <= checkedCacheRows, corruptProtectedRows <= checkedProtectedRows,
                  issues.allSatisfy({ !$0.code.isEmpty && !$0.message.isEmpty }) else {
                throw HostError(message: localized("settings.cache.invalid"))
            }
            let complete = sqliteIntegrity == .ok && foreignKeys == .ok && blobScanComplete
            switch status {
            case .healthy:
                guard complete, issueCount == 0, corruptCacheRows == 0, corruptProtectedRows == 0 else {
                    throw HostError(message: localized("settings.cache.invalid"))
                }
            case .absent:
                guard sqliteIntegrity == .notChecked, foreignKeys == .notChecked, !blobScanComplete,
                      checkedCacheRows == 0, checkedProtectedRows == 0, issueCount == 0 else {
                    throw HostError(message: localized("settings.cache.invalid"))
                }
            case .unhealthy, .error:
                guard issueCount > 0, !issues.isEmpty else {
                    throw HostError(message: localized("settings.cache.invalid"))
                }
            }
        }
    }
    struct Namespace: Decodable {
        let name: String
        let rows: UInt64
        let payloadBytes: UInt64
        let rebuildable: Bool
    }
    let schemaVersion: Int
    let cacheDirectory: String
    let storeBytes: UInt64
    let namespaces: [Namespace]
    let legacyAnalysisBytes: UInt64
    let legacyAnalysisFiles: UInt64
    let removedRows: UInt64
    let removedFiles: UInt64
    let integrity: Integrity?
    let pruning: Pruning?
    let database: Database?

    func totals(rebuildable: Bool) throws -> (rows: UInt64, bytes: UInt64) {
        var rows: UInt64 = 0, bytes: UInt64 = 0
        for namespace in namespaces where namespace.rebuildable == rebuildable {
            let count = rows.addingReportingOverflow(namespace.rows)
            let size = bytes.addingReportingOverflow(namespace.payloadBytes)
            guard !count.overflow, !size.overflow, size.partialValue <= UInt64(Int64.max) else {
                throw HostError(message: localized("settings.cache.invalid"))
            }
            rows = count.partialValue
            bytes = size.partialValue
        }
        return (rows, bytes)
    }

    static func decode(_ data: Data) throws -> LocalCacheStatus {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let status = try decoder.decode(Self.self, from: data)
        guard status.schemaVersion == 1, status.cacheDirectory.hasPrefix("/"),
              status.storeBytes <= UInt64(Int64.max), status.legacyAnalysisBytes <= UInt64(Int64.max),
              Set(status.namespaces.map(\.name)).count == status.namespaces.count,
              status.namespaces.allSatisfy({ !$0.name.isEmpty && $0.rebuildable == ($0.name == "path_tree") }) else {
            throw HostError(message: localized("settings.cache.invalid"))
        }
        _ = try status.totals(rebuildable: true)
        _ = try status.totals(rebuildable: false)
        if let pruning = status.pruning {
            let removed = pruning.expiredRows.addingReportingOverflow(pruning.evictedRows)
            guard pruning.policy.pathTreeMaxBytes > 0, pruning.policy.pathTreeMaxBytes <= UInt64(Int64.max),
                  pruning.policy.pathTreeTtlSeconds > 0, pruning.policy.pathTreeTtlSeconds <= UInt64(Int64.max),
                  !removed.overflow, removed.partialValue == status.removedRows, status.removedFiles == 0 else {
                throw HostError(message: localized("settings.cache.invalid"))
            }
        }
        try status.integrity?.validate()
        try status.database?.validate()
        if let integrity = status.integrity, integrity.status == .healthy || integrity.status == .unhealthy {
            guard integrity.blobScanComplete,
                  integrity.checkedCacheRows == (try status.totals(rebuildable: true).rows),
                  integrity.checkedProtectedRows == (try status.totals(rebuildable: false).rows) else {
                throw HostError(message: localized("settings.cache.invalid"))
            }
        }
        return status
    }
}

private struct CoreDatabaseBackup: Decodable {
    let schemaVersion: Int
    let source: String
    let destination: String
    let storeSchemaVersion: Int
    let bytes: UInt64

    static func decode(_ data: Data, source: URL, destination: URL, storeSchemaVersion: Int) throws -> Self {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let receipt = try decoder.decode(Self.self, from: data)
        guard receipt.schemaVersion == 1, receipt.storeSchemaVersion == storeSchemaVersion,
              receipt.source.hasPrefix("/"), receipt.destination.hasPrefix("/"), receipt.bytes > 0,
              receipt.bytes <= UInt64(Int64.max),
              URL(fileURLWithPath: receipt.source).standardizedFileURL == source.resolvingSymlinksInPath().standardizedFileURL,
              URL(fileURLWithPath: receipt.destination).standardizedFileURL == destination.resolvingSymlinksInPath().standardizedFileURL else {
            throw HostError(message: localized("settings.cache.backup_invalid"))
        }
        let values = try destination.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
        guard values.isRegularFile == true, values.isSymbolicLink != true,
              values.fileSize.flatMap(UInt64.init(exactly:)) == receipt.bytes else {
            throw HostError(message: localized("settings.cache.backup_invalid"))
        }
        return receipt
    }
}

private func backupCoreDatabase(to destination: URL, source: URL, schema: Int) throws -> CoreDatabaseBackup {
    guard let binary = ProcessorLocator.resolveTool(named: "cache_cleaner") else {
        throw HostError(message: localized("settings.cache.backend_missing"))
    }
    let data = try settingsToolOutput(binary, arguments: ["--local-backup", destination.path, "--json"], timeout: 120)
    return try CoreDatabaseBackup.decode(data, source: source, destination: destination, storeSchemaVersion: schema)
}

private struct HistoryDatabaseBackup: Decodable {
    let schemaVersion: Int
    let kind: String
    let source: String
    let destination: String
    let historySchemaVersion: Int
    let bytes: UInt64
    let sessions: UInt64
    let events: UInt64
    let outputRows: UInt64

    static func decode(_ data: Data, source: URL, destination: URL) throws -> Self {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let receipt = try decoder.decode(Self.self, from: data)
        guard receipt.schemaVersion == 1, receipt.kind == "history_sqlite", [1, 2].contains(receipt.historySchemaVersion),
              receipt.historySchemaVersion != 1 || receipt.outputRows == 0,
              receipt.source.hasPrefix("/"), receipt.destination.hasPrefix("/"), receipt.bytes > 0,
              [receipt.bytes, receipt.sessions, receipt.events, receipt.outputRows].allSatisfy({ $0 <= UInt64(Int64.max) }),
              URL(fileURLWithPath: receipt.source).standardizedFileURL == source.resolvingSymlinksInPath().standardizedFileURL,
              URL(fileURLWithPath: receipt.destination).standardizedFileURL == destination.resolvingSymlinksInPath().standardizedFileURL else {
            throw HostError(message: localized("settings.history.backup_invalid"))
        }
        let values = try destination.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
        guard values.isRegularFile == true, values.isSymbolicLink != true,
              values.fileSize.flatMap(UInt64.init(exactly:)) == receipt.bytes else {
            throw HostError(message: localized("settings.history.backup_invalid"))
        }
        return receipt
    }
}

private func historyBackupArguments(directory: URL, destination: URL) -> [String] {
    ["--history-backup", destination.path, "--history-directory", directory.path, "--json"]
}

private func backupHistoryDatabase(to destination: URL, directory: URL) throws -> HistoryDatabaseBackup {
    guard let binary = ProcessorLocator.resolveTool(named: "cache_cleaner") else {
        throw HostError(message: localized("settings.cache.backend_missing"))
    }
    let data = try settingsToolOutput(binary, arguments: historyBackupArguments(directory: directory, destination: destination), timeout: 120)
    return try HistoryDatabaseBackup.decode(data, source: directory.appendingPathComponent("history.sqlite3"), destination: destination)
}

private enum LocalCacheAction: Equatable { case refresh, clear, check, prune }

private func cachePruningArguments(_ policies: [EffectiveRuntimeSettings]) throws -> [String] {
    let keys = ["cache.path_tree_max_bytes", "cache.path_tree_ttl_seconds"]
    guard !policies.isEmpty else { throw HostError(message: localized("settings.config_invalid")) }
    var arguments = ["--no-config"]
    for (key, flag) in zip(keys, ["--cache-max-bytes", "--cache-ttl-seconds"]) {
        let values = try policies.map { policy -> UInt64 in
            guard let raw = policy[key], let value = UInt64(raw), value > 0, value <= UInt64(Int64.max) else {
                throw HostError(message: localized("settings.config_invalid"))
            }
            return value
        }
        guard Set(values).count == 1, let value = values.first else {
            throw HostError(message: localized("settings.cache.prune_mixed"))
        }
        arguments += [flag, String(value)]
    }
    return arguments
}

private func queryLocalCache(action: LocalCacheAction, policyArguments: [String] = []) throws -> LocalCacheStatus {
    guard let binary = ProcessorLocator.resolveTool(named: "cache_cleaner") else {
        throw HostError(message: localized("settings.cache.backend_missing"))
    }
    let arguments: [String]
    switch action {
    case .refresh: arguments = ["--stats", "--json"]
    case .clear: arguments = ["--yes", "--json"]
    case .check: arguments = ["--stats", "--check-integrity", "--json"]
    case .prune:
        guard policyArguments.count == 5 else { throw HostError(message: localized("settings.config_invalid")) }
        arguments = ["--prune", "--yes", "--json"] + policyArguments
    }
    let status = try LocalCacheStatus.decode(settingsToolOutput(binary, arguments: arguments,
        timeout: action == .check ? 120 : 30, acceptedExitCodes: action == .check ? [0, 1] : [0]))
    guard action != .check || status.integrity != nil else {
        throw HostError(message: localized("settings.cache.invalid"))
    }
    if action == .prune {
        guard let pruning = status.pruning,
              String(pruning.policy.pathTreeMaxBytes) == policyArguments[2],
              String(pruning.policy.pathTreeTtlSeconds) == policyArguments[4] else {
            throw HostError(message: localized("settings.cache.invalid"))
        }
    }
    return status
}

@MainActor
private final class MediaSettingsPanel: NSObject, NSTabViewDelegate, NSSearchFieldDelegate, NSTableViewDataSource, NSTableViewDelegate {
    private enum SearchDestination: Equatable {
        case setting(MediaSetting)
        case cache
    }

    private let panel: NSPanel
    private let preferences: UserDefaults
    private let applied: () -> Void
    private var popups: [MediaSetting: NSPopUpButton] = [:]
    private var textFields: [MediaSetting: NSTextField] = [:]
    private var toggles: [MediaSetting: NSButton] = [:]
    private var steppers: [MediaSetting: NSStepper] = [:]
    private var sourceButtons: [MediaSetting: NSButton] = [:]
    private var sourceGeneration = UUID()
    private let sourcePopover = NSPopover()
    private var restored = MediaSettings()
    private var inherited: [MediaSetting: String] = [:]
    private var inheritedSources: [MediaSetting: String] = [:]
    private var inheritedMixed: Set<MediaSetting> = []
    private var displayed: [MediaSetting: String] = [:]
    private var rows: [MediaSetting: NSGridRow] = [:]
    private let developer: Bool
    private let fast: Bool
    private let videos: Bool
    private let advanced = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let status = NSTextField(wrappingLabelWithString: "")
    private let tabs = NSTabView()
    private let root = NSStackView()
    private let search = NSSearchField()
    private let searchResults = NSScrollView()
    private let searchTable = NSTableView()
    private let searchEmpty = NSTextField(labelWithString: "")
    private var searchHeight: NSLayoutConstraint?
    private var searchMatches: [SearchDestination] = []
    private var fieldSections: [MediaSetting: String] = [:]
    private var tabHeight: NSLayoutConstraint?
    private var grids: [String: NSGridView] = [:]
    private var applying = false
    private var applyGeneration = UUID()
    private var inheritedGeneration = UUID()
    private let resetButton = NSButton()
    private let cancelButton = NSButton()
    private let applyButton = NSButton()
    private let cacheRefresh = NSButton()
    private let cacheClear = NSButton()
    private let cachePrune = NSButton()
    private let cacheCheck = NSButton()
    private let cacheBackup = NSButton()
    private let historyBackup = NSButton()
    private var historyBackupDirectory: URL {
        initialHistoryDirectory(preferences: preferences)
    }
    private var cacheStore: (url: URL, schema: Int)?
    private var cacheLabels: [String: NSTextField] = [:]
    private let cacheMessage = NSTextField(wrappingLabelWithString: "")
    private var cacheMessageRow: NSGridRow?
    private var cacheLoaded = false
    private var cacheBusy = false
    private var validatingLayout = false

    init(preferences: UserDefaults, developer: Bool = false, fast: Bool = false, videos: Bool = false, applied: @escaping () -> Void) {
        self.preferences = preferences
        self.applied = applied
        self.developer = developer
        self.fast = fast
        self.videos = videos
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 660, height: 300),
                        styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        panel.title = localized("settings.title")
        let surface = NSView()
        panel.contentView = surface
        root.orientation = .vertical
        root.alignment = .leading
        root.spacing = 12
        root.translatesAutoresizingMaskIntoConstraints = false
        surface.addSubview(root)
        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: surface.leadingAnchor, constant: 20),
            root.trailingAnchor.constraint(equalTo: surface.trailingAnchor, constant: -20),
            root.topAnchor.constraint(equalTo: surface.topAnchor, constant: 16),
            root.bottomAnchor.constraint(equalTo: surface.bottomAnchor, constant: -16),
        ])
        configureSearch()
        tabs.translatesAutoresizingMaskIntoConstraints = false
        tabs.delegate = self
        for section in ["img", "vid", "photos", "performance"] + (developer ? ["developer"] : []) + ["cache"] {
            let fields = MediaSetting.allCases.filter { field in
                if field == .performance { return section == "performance" }
                if field.isCache { return section == "cache" }
                if field.isDeveloper {
                    if field.isVideo { return developer && section == "developer" && !fast }
                    return developer && section == "developer" && !videos && (fast ? field.isFastImage : field.isImage)
                }
                if field.isPhotos { return section == "photos" }
                if field.isVideo { return section == "vid" && !fast }
                return section == "img" && !videos && (fast ? field.isFastImage : field.isImage)
            }
            if fields.isEmpty { continue }
            let tab = NSTabViewItem(identifier: section)
            tab.label = localized("settings.section.\(section)")
            let grid = NSGridView()
            grid.rowSpacing = 10
            grid.columnSpacing = 12
            for field in fields {
                let control: NSView
                if field == .photosAdaptive || field == .photosPreserveTree {
                    let toggle = NSButton(checkboxWithTitle: field.title, target: self, action: #selector(updateVisibility))
                    toggle.allowsMixedState = false
                    toggles[field] = toggle
                    control = toggle
                } else if !field.choices.isEmpty {
                    let popup = NSPopUpButton()
                    popup.target = self
                    popup.action = #selector(updateVisibility)
                    for value in field.choices {
                        popup.addItem(withTitle: localized("settings.value.\(value)"))
                        popup.lastItem?.representedObject = value
                    }
                    popups[field] = popup
                    control = popup
                } else {
                    let text = NSTextField()
                    text.delegate = self
                    text.placeholderString = field.range == nil ? localized("settings.automatic") : ""
                    text.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
                    textFields[field] = text
                    if field.isCache || field == .imgSizeToleranceBytes {
                        // Keep full Int64 precision; NSStepper stores its value as Double.
                        text.placeholderString = localized("settings.automatic")
                        control = text
                    } else if let range = field.range {
                        text.widthAnchor.constraint(equalToConstant: 100).isActive = true
                        let stepper = NSStepper()
                        stepper.minValue = Double(range.lowerBound)
                        stepper.maxValue = Double(range.upperBound)
                        stepper.increment = 1
                        stepper.target = self
                        stepper.action = #selector(stepNumber(_:))
                        steppers[field] = stepper
                        control = NSStackView(views: [text, stepper])
                    } else { control = text }
                }
                control.setAccessibilityLabel(field.title)
                control.toolTip = localized("settings.\(field.labelKey).help")
                control.setContentHuggingPriority(.defaultLow, for: .horizontal)
                var cells: [NSView] = toggles[field] == nil
                    ? [NSTextField(labelWithString: field.title), control] : [control, NSView()]
                if developer && field.runtimeKey != nil {
                    let source = NSButton()
                    source.image = NSImage(systemSymbolName: "info.circle", accessibilityDescription: nil)
                    source.target = self
                    source.action = #selector(showSources(_:))
                    source.isBordered = false
                    source.toolTip = localized("settings.sources.button", field.title)
                    source.setAccessibilityLabel(source.toolTip)
                    source.widthAnchor.constraint(equalToConstant: 22).isActive = true
                    source.heightAnchor.constraint(equalToConstant: 22).isActive = true
                    sourceButtons[field] = source
                    cells.append(source)
                } else if developer { cells.append(NSView()) }
                let row = grid.addRow(with: cells)
                if toggles[field] != nil { row.mergeCells(in: NSRange(location: 0, length: 2)) }
                rows[field] = row
                fieldSections[field] = section
                row.yPlacement = .center
            }
            if section == "cache" { addCacheRows(to: grid) }
            grid.column(at: 0).xPlacement = .leading
            grid.column(at: 0).width = 220
            grid.column(at: 1).xPlacement = .fill
            if developer {
                grid.column(at: 2).width = 22
                grid.column(at: 2).xPlacement = .center
            }
            grids[section] = grid
            let content = NSView()
            grid.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(grid)
            NSLayoutConstraint.activate([
                grid.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
                grid.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
                grid.topAnchor.constraint(equalTo: content.topAnchor, constant: 20),
                grid.widthAnchor.constraint(equalTo: content.widthAnchor, constant: -32),
            ])
            tab.view = content
            tabs.addTabViewItem(tab)
        }
        root.addArrangedSubview(tabs)
        tabHeight = tabs.heightAnchor.constraint(equalToConstant: 210)
        tabHeight?.isActive = true
        tabs.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        advanced.title = localized("settings.advanced")
        advanced.target = self
        advanced.action = #selector(updateVisibility)
        advanced.isHidden = true
        root.addArrangedSubview(advanced)
        status.font = .systemFont(ofSize: 11)
        status.textColor = .secondaryLabelColor
        status.maximumNumberOfLines = 3
        status.isHidden = true
        root.addArrangedSubview(status)
        status.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        resetButton.title = localized("settings.reset")
        resetButton.bezelStyle = .rounded
        resetButton.target = self
        resetButton.action = #selector(resetTab)
        cancelButton.title = localized("alert.cancel")
        cancelButton.bezelStyle = .rounded
        cancelButton.target = self
        cancelButton.action = #selector(cancel)
        cancelButton.keyEquivalent = "\u{1b}"
        applyButton.title = localized("settings.apply")
        applyButton.bezelStyle = .rounded
        applyButton.target = self
        applyButton.action = #selector(apply)
        applyButton.keyEquivalent = "\r"
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let actions = NSStackView(views: [resetButton, spacer, cancelButton, applyButton])
        actions.distribution = .fill
        root.addArrangedSubview(actions)
        actions.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        restore(MediaSettings(preferences: preferences))
    }

    private func configureSearch() {
        search.placeholderString = localized("settings.search")
        search.setAccessibilityLabel(localized("settings.search"))
        search.delegate = self
        search.sendsSearchStringImmediately = true
        root.addArrangedSubview(search)
        search.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        search.heightAnchor.constraint(equalToConstant: 24).isActive = true

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("setting"))
        searchTable.addTableColumn(column)
        searchTable.headerView = nil
        searchTable.rowHeight = 26
        searchTable.intercellSpacing = NSSize(width: 0, height: 0)
        searchTable.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        searchTable.allowsMultipleSelection = false
        searchTable.dataSource = self
        searchTable.delegate = self
        searchTable.target = self
        searchTable.action = #selector(activateSearchResult)
        searchTable.setAccessibilityLabel(localized("settings.search.results"))
        searchResults.documentView = searchTable
        searchResults.hasVerticalScroller = true
        searchResults.autohidesScrollers = true
        searchResults.borderType = .bezelBorder
        searchResults.isHidden = true
        root.addArrangedSubview(searchResults)
        searchResults.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        searchHeight = searchResults.heightAnchor.constraint(equalToConstant: 28)
        searchHeight?.isActive = true
        searchEmpty.stringValue = localized("settings.search.empty")
        searchEmpty.textColor = .secondaryLabelColor
        searchEmpty.isHidden = true
        root.addArrangedSubview(searchEmpty)
    }

    private func searchTitle(_ destination: SearchDestination) -> String {
        switch destination {
        case let .setting(field):
            let section = fieldSections[field] ?? field.section
            return field.title + " · " + localized("settings.section.\(section)")
        case .cache: return localized("settings.section.cache")
        }
    }

    private func matchingSettings(_ query: String) -> [SearchDestination] {
        let terms = query.split(whereSeparator: \.isWhitespace).map(String.init)
        guard !terms.isEmpty else { return [] }
        func matches(_ text: String) -> Bool { terms.allSatisfy { text.localizedStandardContains($0) } }
        var result = MediaSetting.allCases.compactMap { field -> SearchDestination? in
            guard rows[field] != nil, isApplicable(field) else { return nil }
            let destination = SearchDestination.setting(field)
            var words = [searchTitle(destination), localized("settings.\(field.labelKey).help")]
            words += field.choices.flatMap { [$0, localized("settings.value.\($0)")] }
            if developer { words += [field.runtimeKey ?? "", field.flag] }
            return matches(words.joined(separator: " ")) ? destination : nil
        }
        let cacheTerms = ["settings.section.cache", "settings.cache.path", "settings.cache.rebuildable",
                          "settings.cache.retained", "settings.cache.store", "settings.cache.obsolete",
                          "settings.cache.clear", "settings.cache.check", "settings.cache.prune",
                          "settings.cache.database", "settings.cache.journal", "settings.cache.pages", "settings.cache.backup",
                          "settings.history.path", "settings.history.backup", "settings.history.backup_help"].map { localized($0) }.joined(separator: " ")
        if matches(cacheTerms) { result.append(.cache) }
        return result
    }

    private func updateSearch() {
        let selected = searchMatches.indices.contains(searchTable.selectedRow) ? searchMatches[searchTable.selectedRow] : nil
        searchMatches = matchingSettings(search.stringValue)
        searchTable.reloadData()
        searchResults.isHidden = searchMatches.isEmpty
        searchEmpty.isHidden = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !searchMatches.isEmpty
        searchHeight?.constant = CGFloat(min(4, searchMatches.count)) * searchTable.rowHeight + 2
        if !searchMatches.isEmpty {
            let index = selected.flatMap { searchMatches.firstIndex(of: $0) } ?? 0
            searchTable.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            searchTable.scrollRowToVisible(index)
        }
        // Return in the search field navigates; it must never apply the draft.
        applyButton.keyEquivalent = search.stringValue.isEmpty ? "\r" : ""
        updatePanelSize()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { searchMatches.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard searchMatches.indices.contains(row) else { return nil }
        let title = searchTitle(searchMatches[row])
        let label = NSTextField(labelWithString: title)
        label.lineBreakMode = .byTruncatingTail
        label.toolTip = title
        return label
    }

    @objc private func activateSearchResult() {
        guard searchMatches.indices.contains(searchTable.selectedRow) else { return }
        let destination = searchMatches[searchTable.selectedRow]
        search.stringValue = ""
        updateSearch()
        switch destination {
        case let .setting(field):
            guard let section = fieldSections[field] else { return }
            if field.isPhotos && field.range != nil { advanced.state = .on }
            updateVisibility()
            tabs.selectTabViewItem(withIdentifier: section)
            let control: NSView? = (textFields[field] as NSView?) ?? popups[field] ?? toggles[field]
            if let control { panel.makeFirstResponder(control) }
        case .cache:
            tabs.selectTabViewItem(withIdentifier: "cache")
            panel.makeFirstResponder(cacheRefresh)
        }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        guard control === search else { return false }
        if commandSelector == #selector(NSResponder.insertNewline(_:)), !search.stringValue.isEmpty {
            activateSearchResult()
            return true
        }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)), !search.stringValue.isEmpty {
            search.stringValue = ""
            updateSearch()
            return true
        }
        if commandSelector == #selector(NSResponder.moveDown(_:)) || commandSelector == #selector(NSResponder.moveUp(_:)) {
            guard !searchMatches.isEmpty else { return true }
            let delta = commandSelector == #selector(NSResponder.moveDown(_:)) ? 1 : -1
            let index = min(searchMatches.count - 1, max(0, searchTable.selectedRow + delta))
            searchTable.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            searchTable.scrollRowToVisible(index)
            return true
        }
        return false
    }

    private func addCacheRows(to grid: NSGridView) {
        for key in ["path", "database", "journal", "pages", "rebuildable", "retained", "store", "obsolete"] {
            let value = NSTextField(labelWithString: localized("settings.cache.not_loaded"))
            value.isSelectable = true
            value.lineBreakMode = .byTruncatingMiddle
            value.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            cacheLabels[key] = value
            grid.addRow(with: [NSTextField(labelWithString: localized("settings.cache.\(key)")), value]).yPlacement = .center
        }
        cacheRefresh.image = NSImage(systemSymbolName: "arrow.clockwise", accessibilityDescription: localized("settings.cache.refresh"))
        cacheRefresh.bezelStyle = .rounded
        cacheRefresh.toolTip = localized("settings.cache.refresh")
        cacheRefresh.setAccessibilityLabel(localized("settings.cache.refresh"))
        cacheRefresh.target = self
        cacheRefresh.action = #selector(refreshCache)
        cacheRefresh.widthAnchor.constraint(equalToConstant: 32).isActive = true
        cacheClear.title = localized("settings.cache.clear")
        cacheClear.bezelStyle = .rounded
        cacheClear.target = self
        cacheClear.action = #selector(clearCache)
        cacheClear.isEnabled = false
        cachePrune.title = localized("settings.cache.prune")
        cachePrune.image = NSImage(systemSymbolName: "line.3.horizontal.decrease.circle", accessibilityDescription: nil)
        cachePrune.imagePosition = .imageLeading
        cachePrune.toolTip = localized("settings.cache.prune_detail")
        cachePrune.setAccessibilityLabel(localized("settings.cache.prune"))
        cachePrune.bezelStyle = .rounded
        cachePrune.target = self
        cachePrune.action = #selector(pruneCache)
        cachePrune.isEnabled = false
        cacheCheck.title = localized("settings.cache.check")
        cacheCheck.image = NSImage(systemSymbolName: "checkmark.shield", accessibilityDescription: nil)
        cacheCheck.imagePosition = .imageLeading
        cacheCheck.toolTip = localized("settings.cache.check")
        cacheCheck.setAccessibilityLabel(localized("settings.cache.check"))
        cacheCheck.bezelStyle = .rounded
        cacheCheck.target = self
        cacheCheck.action = #selector(checkCacheIntegrity)
        cacheBackup.title = localized("settings.cache.backup")
        cacheBackup.image = NSImage(systemSymbolName: "externaldrive", accessibilityDescription: nil)
        cacheBackup.imagePosition = .imageLeading
        cacheBackup.bezelStyle = .rounded
        cacheBackup.toolTip = localized("settings.cache.backup_help")
        cacheBackup.setAccessibilityLabel(localized("settings.cache.backup"))
        cacheBackup.target = self
        cacheBackup.action = #selector(backupStore)
        cacheBackup.isEnabled = false
        grid.addRow(with: [NSView(), NSStackView(views: [cacheRefresh, cacheCheck, cacheBackup])]).yPlacement = .center
        grid.addRow(with: [NSView(), NSStackView(views: [cachePrune, cacheClear])]).yPlacement = .center
        let historyPath = NSTextField(labelWithString: historyBackupDirectory.path)
        historyPath.isSelectable = true
        historyPath.lineBreakMode = .byTruncatingMiddle
        historyPath.toolTip = historyBackupDirectory.path
        historyPath.setAccessibilityLabel(localized("settings.history.path"))
        historyPath.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        grid.addRow(with: [NSTextField(labelWithString: localized("settings.history.path")), historyPath]).yPlacement = .center
        historyBackup.title = localized("settings.history.backup")
        historyBackup.image = NSImage(systemSymbolName: "externaldrive", accessibilityDescription: nil)
        historyBackup.imagePosition = .imageLeading
        historyBackup.bezelStyle = .rounded
        historyBackup.toolTip = localized("settings.history.backup_help")
        historyBackup.setAccessibilityLabel(localized("settings.history.backup"))
        historyBackup.target = self
        historyBackup.action = #selector(backupHistory)
        grid.addRow(with: [NSView(), NSStackView(views: [historyBackup])]).yPlacement = .center
        cacheMessage.font = .systemFont(ofSize: 11)
        cacheMessage.maximumNumberOfLines = 4
        cacheMessage.isSelectable = true
        cacheMessageRow = grid.addRow(with: [NSView(), cacheMessage])
        cacheMessageRow?.isHidden = true
    }

    private func displayCache(_ snapshot: LocalCacheStatus) throws {
        let cache = try snapshot.totals(rebuildable: true)
        let retained = try snapshot.totals(rebuildable: false)
        func size(_ bytes: UInt64) -> String { ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .binary) }
        cacheLabels["path"]?.stringValue = snapshot.cacheDirectory
        cacheLabels["path"]?.toolTip = snapshot.cacheDirectory
        cacheStore = snapshot.database.map {
            (URL(fileURLWithPath: snapshot.cacheDirectory, isDirectory: true).appendingPathComponent("mfb_store.sqlite"), $0.storeSchemaVersion)
        }
        if let database = snapshot.database {
            cacheLabels["database"]?.stringValue = localized("settings.cache.database_value", database.storeSchemaVersion, database.sqliteVersion)
            cacheLabels["database"]?.toolTip = cacheStore?.url.path
            cacheLabels["journal"]?.stringValue = database.journalMode.uppercased()
            cacheLabels["pages"]?.stringValue = localized("settings.cache.pages_value", String(database.pageCount), String(database.freePages), String(database.pageSize))
            cacheLabels["pages"]?.toolTip = localized("settings.cache.pages_help")
        } else {
            let value = snapshot.storeBytes == 0 ? localized("settings.cache.absent") : localized("result.unknown")
            for key in ["database", "journal", "pages"] { cacheLabels[key]?.stringValue = value; cacheLabels[key]?.toolTip = nil }
        }
        cacheLabels["rebuildable"]?.stringValue = localized("settings.cache.records", String(cache.rows), size(cache.bytes))
        cacheLabels["retained"]?.stringValue = localized("settings.cache.records", String(retained.rows), size(retained.bytes))
        cacheLabels["retained"]?.toolTip = snapshot.namespaces.filter { !$0.rebuildable }.map { "\($0.name): \($0.rows)" }.joined(separator: "\n")
        cacheLabels["store"]?.stringValue = size(snapshot.storeBytes)
        cacheLabels["store"]?.toolTip = localized("settings.cache.store_help")
        cacheLabels["obsolete"]?.stringValue = localized("settings.cache.files", String(snapshot.legacyAnalysisFiles), size(snapshot.legacyAnalysisBytes))
        cacheClear.isEnabled = !cacheBusy && (cache.rows > 0 || snapshot.legacyAnalysisFiles > 0)
        cachePrune.isEnabled = !cacheBusy && cache.rows > 0
        cacheCheck.isEnabled = !cacheBusy
        cacheBackup.isEnabled = !cacheBusy && cacheStore != nil
        cacheLoaded = true
    }

    @objc private func refreshCache() { updateCache(action: .refresh) }
    @objc private func checkCacheIntegrity() { updateCache(action: .check) }

    @objc private func backupHistory() {
        guard !cacheBusy, historyBackup.isEnabled else { return }
        let directory = historyBackupDirectory
        let save = NSSavePanel()
        save.title = localized("settings.history.backup")
        save.message = localized("settings.history.backup_help")
        save.nameFieldStringValue = "mfb-history-backup-\(UUID().uuidString.prefix(8)).sqlite"
        save.canCreateDirectories = true
        save.beginSheetModal(for: panel) { [weak self] response in
            guard let self, response == .OK, let destination = save.url, !self.cacheBusy,
                  self.panel.sheetParent != nil else { return }
            let previous = [self.cacheRefresh, self.cacheCheck, self.cacheBackup, self.cachePrune, self.cacheClear].map { ($0, $0.isEnabled) }
            self.cacheBusy = true
            for (button, _) in previous { button.isEnabled = false }
            self.cacheMessage.stringValue = localized("settings.history.backing_up")
            self.cacheMessage.textColor = .secondaryLabelColor
            self.cacheMessage.toolTip = nil
            self.cacheMessageRow?.isHidden = false
            self.updatePanelSize()
            DispatchQueue.global(qos: .utility).async { [weak self] in
                let result = Result { try backupHistoryDatabase(to: destination, directory: directory) }
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.cacheBusy = false
                    for (button, enabled) in previous { button.isEnabled = enabled }
                    guard self.panel.sheetParent != nil else { return }
                    do {
                        let receipt = try result.get()
                        self.cacheMessage.stringValue = localized("settings.history.backed_up", destination.lastPathComponent,
                            String(receipt.sessions), String(receipt.events), String(receipt.outputRows))
                        self.cacheMessage.textColor = .labelColor
                        self.cacheMessage.toolTip = receipt.destination
                    } catch {
                        self.cacheMessage.stringValue = error.localizedDescription
                        self.cacheMessage.textColor = .systemRed
                    }
                    self.updatePanelSize()
                }
            }
        }
    }

    @objc private func backupStore() {
        guard !cacheBusy, cacheLoaded, cacheBackup.isEnabled, let store = cacheStore else { return }
        let save = NSSavePanel()
        save.title = localized("settings.cache.backup")
        save.message = localized("settings.cache.backup_help")
        save.nameFieldStringValue = "mfb-core-backup-\(UUID().uuidString.prefix(8)).sqlite"
        save.canCreateDirectories = true
        save.beginSheetModal(for: panel) { [weak self] response in
            guard let self, response == .OK, let destination = save.url, !self.cacheBusy,
                  self.panel.sheetParent != nil else { return }
            let previousCleanup = (self.cachePrune.isEnabled, self.cacheClear.isEnabled)
            self.cacheBusy = true
            for button in [self.cacheRefresh, self.cacheCheck, self.cacheBackup, self.cachePrune, self.cacheClear, self.historyBackup] { button.isEnabled = false }
            self.cacheMessage.stringValue = localized("settings.cache.backing_up")
            self.cacheMessage.textColor = .secondaryLabelColor
            self.cacheMessage.toolTip = nil
            self.cacheMessageRow?.isHidden = false
            self.updatePanelSize()
            DispatchQueue.global(qos: .utility).async { [weak self] in
                let result = Result { try backupCoreDatabase(to: destination, source: store.url, schema: store.schema) }
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.cacheBusy = false
                    self.cacheRefresh.isEnabled = true
                    self.cacheCheck.isEnabled = true
                    self.cacheBackup.isEnabled = self.cacheStore != nil && self.cacheLoaded
                    self.cachePrune.isEnabled = previousCleanup.0
                    self.cacheClear.isEnabled = previousCleanup.1
                    guard self.panel.sheetParent != nil else { return }
                    do {
                        let receipt = try result.get()
                        self.cacheMessage.stringValue = localized("settings.cache.backed_up", destination.lastPathComponent,
                            ByteCountFormatter.string(fromByteCount: Int64(receipt.bytes), countStyle: .binary))
                        self.cacheMessage.textColor = .labelColor
                        self.cacheMessage.toolTip = receipt.destination
                    } catch {
                        self.cacheMessage.stringValue = error.localizedDescription
                        self.cacheMessage.textColor = .systemRed
                    }
                    self.updatePanelSize()
                }
            }
        }
    }

    @objc private func pruneCache() {
        guard !cacheBusy, cacheLoaded, cachePrune.isEnabled else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = localized("settings.cache.prune_confirm")
        alert.informativeText = localized("settings.cache.prune_detail")
        alert.addButton(withTitle: localized("settings.cache.prune"))
        alert.addButton(withTitle: localized("alert.cancel"))
        alert.beginSheetModal(for: panel) { [weak self] response in
            if response == .alertFirstButtonReturn { self?.updateCache(action: .prune) }
        }
    }

    @objc private func clearCache() {
        guard !cacheBusy, cacheLoaded, cacheClear.isEnabled else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = localized("settings.cache.confirm")
        alert.informativeText = localized("settings.cache.confirm_detail")
        alert.addButton(withTitle: localized("settings.cache.clear"))
        alert.addButton(withTitle: localized("alert.cancel"))
        alert.beginSheetModal(for: panel) { [weak self] response in
            if response == .alertFirstButtonReturn { self?.updateCache(action: .clear) }
        }
    }

    private func updateCache(action: LocalCacheAction) {
        guard !cacheBusy else { return }
        let settings = draft()
        let fast = self.fast, videos = self.videos
        cacheBusy = true
        cacheRefresh.isEnabled = false
        cacheClear.isEnabled = false
        cachePrune.isEnabled = false
        cacheCheck.isEnabled = false
        cacheBackup.isEnabled = false
        cacheMessage.toolTip = nil
        cacheMessage.maximumNumberOfLines = 4
        let message: String
        switch action {
        case .refresh: message = "settings.cache.loading"
        case .check: message = "settings.cache.checking"
        case .clear: message = "settings.cache.clearing"
        case .prune: message = "settings.cache.pruning"
        }
        cacheMessage.stringValue = localized(message)
        cacheMessage.textColor = .secondaryLabelColor
        cacheMessageRow?.isHidden = false
        updatePanelSize()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Result {
                var arguments: [String] = []
                if action == .prune {
                    let fields: [MediaSetting] = [.cacheMaxBytes, .cacheTtlSeconds]
                        + (fast ? [.fastConfig] : (videos ? [.vidConfig] : [.imgConfig, .vidConfig]))
                    try settings.validate(fields)
                    var policies: [EffectiveRuntimeSettings] = []
                    if !videos {
                        policies.append(try queryRuntimeSettings(arguments: settings.runtimeArguments(fast: fast, inheritedOnly: true)
                            + fields.filter(\.isCache).flatMap { field in settings.values[field].map(field.arguments) ?? [] }))
                    }
                    if !fast {
                        let configuration = settings.videoConfigurationFile.map { ["--config", $0] } ?? []
                        policies.append(try queryRuntimeSettings(arguments: configuration
                            + fields.filter(\.isCache).flatMap { field in settings.values[field].map(field.arguments) ?? [] }, tool: "vid"))
                    }
                    arguments = try cachePruningArguments(policies)
                }
                return try queryLocalCache(action: action, policyArguments: arguments)
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.cacheBusy = false
                self.cacheRefresh.isEnabled = true
                self.cacheCheck.isEnabled = true
                do {
                    let snapshot = try result.get()
                    try self.displayCache(snapshot)
                    self.cacheMessage.stringValue = action == .clear
                        ? localized("settings.cache.cleared", String(snapshot.removedRows), String(snapshot.removedFiles)) : ""
                    if let pruning = snapshot.pruning {
                        self.cacheMessage.stringValue = localized("settings.cache.pruned", String(pruning.expiredRows),
                            String(pruning.evictedRows), String(pruning.policy.pathTreeMaxBytes), String(pruning.policy.pathTreeTtlSeconds))
                    }
                    self.cacheMessageRow?.isHidden = self.cacheMessage.stringValue.isEmpty
                    if let report = snapshot.integrity { self.displayCacheIntegrity(report) }
                } catch {
                    self.cacheLoaded = false
                    self.cacheStore = nil
                    self.cacheBackup.isEnabled = false
                    for label in self.cacheLabels.values { label.stringValue = localized("settings.cache.unavailable"); label.toolTip = nil }
                    self.cacheMessage.stringValue = error.localizedDescription
                    self.cacheMessage.textColor = .systemRed
                    self.cacheMessageRow?.isHidden = false
                }
                self.updatePanelSize()
            }
        }
    }

    private func displayCacheIntegrity(_ report: LocalCacheStatus.Integrity) {
        cacheMessage.maximumNumberOfLines = 0
        if report.status == .error {
            cacheLoaded = false
            cacheStore = nil
            cacheBackup.isEnabled = false
            cacheClear.isEnabled = false
            cachePrune.isEnabled = false
            for (key, label) in cacheLabels where key != "path" {
                label.stringValue = localized("settings.cache.unavailable")
                label.toolTip = nil
            }
        }
        cacheMessage.stringValue = localized("settings.cache.integrity.\(report.status.rawValue)")
        if report.status != .absent {
            cacheMessage.stringValue += "\n" + localized("settings.cache.checked",
                String(report.checkedCacheRows), String(report.checkedProtectedRows),
                String(report.corruptCacheRows), String(report.corruptProtectedRows), String(report.issueCount))
        }
        cacheMessage.textColor = report.status == .unhealthy || report.status == .error ? .systemRed : .labelColor
        cacheMessage.toolTip = report.issues.map { "\($0.code): \($0.message)" }.joined(separator: "\n")
        cacheMessageRow?.isHidden = false
    }

    func show(for window: NSWindow) {
        if tabs.numberOfTabViewItems > 0 { tabs.selectTabViewItem(at: 0) }
        window.beginSheet(panel)
        loadInheritedValues()
    }

    func tabView(_ tabView: NSTabView, didSelect tabViewItem: NSTabViewItem?) {
        updatePanelSize()
        if tabViewItem?.identifier as? String == "cache", !cacheLoaded && !validatingLayout { refreshCache() }
    }

    private func updatePanelSize() {
        guard let section = tabs.selectedTabViewItem?.identifier as? String,
              let grid = grids[section], let tabHeight else { return }
        advanced.isHidden = developer || section != "photos"
        cancelButton.title = localized("alert.cancel")
        cancelButton.isEnabled = !cacheBusy
        resetButton.isEnabled = !cacheBusy
        applyButton.isEnabled = !cacheBusy && !applying
        historyBackup.isEnabled = !cacheBusy
        status.isHidden = status.stringValue.isEmpty
        let visibleRows = (0..<grid.numberOfRows).map { grid.row(at: $0) }.filter { !$0.isHidden }
        let rowsHeight: CGFloat
        if section == "cache" {
            panel.contentView?.layoutSubtreeIfNeeded()
            rowsHeight = grid.frame.height
        } else {
            rowsHeight = visibleRows.reduce(CGFloat(0)) { height, row in
                height + max(24, row.cell(at: 1).contentView?.fittingSize.height ?? 24)
            } + CGFloat(max(0, visibleRows.count - 1)) * grid.rowSpacing
        }
        tabHeight.constant = rowsHeight + 64
        let searchExtras: CGFloat = 36 + (searchResults.isHidden ? 0 : (searchHeight?.constant ?? 0) + 12)
            + (searchEmpty.isHidden ? 0 : 30)
        let extras: CGFloat = (advanced.isHidden ? 0 : 32) + (status.isHidden ? 0 : 52) + searchExtras
        panel.setContentSize(NSSize(width: 660, height: tabHeight.constant + 76 + extras))
        panel.contentView?.layoutSubtreeIfNeeded()
    }

    private func loadInheritedValues() {
        inheritedGeneration = UUID()
        let generation = inheritedGeneration
        let settings = draft()
        let standardArgs = settings.runtimeArguments(fast: false, inheritedOnly: true)
        let fastArgs = settings.runtimeArguments(fast: true, inheritedOnly: true)
        let videoArgs = settings.videoRuntimeArguments(inheritedOnly: true)
        let needsStandard = !fast && !videos
        let needsVideo = !fast
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = Result {
                (try needsStandard ? queryRuntimeSettings(arguments: standardArgs) : nil,
                 try queryRuntimeSettings(arguments: fastArgs),
                 try needsVideo ? queryRuntimeSettings(arguments: videoArgs, tool: "vid") : nil)
            }
            DispatchQueue.main.async { [weak self] in
                guard let self, self.inheritedGeneration == generation else { return }
                switch result {
                case let .success((standard, fast, video)):
                    let draft = self.draft()
                    self.setInheritedValues(standard: standard, fast: fast, video: video)
                    self.restore(draft)
                    self.status.stringValue = ""
                case let .failure(error):
                    let draft = self.draft()
                    self.inherited.removeAll()
                    self.inheritedSources.removeAll()
                    self.inheritedMixed.removeAll()
                    self.restore(draft)
                    self.status.stringValue = error.localizedDescription
                }
                self.updatePanelSize()
            }
        }
    }

    private func setInheritedValues(standard: EffectiveRuntimeSettings?, fast: EffectiveRuntimeSettings,
                                    video: EffectiveRuntimeSettings?) {
        inherited.removeAll()
        inheritedSources.removeAll()
        inheritedMixed.removeAll()
        for field in MediaSetting.allCases {
            guard let key = field.runtimeKey else { continue }
            let effective = field.isShared ? (self.fast ? fast : (videos ? video : standard))
                : (field.isFastImage || field.isPhotos ? fast : (field.isVideo ? video : standard))
            inherited[field] = effective?[key]
            inheritedSources[field] = effective?.sourceChain[key]?.joined(separator: " → ")
        }
        for field in MediaSetting.allCases where field.isShared && !self.fast && !videos {
            guard let key = field.runtimeKey, let imageValue = standard?[key],
                  let videoValue = video?[key], imageValue != videoValue else { continue }
            inheritedMixed.insert(field)
            inherited[field] = nil
            inheritedSources[field] = "IMG: \(imageValue) (\(standard?.sources[key] ?? localized("result.unknown")))\nVID: \(videoValue) (\(video?.sources[key] ?? localized("result.unknown")))"
        }
    }

    private func restore(_ settings: MediaSettings) {
        restored = settings
        displayed = inherited.merging(settings.values) { _, explicit in explicit }
        for (field, text) in textFields {
            text.stringValue = displayed[field] ?? ""
            if field.isCache {
                text.placeholderString = localized(inheritedMixed.contains(field) ? "settings.performance.per_pipeline" : "settings.automatic")
            }
            updateStepper(for: field)
        }
        for (field, toggle) in toggles {
            let value = displayed[field]
            toggle.state = value == "true" ? .on : .off
            toggle.isEnabled = value.flatMap(Bool.init) != nil
            toggle.title = field.title
            if let value, Bool(value) == nil { toggle.title = localized("settings.invalid", field.title, value) }
        }
        for (field, popup) in popups {
            for index in popup.itemArray.indices.reversed()
                where !field.choices.contains(popup.item(at: index)?.representedObject as? String ?? "") {
                popup.removeItem(at: index)
            }
            popup.selectItem(at: -1)
            if field.isFailurePolicy {
                let inheritedValue = inherited[field].map {
                    field.choices.contains($0) ? localized("settings.value.\($0)") : localized("settings.invalid", field.title, $0)
                } ?? localized("result.unknown")
                popup.insertItem(withTitle: localized("settings.failure.inherited", inheritedValue), at: 0)
                if settings.values[field] == nil { popup.selectItem(at: 0); continue }
            }
            if field == .performance && inheritedMixed.contains(field) {
                popup.insertItem(withTitle: localized("settings.performance.per_pipeline"), at: 0)
                if settings.values[field] == nil { popup.selectItem(at: 0) }
            }
            if let value = displayed[field] {
                if let item = popup.itemArray.first(where: { ($0.representedObject as? String) == value }) {
                    popup.select(item)
                } else {
                    popup.addItem(withTitle: localized("settings.invalid", field.title, value))
                    popup.lastItem?.representedObject = value
                    popup.select(popup.lastItem)
                }
            }
        }
        if developer {
            for (field, row) in rows {
                row.cell(at: 1).contentView?.toolTip = localized("settings.\(field.labelKey).help")
                    + "\n" + (field.runtimeKey ?? field.flag) + ": " + (displayed[field] ?? localized("settings.automatic"))
                    + "\n" + (settings.values[field] == nil ? (inheritedSources[field] ?? localized("result.unknown")) : "GUI")
            }
        }
        updateVisibility()
    }

    private func draft() -> MediaSettings {
        var settings = restored
        for (field, text) in textFields {
            if text.stringValue != (displayed[field] ?? "") {
                settings.values[field] = text.stringValue.isEmpty ? nil : text.stringValue
            }
        }
        for (field, toggle) in toggles {
            let value = toggle.state == .on ? "true" : "false"
            if toggle.isEnabled && value != displayed[field] { settings.values[field] = value }
        }
        for (field, popup) in popups {
            if field.isFailurePolicy {
                settings.values[field] = popup.selectedItem?.representedObject as? String
                continue
            }
            if field == .performance, inheritedMixed.contains(field), popup.indexOfSelectedItem == 0 {
                settings.values[field] = nil
                continue
            }
            if let value = popup.selectedItem?.representedObject as? String, value != displayed[field] {
                settings.values[field] = value
            }
        }
        return settings
    }

    private func isApplicable(_ field: MediaSetting) -> Bool {
        let backend = (popups[.photosBackend]?.selectedItem?.representedObject as? String) ?? displayed[.photosBackend]
        let adaptive = toggles[.photosAdaptive]?.state == .on
        if field == .photosAppleScriptBatch && backend == "native" { return false }
        if [.photosNativeBatch, .photosAdaptive, .photosMinimumBatch, .photosMaximumBatch, .photosTargetSeconds].contains(field),
           backend == "applescript" { return false }
        if [.photosMinimumBatch, .photosMaximumBatch, .photosTargetSeconds].contains(field), !adaptive { return false }
        return true
    }

    @objc private func updateVisibility() {
        sourceGeneration = UUID()
        for (field, row) in rows {
            row.isHidden = !isApplicable(field) || (field.range != nil && field.isPhotos && !developer && advanced.state != .on)
        }
        updateSearch()
    }

    @objc private func stepNumber(_ sender: NSStepper) {
        sourceGeneration = UUID()
        if let field = steppers.first(where: { $0.value === sender })?.key {
            textFields[field]?.integerValue = sender.integerValue
        }
    }

    func controlTextDidChange(_ notification: Notification) {
        if notification.object as? NSSearchField === search { updateSearch(); return }
        sourceGeneration = UUID()
        guard let text = notification.object as? NSTextField,
              let field = textFields.first(where: { $0.value === text })?.key else { return }
        updateStepper(for: field)
        if [.imgConfig, .fastConfig, .vidConfig].contains(field) {
            inheritedGeneration = UUID()
        }
    }

    func controlTextDidEndEditing(_ notification: Notification) {
        guard let text = notification.object as? NSTextField,
              let field = textFields.first(where: { $0.value === text })?.key,
              [.imgConfig, .fastConfig, .vidConfig].contains(field) else { return }
        loadInheritedValues()
    }

    private func updateStepper(for field: MediaSetting) {
        guard let stepper = steppers[field], let text = textFields[field], let range = field.range else { return }
        if text.stringValue.isEmpty {
            stepper.integerValue = range.lowerBound
            stepper.isEnabled = true
        } else if let value = Int(text.stringValue), range.contains(value) {
            stepper.integerValue = value
            stepper.isEnabled = true
        } else {
            stepper.isEnabled = false
        }
    }

    @objc private func showSources(_ sender: NSButton) {
        guard let field = sourceButtons.first(where: { $0.value === sender })?.key,
              let key = field.runtimeKey else { return }
        sourcePopover.close()
        do {
            let settings = draft()
            try settings.validate()
            sourceGeneration = UUID()
            let generation = sourceGeneration
            let requests: [(String, String, [String])]
            if field.isShared && !fast && !videos {
                requests = [("IMG", "img", settings.runtimeArguments(fast: false, inheritedOnly: false)),
                            ("VID", "vid", settings.videoRuntimeArguments(inheritedOnly: false))]
            } else if field.isVideo || (field.isShared && videos) {
                requests = [("VID", "vid", settings.videoRuntimeArguments(inheritedOnly: false))]
            } else {
                let useFast = field.isFastImage || field.isPhotos || (field.isShared && fast)
                requests = [(useFast ? "Fast IMG" : "IMG", "img", settings.runtimeArguments(fast: useFast, inheritedOnly: false))]
            }
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let result = Result {
                    try requests.map { label, tool, arguments in
                        let effective = try queryRuntimeSettings(arguments: arguments, tool: tool)
                        return "\(label)\n" + (try effective.originDescription(key: key, guiOverride: settings.values[field] != nil))
                    }.joined(separator: "\n\n")
                }
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.panel.sheetParent != nil, self.sourceGeneration == generation,
                          self.draft().values == settings.values else { return }
                    do {
                        let text = try result.get()
                        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 520, height: 260))
                        scroll.hasVerticalScroller = true
                        scroll.drawsBackground = false
                        let view = NSTextView(frame: scroll.bounds)
                        view.isEditable = false
                        view.isSelectable = true
                        view.drawsBackground = false
                        view.textColor = .labelColor
                        view.textContainerInset = NSSize(width: 12, height: 12)
                        view.autoresizingMask = [.width]
                        view.textContainer?.widthTracksTextView = true
                        view.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
                        view.string = "\(field.title)\n\(localized("settings.sources.preview"))\n\n\(text)"
                        view.setAccessibilityLabel(localized("settings.sources.preview"))
                        scroll.documentView = view
                        let controller = NSViewController()
                        controller.view = scroll
                        self.sourcePopover.contentViewController = controller
                        self.sourcePopover.contentSize = scroll.frame.size
                        self.sourcePopover.behavior = .transient
                        self.sourcePopover.show(relativeTo: sender.bounds, of: sender, preferredEdge: .minX)
                    } catch { NSAlert(error: error).beginSheetModal(for: self.panel) }
                }
            }
        } catch { NSAlert(error: error).beginSheetModal(for: panel) }
    }

    @objc private func resetTab() {
        var settings = draft()
        let section = tabs.selectedTabViewItem?.identifier as? String
        for field in rows.keys where field.section == section || (section == "img" && field.isFastImage && !field.isDeveloper) {
            settings.values.removeValue(forKey: field)
        }
        restore(settings)
        if section == "developer" && !validatingLayout { loadInheritedValues() }
    }

    @objc private func cancel() {
        guard !cacheBusy else { return }
        sourcePopover.close()
        sourceGeneration = UUID()
        applyGeneration = UUID()
        inheritedGeneration = UUID()
        applying = false
        panel.sheetParent?.endSheet(panel)
    }

    @objc private func apply() {
        guard !applying, !cacheBusy else { return }
        sourcePopover.close()
        sourceGeneration = UUID()
        do {
            let settings = draft()
            try settings.validate()
            applying = true
            applyGeneration = UUID()
            let generation = applyGeneration
            let standardArgs = settings.runtimeArguments(fast: false, inheritedOnly: false)
            let fastArgs = settings.runtimeArguments(fast: true, inheritedOnly: false)
            let videoArgs = settings.videoRuntimeArguments(inheritedOnly: false)
            let needsStandard = !fast && !videos
            let needsVideo = !fast
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let result = Result {
                    if needsStandard { _ = try queryRuntimeSettings(arguments: standardArgs) }
                    _ = try queryRuntimeSettings(arguments: fastArgs)
                    if needsVideo { _ = try queryRuntimeSettings(arguments: videoArgs, tool: "vid") }
                }
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.applyGeneration == generation else { return }
                    self.applying = false
                    do {
                        try result.get()
                        try settings.save(to: self.preferences)
                        self.applied()
                        self.cancel()
                    } catch { NSAlert(error: error).beginSheetModal(for: self.panel) }
                }
            }
        } catch { NSAlert(error: error).beginSheetModal(for: panel) }
    }

    func validateForSelfTest() throws {
        validatingLayout = true
        defer { validatingLayout = false }
        try validateCacheForSelfTest()
        try validateCachePolicyForSelfTest()
        try validatePhotosForSelfTest()
        try validateSearchForSelfTest()
        try validateFailurePolicyForSelfTest()
        if !fast && !videos {
            var budget = MediaSettings()
            budget.values = [.imgSizeToleranceBytes: String(Int.max)]
            restore(budget)
            guard draft().imageSizeToleranceBytes == Int.max, steppers[.imgSizeToleranceBytes] == nil,
                  draft().imageSizeTolerance == nil else {
                throw HostError(message: "Image growth limit lost precision or enabled tolerance")
            }
            budget.values[.imgSizeToleranceBytes] = "0"
            restore(budget)
            guard draft().imageSizeToleranceBytes == 0 else {
                throw HostError(message: "Zero image growth budget was not preserved")
            }
            for invalid in ["-1", "1.5", "9223372036854775808"] {
                budget.values[.imgSizeToleranceBytes] = invalid
                do { try budget.validate() }
                catch { continue }
                throw HostError(message: "Invalid image growth budget was accepted")
            }
        }
        var settings = MediaSettings()
        let effort: MediaSetting = fast ? .fastJpegEffort : .imgJpegEffort
        let database: MediaSetting = fast ? .fastDatabase : .imgDatabase
        let fallback: MediaSetting = fast ? .fastFallback : .imgFallback
        settings.values = [fallback: "strict", effort: "9", database: "false",
                           .vidCodec: "av1", .vidErrorMode: "fail-fast"]
        restore(settings)
        guard draft().values == settings.values, textFields[effort]?.stringValue == "9" else {
            throw HostError(message: "Settings controls did not restore explicit overrides")
        }
        textFields[effort]?.stringValue = "10"
        controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: textFields[effort]))
        guard steppers[effort]?.integerValue == 10, steppers[effort]?.isEnabled == true else {
            throw HostError(message: "Numeric editing left a stale stepper value")
        }
        steppers[effort]?.integerValue = 9
        if let stepper = steppers[effort] { stepNumber(stepper) }
        guard textFields[effort]?.stringValue == "9" else {
            throw HostError(message: "Stepper did not continue from the edited number")
        }
        textFields[effort]?.stringValue = "12"
        controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: textFields[effort]))
        guard steppers[effort]?.isEnabled == false, draft().values[effort] == "12" else {
            throw HostError(message: "Invalid numeric edit was silently replaced")
        }
        restore(settings)
        try draft().save(to: preferences)
        guard MediaSettings(preferences: preferences).values == settings.values else {
            throw HostError(message: "Media settings did not persist independently")
        }
        tabs.selectTabViewItem(at: 0)
        resetTab()
        guard draft().values == [.vidCodec: "av1", .vidErrorMode: "fail-fast"], textFields[effort]?.stringValue.isEmpty == true else {
            throw HostError(message: "Resetting image settings changed video settings")
        }
        for index in 0..<tabs.numberOfTabViewItems {
            tabs.selectTabViewItem(at: index)
            panel.contentView?.layoutSubtreeIfNeeded()
            guard let content = tabs.selectedTabViewItem?.view, let grid = content.subviews.first as? NSGridView,
                  content.bounds.contains(grid.frame), grid.frame.width >= content.bounds.width - 40 else {
                throw HostError(message: "Settings grid extends outside its tab")
            }
            if tabs.selectedTabViewItem?.identifier as? String == "performance",
               let surface = panel.contentView, surface.bounds.height > 230 {
                throw HostError(message: "Single-row settings tab retained oversized empty space")
            }
            for rowIndex in 0..<grid.numberOfRows {
                let row = grid.row(at: rowIndex)
                if row.isHidden { continue }
                if let toggle = row.cell(at: 0).contentView as? NSButton, toggles.values.contains(where: { $0 === toggle }) {
                    guard toggle.frame.width + 1 >= toggle.intrinsicContentSize.width else {
                        throw HostError(message: "Settings checkbox label clipped")
                    }
                    continue
                }
                guard let label = row.cell(at: 0).contentView, let control = row.cell(at: 1).contentView,
                      label.frame.width + 1 >= label.intrinsicContentSize.width,
                      !label.frame.intersects(control.frame) else {
                    throw HostError(message: "Settings labels overlap or clip")
                }
            }
            for (field, popup) in popups where rows[field]?.isHidden == false && popup.window != nil {
                guard popup.bounds.width + 1 >= popup.intrinsicContentSize.width else {
                    throw HostError(message: "Settings choice clipped: \(field.rawValue)")
                }
            }
        }
        var invalid = MediaSettings()
        invalid.values = [database: "invalid"]
        restore(invalid)
        restore(invalid)
        guard draft().values[database] == "invalid" else {
            throw HostError(message: "Invalid saved checkbox was silently converted to a valid value")
        }
        guard popups[database]?.numberOfItems == database.choices.count + 1 else {
            throw HostError(message: "Reloading invalid settings duplicated menu entries")
        }
        var independent = MediaSettings()
        independent.values = [.imgJpegEffort: "11", .fastJpegEffort: "8", .photosNativeBatch: "250", .photosAlbum: "Selected"]
        restore(independent)
        guard popups[database]?.numberOfItems == database.choices.count else {
            throw HostError(message: "Recovered settings kept stale invalid menu entries")
        }
        tabs.selectTabViewItem(withIdentifier: "img")
        resetTab()
        independent.values.removeValue(forKey: effort)
        guard draft().values == independent.values else { throw HostError(message: "Fast IMG reset affected other groups") }
        inherited[effort] = "11"
        inherited[database] = "true"
        restore(MediaSettings())
        guard textFields[effort]?.stringValue == "11",
              popups[database]?.selectedItem?.representedObject as? String == "true",
              draft().values.isEmpty, toggles.values.allSatisfy({ !$0.allowsMixedState }) else {
            throw HostError(message: "Effective values must not create overrides or mixed states")
        }
        try MediaSettings().save(to: preferences)
        guard MediaSettings(preferences: preferences).values.isEmpty else {
            throw HostError(message: "Reset settings still override inherited configuration")
        }
        let standard = EffectiveRuntimeSettings(values: ["performance.mode": "tight", "tools.policy": "single"],
                                                sources: ["performance.mode": "/synthetic/img.json", "tools.policy": "CLI"],
                                                sourceChain: ["performance.mode": ["default", "/synthetic/img.json"], "tools.policy": ["default", "CLI"]])
        let fastValues = EffectiveRuntimeSettings(values: ["performance.mode": "adaptive", "tools.policy": "fallback"],
                                                  sources: ["performance.mode": "default", "tools.policy": "default"],
                                                  sourceChain: ["performance.mode": ["default"], "tools.policy": ["default"]])
        let video = EffectiveRuntimeSettings(values: ["performance.mode": "relaxed", "vid.codec": "av1"],
                                             sources: ["performance.mode": "/synthetic/vid.json", "vid.codec": "/synthetic/vid.json"],
                                             sourceChain: ["performance.mode": ["default", "/synthetic/vid.json"], "vid.codec": ["default", "/synthetic/vid.json"]])
        setInheritedValues(standard: standard, fast: fastValues, video: video)
        restore(MediaSettings())
        guard rows.keys.filter({ $0.runtimeKey != nil }).allSatisfy({ (sourceButtons[$0] != nil) == developer }),
              sourceButtons.values.allSatisfy({ $0.frame.width == 22 }) else {
            throw HostError(message: "Configuration source controls escaped Developer mode or lost their fixed size")
        }
        if !fast && !videos {
            guard inheritedMixed.contains(.performance), popups[.performance]?.indexOfSelectedItem == 0,
                  draft().values[.performance] == nil, inherited[.vidCodec] == "av1" else {
                throw HostError(message: "Mixed pipeline defaults became a fabricated shared performance override")
            }
            var explicit = MediaSettings()
            explicit.values[.performance] = "balanced"
            restore(explicit)
            guard draft().values[.performance] == "balanced" else {
                throw HostError(message: "Explicit shared performance choice was lost")
            }
            popups[.performance]?.selectItem(at: 0)
            guard draft().values[.performance] == nil else {
                throw HostError(message: "Returning to per-pipeline defaults kept a shared override")
            }
            restore(MediaSettings())
            restore(MediaSettings())
            guard popups[.performance]?.numberOfItems == MediaSetting.performance.choices.count + 1 else {
                throw HostError(message: "Repeated reload duplicated the per-pipeline choice")
            }
        } else {
            guard !inheritedMixed.contains(.performance), inherited[.performance] == (fast ? "adaptive" : "relaxed") else {
                throw HostError(message: "Performance inheritance used the wrong pipeline")
            }
        }
        if developer {
            let tool: MediaSetting = fast ? .fastToolPolicy : .imgToolPolicy
            if !videos {
                guard popups[tool] != nil,
                      inherited[tool] == (fast ? "fallback" : "single") else {
                    throw HostError(message: "Developer tool policy is missing or inherited from another pipeline")
                }
            }
        }
        inherited.removeAll()
        inheritedSources.removeAll()
        inheritedMixed.removeAll()
        restore(MediaSettings())
    }

    private func validateFailurePolicyForSelfTest() throws {
        let key = "execution.error_mode"
        guard MediaSetting.imgErrorMode.runtimeKey == key, MediaSetting.vidErrorMode.runtimeKey == key else {
            throw HostError(message: "Failure policies are missing their effective configuration key")
        }
        if !developer || fast {
            guard popups[.imgErrorMode] == nil, popups[.vidErrorMode] == nil else {
                throw HostError(message: "Standard failure controls escaped Developer mode or entered Fast mode")
            }
            return
        }
        let previous = restored
        let previousInherited = inherited
        let previousSources = inheritedSources
        let previousMixed = inheritedMixed
        defer {
            inherited = previousInherited
            inheritedSources = previousSources
            inheritedMixed = previousMixed
            restore(previous)
        }
        let image = EffectiveRuntimeSettings(values: [key: "fail-fast"], sources: [key: "MFB_ERROR_MODE"],
                                             sourceChain: [key: ["default", "MFB_ERROR_MODE"]])
        let video = EffectiveRuntimeSettings(values: [key: "log-and-continue"], sources: [key: "/synthetic/vid.json"],
                                             sourceChain: [key: ["default", "/synthetic/vid.json"]])
        setInheritedValues(standard: image, fast: image, video: video)
        restore(MediaSettings())
        restore(MediaSettings())
        let fields: [MediaSetting] = videos ? [.vidErrorMode] : [.imgErrorMode, .vidErrorMode]
        for field in fields {
            guard let popup = popups[field], popup.indexOfSelectedItem == 0,
                  popup.numberOfItems == field.choices.count + 1, sourceButtons[field] != nil,
                  inherited[field] == (field.isImage ? "fail-fast" : "log-and-continue"),
                  inheritedSources[field] == (field.isImage ? "default → MFB_ERROR_MODE" : "default → /synthetic/vid.json"),
                  draft().values[field] == nil else {
                throw HostError(message: "Failure inheritance fabricated an override or used the wrong pipeline")
            }
            popup.selectItem(at: field.isImage ? 2 : 1)
            guard draft().values[field] == inherited[field] else {
                throw HostError(message: "Choosing the inherited failure value explicitly lost its override")
            }
            try draft().save(to: preferences)
            guard MediaSettings(preferences: preferences).values[field] == inherited[field] else {
                throw HostError(message: "Explicit failure override did not persist")
            }
            popup.selectItem(at: 0)
            try draft().save(to: preferences)
            guard draft().values[field] == nil, MediaSettings(preferences: preferences).values[field] == nil else {
                throw HostError(message: "Returning to failure inheritance retained an override")
            }
        }
        var explicit = MediaSettings()
        explicit.values = [.imgErrorMode: "log-and-continue", .vidErrorMode: "fail-fast"]
        restore(explicit)
        tabs.selectTabViewItem(withIdentifier: "developer")
        resetTab()
        guard fields.allSatisfy({ draft().values[$0] == nil && popups[$0]?.indexOfSelectedItem == 0 }) else {
            throw HostError(message: "Developer reset forced a failure default instead of inheritance")
        }
        inherited.removeAll()
        inheritedSources.removeAll()
        restore(MediaSettings())
        guard fields.allSatisfy({ displayed[$0] == nil && popups[$0]?.indexOfSelectedItem == 0
            && popups[$0]?.titleOfSelectedItem == localized("settings.failure.inherited", localized("result.unknown")) }) else {
            throw HostError(message: "Unknown failure policy was presented as record-and-continue")
        }
    }

    func validatePhotosForSelfTest() throws {
        guard tabs.tabViewItems.contains(where: { $0.identifier as? String == "photos" }),
              MediaSetting.allCases.filter(\.isPhotos).allSatisfy({ rows[$0] != nil }) else {
            throw HostError(message: "Photos import settings disappeared outside Fast IMG")
        }
        for field in [MediaSetting.photosAdaptive, .photosPreserveTree] {
            guard toggles[field]?.title == field.title,
                  rows[field]?.cell(at: 0).contentView === toggles[field] else {
                throw HostError(message: "Photos checkbox lost its visible inline title")
            }
        }
        if !fast {
            guard tabs.tabViewItems.contains(where: { $0.identifier as? String == "vid" }),
                  popups[.vidCodec] != nil,
                  !developer || (textFields[.vidConfig] != nil && popups[.vidErrorMode] != nil) else {
                throw HostError(message: "Video configuration controls are missing")
            }
        }
    }

    private func validateSearchForSelfTest() throws {
        let original = draft()
        let wasAdvanced = advanced.state
        defer {
            search.stringValue = ""
            advanced.state = wasAdvanced
            restore(original)
        }
        var settings = MediaSettings()
        settings.values = [.photosBackend: "native", .photosAdaptive: "true", .photosAlbum: "Unsaved album"]
        restore(settings)
        advanced.state = .off
        updateVisibility()
        let before = draft().values
        search.stringValue = MediaSetting.photosVerificationBatch.title
        controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: search))
        guard searchMatches.contains(.setting(.photosVerificationBatch)), !searchResults.isHidden,
              applyButton.keyEquivalent.isEmpty, draft().values == before else {
            throw HostError(message: "Settings search lost an advanced field or modified the draft")
        }
        guard let index = searchMatches.firstIndex(of: .setting(.photosVerificationBatch)) else {
            throw HostError(message: "Settings search did not find the verification batch size")
        }
        searchTable.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        let editor = NSTextView()
        guard control(search, textView: editor, doCommandBy: #selector(NSResponder.insertNewline(_:))),
              tabs.selectedTabViewItem?.identifier as? String == "photos",
              rows[.photosVerificationBatch]?.isHidden == false,
              search.stringValue.isEmpty, draft().values == before else {
            throw HostError(message: "Search navigation changed settings or failed to reveal the destination")
        }
        search.stringValue = "no-such-setting-948217"
        updateSearch()
        guard searchMatches.isEmpty, !searchEmpty.isHidden,
              control(search, textView: editor, doCommandBy: #selector(NSResponder.insertNewline(_:))),
              !applying, draft().values == before else {
            throw HostError(message: "Return in empty search results attempted to apply settings")
        }
        guard control(search, textView: editor, doCommandBy: #selector(NSResponder.cancelOperation(_:))),
              search.stringValue.isEmpty, searchEmpty.isHidden, draft().values == before else {
            throw HostError(message: "Search cancellation changed the settings draft")
        }
        guard !matchingSettings(MediaSetting.photosAppleScriptBatch.title).contains(.setting(.photosAppleScriptBatch)),
              matchingSettings("fail-fast").contains(.setting(.vidErrorMode)) == (developer && !fast),
              matchingSettings("vid.codec").contains(.setting(.vidCodec)) == (developer && !fast),
              matchingSettings(localized("settings.cache.clear")).contains(.cache) else {
            throw HostError(message: "Search exposed unavailable settings or missed cache management")
        }
        search.stringValue = localized("settings.section.photos")
        updateSearch()
        guard searchMatches.count > 1 else { throw HostError(message: "Section search missed its controls") }
        _ = control(search, textView: editor, doCommandBy: #selector(NSResponder.moveDown(_:)))
        guard searchTable.selectedRow == 1 else { throw HostError(message: "Settings search keyboard navigation failed") }
        panel.contentView?.layoutSubtreeIfNeeded()
        if let surface = panel.contentView {
            guard surface.bounds.contains(surface.convert(searchResults.bounds, from: searchResults)),
                  searchResults.frame.maxY <= search.frame.minY || searchResults.frame.minY >= search.frame.maxY else {
                throw HostError(message: "Settings search results overlap or escape the panel")
            }
        }
        settings.values[.photosBackend] = "applescript"
        restore(settings)
        guard !matchingSettings(MediaSetting.photosNativeBatch.title).contains(.setting(.photosNativeBatch)),
              matchingSettings(MediaSetting.photosAppleScriptBatch.title).contains(.setting(.photosAppleScriptBatch)) else {
            throw HostError(message: "Settings search kept stale backend-specific results")
        }
    }

    func validateCachePolicyForSelfTest() throws {
        let previous = restored
        let previousInherited = inherited
        let previousSources = inheritedSources
        let previousMixed = inheritedMixed
        defer {
            inherited = previousInherited
            inheritedSources = previousSources
            inheritedMixed = previousMixed
            restore(previous)
        }
        let key = "cache.path_tree_max_bytes"
        let ttl = "cache.path_tree_ttl_seconds"
        let standard = EffectiveRuntimeSettings(values: [key: "1024", ttl: "60"], sources: [:], sourceChain: [:])
        let fastValues = EffectiveRuntimeSettings(values: [key: "2048", ttl: "60"], sources: [:], sourceChain: [:])
        let video = EffectiveRuntimeSettings(values: [key: "4096", ttl: "60"], sources: [:], sourceChain: [:])
        setInheritedValues(standard: standard, fast: fastValues, video: video)
        restore(MediaSettings())
        tabs.selectTabViewItem(withIdentifier: "cache")
        guard textFields[.cacheMaxBytes] != nil, textFields[.cacheTtlSeconds]?.stringValue == "60",
              steppers[.cacheMaxBytes] == nil, steppers[.cacheTtlSeconds] == nil,
              draft().values.isEmpty, !applyButton.isHidden, !resetButton.isHidden else {
            throw HostError(message: "Cache controls lost inherited values or created overrides")
        }
        if !fast && !videos {
            guard inheritedMixed.contains(.cacheMaxBytes), textFields[.cacheMaxBytes]?.stringValue.isEmpty == true else {
                throw HostError(message: "Different pipeline cache limits became a shared default")
            }
        } else if textFields[.cacheMaxBytes]?.stringValue != (fast ? "2048" : "4096") {
            throw HostError(message: "Cache inheritance used another pipeline")
        }
        textFields[.cacheMaxBytes]?.stringValue = String(Int.max)
        textFields[.cacheTtlSeconds]?.stringValue = "1"
        let explicit = draft()
        try explicit.save(to: preferences)
        guard MediaSettings(preferences: preferences).cacheMaxBytes == Int.max,
              explicit.cacheTtlSeconds == 1 else {
            throw HostError(message: "Cache settings lost integer precision or persistence")
        }
        restore(explicit)
        textFields[.cacheMaxBytes]?.stringValue = ""
        guard draft().cacheMaxBytes == nil else { throw HostError(message: "Clearing cache limit kept an override") }
        for value in ["0", "-1", "1.5", "9223372036854775808", "bad"] {
            textFields[.cacheMaxBytes]?.stringValue = value
            do { try draft().validate() }
            catch { continue }
            throw HostError(message: "Invalid cache limit accepted: \(value)")
        }
        var independent = explicit
        independent.values[.vidCodec] = "av1"
        restore(independent)
        resetTab()
        guard draft().values == [.vidCodec: "av1"] else {
            throw HostError(message: "Cache reset retained overrides or changed another section")
        }
        try previous.save(to: preferences)
    }

    private func validateCacheForSelfTest() throws {
        let document: [String: Any] = ["schema_version": 1, "cache_directory": "/synthetic/cache",
            "store_bytes": 1024, "legacy_analysis_bytes": 64, "legacy_analysis_files": 1,
            "removed_rows": 0, "removed_files": 0, "namespaces": [
                ["name": "path_tree", "rows": 3, "payload_bytes": 512, "rebuildable": true],
                ["name": "checkpoint", "rows": 2, "payload_bytes": 128, "rebuildable": false],
                ["name": "future_state", "rows": 4, "payload_bytes": 256, "rebuildable": false]]]
        let snapshot = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: document))
        let database: [String: Any] = ["store_schema_version": 2, "sqlite_version": "3.53.0",
            "journal_mode": "wal", "page_size": 4096, "page_count": 12, "free_pages": 3]
        var withDatabase = document
        withDatabase["database"] = database
        let databaseSnapshot = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: withDatabase))
        guard matchingSettings(localized("settings.cache.backup")).contains(.cache) else {
            throw HostError(message: "Database backup was omitted from Settings search")
        }
        guard matchingSettings(localized("settings.history.backup")).contains(.cache),
              historyBackup.toolTip == localized("settings.history.backup_help") else {
            throw HostError(message: "History backup is missing from Settings search or accessibility context")
        }
        try displayCache(databaseSnapshot)
        guard cacheBackup.isEnabled, cacheStore?.schema == 2,
              cacheLabels["journal"]?.stringValue == "WAL",
              cacheLabels["pages"]?.stringValue == localized("settings.cache.pages_value", "12", "3", "4096") else {
            throw HostError(message: "Database overview or backup availability lost its recorded values")
        }
        for (key, value) in [("store_schema_version", 0 as Any), ("journal_mode", "unknown"),
                             ("page_size", 513), ("page_count", 0), ("free_pages", 13),
                             ("page_count", UInt64.max)] {
            var invalidDatabase = database
            invalidDatabase[key] = value
            withDatabase["database"] = invalidDatabase
            do { _ = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: withDatabase)) }
            catch { continue }
            throw HostError(message: "Invalid database overview was accepted: \(key)")
        }
        let backupDirectory = FileManager.default.temporaryDirectory.appendingPathComponent("mfb-backup-receipt-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: backupDirectory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: backupDirectory) }
        let destination = backupDirectory.appendingPathComponent("snapshot.sqlite")
        try Data([0, 1, 2, 3]).write(to: destination)
        let source = URL(fileURLWithPath: "/synthetic/cache/mfb_store.sqlite")
        let backup: [String: Any] = ["schema_version": 1, "source": source.path,
            "destination": destination.resolvingSymlinksInPath().path, "store_schema_version": 2, "bytes": 4]
        guard try CoreDatabaseBackup.decode(JSONSerialization.data(withJSONObject: backup), source: source,
            destination: destination, storeSchemaVersion: 2).bytes == 4 else {
            throw HostError(message: "Valid backup receipt was lost")
        }
        for (key, value) in [("schema_version", 2 as Any), ("source", "/synthetic/other.sqlite"),
                             ("destination", "/synthetic/other.sqlite"), ("store_schema_version", 3), ("bytes", 0), ("bytes", 5)] {
            var invalid = backup
            invalid[key] = value
            do { _ = try CoreDatabaseBackup.decode(JSONSerialization.data(withJSONObject: invalid), source: source,
                destination: destination, storeSchemaVersion: 2) }
            catch { continue }
            throw HostError(message: "Contradictory backup receipt was accepted: \(key)")
        }
        let link = backupDirectory.appendingPathComponent("link.sqlite")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: destination)
        do { _ = try CoreDatabaseBackup.decode(JSONSerialization.data(withJSONObject: backup), source: source,
            destination: link, storeSchemaVersion: 2)
            throw HostError(message: "Symlinked backup was accepted")
        } catch let error as HostError where error.message == "Symlinked backup was accepted" { throw error }
          catch { }
        let historySource = URL(fileURLWithPath: "/synthetic/history/history.sqlite3")
        var historyReceipt: [String: Any] = ["schema_version": 1, "kind": "history_sqlite", "source": historySource.path,
            "destination": destination.resolvingSymlinksInPath().path, "history_schema_version": 2, "bytes": 4,
            "sessions": 2, "events": 3, "output_rows": 5]
        let historyData = try JSONSerialization.data(withJSONObject: historyReceipt)
        let history = try HistoryDatabaseBackup.decode(historyData, source: historySource, destination: destination)
        guard history.sessions == 2, history.events == 3, history.outputRows == 5,
              historyBackupArguments(directory: historySource.deletingLastPathComponent(), destination: destination)
                == ["--history-backup", destination.path, "--history-directory", "/synthetic/history", "--json"] else {
            throw HostError(message: "History backup receipt or explicit source arguments lost their scope")
        }
        for (key, value) in [("kind", "core_sqlite" as Any), ("schema_version", 2),
                             ("history_schema_version", 0), ("history_schema_version", 1),
                             ("source", "/synthetic/other.sqlite"), ("destination", "/synthetic/other.sqlite"),
                             ("bytes", 5), ("sessions", UInt64.max), ("events", -1), ("output_rows", NSNull())] {
            var invalid = historyReceipt
            invalid[key] = value
            do { _ = try HistoryDatabaseBackup.decode(JSONSerialization.data(withJSONObject: invalid), source: historySource, destination: destination) }
            catch { continue }
            throw HostError(message: "Invalid History backup evidence was accepted: \(key)")
        }
        do {
            _ = try HistoryDatabaseBackup.decode(historyData, source: historySource, destination: link)
            throw HostError(message: "History backup destination symlink was accepted")
        } catch let error as HostError where error.message == "History backup destination symlink was accepted" { throw error }
          catch { }
        historyReceipt["history_schema_version"] = 1
        historyReceipt["output_rows"] = 0
        guard try HistoryDatabaseBackup.decode(JSONSerialization.data(withJSONObject: historyReceipt), source: historySource, destination: destination).historySchemaVersion == 1 else {
            throw HostError(message: "Version-one History backup was incorrectly migrated or rejected")
        }
        guard try snapshot.totals(rebuildable: true).rows == 3,
              try snapshot.totals(rebuildable: false).rows == 6 else {
            throw HostError(message: "Cache and retained state counts were mixed")
        }
        try displayCache(snapshot)
        guard !cacheBackup.isEnabled, cacheLabels["database"]?.stringValue == localized("result.unknown") else {
            throw HostError(message: "Missing database evidence was guessed or allowed a backup")
        }
        tabs.selectTabViewItem(withIdentifier: "cache")
        guard cacheClear.isEnabled, cachePrune.isEnabled, !resetButton.isHidden, !applyButton.isHidden,
              cacheCheck.isEnabled, cacheLabels["path"]?.stringValue == snapshot.cacheDirectory else {
            throw HostError(message: "Cache controls were not placed in Settings")
        }
        cacheBusy = true
        try displayCache(databaseSnapshot)
        updatePanelSize()
        guard !cacheClear.isEnabled, !cachePrune.isEnabled, !cacheCheck.isEnabled, !cacheBackup.isEnabled,
              !historyBackup.isEnabled, !cancelButton.isEnabled else {
            throw HostError(message: "Cache commands stayed enabled while busy")
        }
        cacheBusy = false
        try displayCache(snapshot)
        let policy = EffectiveRuntimeSettings(values: ["cache.path_tree_max_bytes": "4096", "cache.path_tree_ttl_seconds": "60"],
            sources: [:], sourceChain: [:])
        guard try cachePruningArguments([policy, policy]) == ["--no-config", "--cache-max-bytes", "4096", "--cache-ttl-seconds", "60"] else {
            throw HostError(message: "Cache pruning did not pin the resolved policy")
        }
        for values in [[String: String](), ["cache.path_tree_max_bytes": "0", "cache.path_tree_ttl_seconds": "60"],
                       ["cache.path_tree_max_bytes": "4096", "cache.path_tree_ttl_seconds": "61"],
                       ["cache.path_tree_max_bytes": "18446744073709551615", "cache.path_tree_ttl_seconds": "60"]] {
            let other = EffectiveRuntimeSettings(values: values, sources: [:], sourceChain: [:])
            do { _ = try cachePruningArguments([policy, other]) }
            catch { continue }
            throw HostError(message: "Missing, mixed or invalid pruning policy was accepted")
        }
        var pruned = document
        pruned["removed_rows"] = 3
        let pruning: [String: Any] = ["policy": ["path_tree_max_bytes": 4096, "path_tree_ttl_seconds": 60],
            "expired_rows": 1, "evicted_rows": 2]
        pruned["pruning"] = pruning
        guard try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: pruned)).pruning?.expiredRows == 1 else {
            throw HostError(message: "Pruning receipt lost category counts")
        }
        for key in ["removed_rows", "removed_files"] {
            var invalid = pruned
            invalid[key] = 1
            do { _ = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: invalid)) }
            catch { continue }
            throw HostError(message: "Contradictory pruning receipt was accepted")
        }
        cacheLabels["path"]?.stringValue = "/synthetic/" + String(repeating: "long-directory/", count: 30)
        cacheMessage.stringValue = String(repeating: "Synthetic cache error. ", count: 30)
        cacheMessageRow?.isHidden = false
        updatePanelSize()
        panel.contentView?.layoutSubtreeIfNeeded()
        guard let content = tabs.selectedTabViewItem?.view, let grid = grids["cache"],
              content.bounds.contains(grid.frame),
              cacheLabels.values.allSatisfy({ grid.bounds.contains($0.alignmentRect(forFrame: $0.frame)) }) else {
            let labels = cacheLabels.map { "\($0.key)=\($0.value.frame)" }.joined(separator: ", ")
            throw HostError(message: "Long cache paths or errors escaped the Settings tab: content=\(String(describing: tabs.selectedTabViewItem?.view?.bounds)), grid=\(String(describing: grids["cache"]?.frame)), labels=\(labels)")
        }
        cacheMessageRow?.isHidden = true
        try displayCache(snapshot)
        let healthy: [String: Any] = ["schema_version": 1, "status": "healthy",
            "sqlite_integrity": "ok", "foreign_keys": "ok", "blob_scan_complete": true,
            "checked_cache_rows": 3, "checked_protected_rows": 6,
            "corrupt_cache_rows": 0, "corrupt_protected_rows": 0, "issue_count": 0, "issues": []]
        var checked = document
        checked["integrity"] = healthy
        guard try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: checked)).integrity?.status == .healthy else {
            throw HostError(message: "Database integrity result was not decoded")
        }
        for (key, value) in [("schema_version", 2 as Any), ("status", "unknown"),
                             ("checked_cache_rows", 4), ("blob_scan_complete", false),
                             ("corrupt_protected_rows", 1), ("issue_count", 1)] {
            var report = healthy
            report[key] = value
            checked["integrity"] = report
            do { _ = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: checked)) }
            catch { continue }
            throw HostError(message: "Invalid database integrity receipt was accepted: \(key)")
        }
        for state in ["unhealthy", "error"] {
            var report = healthy
            report["status"] = state
            report["corrupt_protected_rows"] = 1
            report["issue_count"] = 1
            report["issues"] = [["code": "protected_payload", "message": "Invalid digest."]]
            checked["integrity"] = report
            let decoded = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: checked))
            guard let integrity = decoded.integrity, integrity.status.rawValue == state else { throw HostError(message: "Integrity failure was lost") }
            displayCacheIntegrity(integrity)
            guard cacheMessage.textColor == .systemRed, cacheMessageRow?.isHidden == false,
                  state != "error" || (!cacheClear.isEnabled && !cachePrune.isEnabled && !cacheLoaded) else {
                throw HostError(message: "Incomplete integrity check was shown as available or successful")
            }
        }
        var absent = healthy
        absent["status"] = "absent"
        absent["sqlite_integrity"] = "not_checked"
        absent["foreign_keys"] = "not_checked"
        absent["blob_scan_complete"] = false
        absent["checked_cache_rows"] = 0
        absent["checked_protected_rows"] = 0
        checked["integrity"] = absent
        checked["namespaces"] = []
        guard try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: checked)).integrity?.status == .absent else {
            throw HostError(message: "Missing database was not distinguished from healthy")
        }
        cacheMessageRow?.isHidden = true
        try displayCache(snapshot)
        for version in [0, 2] {
            var invalid = document
            invalid["schema_version"] = version
            do {
                _ = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: invalid))
            } catch { continue }
            throw HostError(message: "Unsupported cache status schema was accepted")
        }
        var invalid = document
        invalid["namespaces"] = [["name": "checkpoint", "rows": 2, "payload_bytes": 128, "rebuildable": true]]
        do {
            _ = try LocalCacheStatus.decode(JSONSerialization.data(withJSONObject: invalid))
        } catch { return }
        throw HostError(message: "Recovery state was mislabeled as rebuildable cache")
    }
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
        arguments += try request.mediaSettings.arguments(operation: request.operationMode, processing: request.processingMode)
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

private func countStatusValue(in line: String) -> String? {
    guard let range = line.range(of: #"^\s*(?:ERR:\s*)?Count status:\s*\S+"#, options: [.regularExpression, .caseInsensitive]) else {
        return nil
    }
    return line[range].split(separator: ":").last?
        .trimmingCharacters(in: .whitespaces)
}

private struct FileStageProgress {
    struct Event: Decodable {
        let schema_version: Int
        let stage_id: String
        let stage: String
        let processed: UInt64
        let total: UInt64
        let state: String

        var valid: Bool {
            schema_version == 1 && !stage_id.isEmpty && stage_id.utf8.count <= 64
                && ["image_processing", "video_processing", "fast_img_encode"].contains(stage)
                && ["running", "finished"].contains(state)
                && processed <= total && total <= UInt64(Int64.max)
        }

        var percentage: Double? {
            guard total > 0 else { return nil }
            // Do not round an unfinished stage up to 100.00%.
            return processed == total ? 100 : min(99.99, Double(processed) / Double(total) * 100)
        }
    }

    private(set) var event: Event?
    private(set) var invalid = false

    static func payload(_ line: String) -> Substring? {
        let raw = line.hasPrefix("ERR: ") ? line.dropFirst(5) : line[...]
        let prefix = "MFB_PROGRESS="
        return raw.hasPrefix(prefix) ? raw.dropFirst(prefix.count) : nil
    }

    mutating func ingest(_ line: String) -> Bool {
        guard let payload = Self.payload(line) else { return false }
        guard payload.utf8.count <= 1_024,
              let next = try? JSONDecoder().decode(Event.self, from: Data(payload.utf8)), next.valid else {
            event = nil
            invalid = true
            return true
        }
        if let previous = event, previous.stage_id == next.stage_id {
            guard previous.stage == next.stage, previous.total == next.total,
                  next.processed >= previous.processed,
                  previous.state != "finished" || next.state == "finished" else {
                event = nil
                invalid = true
                return true
            }
        }
        event = next
        invalid = false
        return true
    }

    var label: String {
        guard let event else { return localized("progress.unavailable") }
        let stage = localized("progress.stage.\(event.stage)")
        if event.state == "finished" {
            return localized("progress.stage_finished", stage, String(event.processed), String(event.total))
        }
        guard let percentage = event.percentage else { return localized("progress.empty", stage) }
        return localized("progress.files", stage, String(event.processed), String(event.total), percentage)
    }
}

struct GateCheckActivity {
    struct Event: Decodable, Equatable {
        let schema_version: Int
        let stage_id: String
        let gate: String
        let check: String
        let completed: UInt64
        let total: UInt64
        let state: String

        var valid: Bool {
            guard schema_version == 1, !stage_id.isEmpty, stage_id.utf8.count <= 64,
                  gate == "local", total == 6 else { return false }
            if check == "inventory" { return completed == 0 && ["running", "failed"].contains(state) }
            guard let index = ["count", "size", "blake3", "metadata", "orient", "decode"].firstIndex(of: check) else { return false }
            switch state {
            case "running": return completed == UInt64(index)
            case "failed": return completed == UInt64(index + 1)
            case "passed": return check == "decode" && completed == total
            default: return false
            }
        }
    }

    private(set) var event: Event?
    private(set) var invalid = false
    private var updatedAt: TimeInterval?

    mutating func ingest(_ line: String, now: TimeInterval) -> Bool {
        let raw = line.hasPrefix("ERR: ") ? line.dropFirst(5) : line[...]
        let prefix = "MFB_GATE_ACTIVITY="
        guard raw.hasPrefix(prefix) else { return false }
        let payload = raw.dropFirst(prefix.count)
        guard now.isFinite, payload.utf8.count <= 1_024,
              let next = try? JSONDecoder().decode(Event.self, from: Data(payload.utf8)), next.valid else {
            invalidate()
            return true
        }
        if let previous = event, previous.stage_id == next.stage_id {
            guard next.completed >= previous.completed,
                  previous.state == "running" || previous == next,
                  next.completed != previous.completed || previous == next
                    || (previous.check == "inventory" && next.check == "count" && next.state == "running")
                    || (previous.check == "inventory" && next.check == "inventory" && next.state == "failed") else {
                invalidate()
                return true
            }
        }
        event = next
        updatedAt = now
        invalid = false
        return true
    }

    private mutating func invalidate() {
        event = nil
        updatedAt = nil
        invalid = true
    }

    func label(now: TimeInterval) -> String {
        guard let event else { return invalid ? localized("gate.activity.invalid") : "" }
        let text: String
        switch event.state {
        case "passed": text = localized("gate.activity.passed", String(event.completed), String(event.total))
        case "failed": text = localized("gate.activity.failed", localized("gate.check.\(event.check)"),
                                        String(event.completed), String(event.total), String(event.total - event.completed))
        default: text = localized("gate.activity.running", localized("gate.check.\(event.check)"),
                                  String(event.completed), String(event.total))
        }
        guard event.state == "running", let updatedAt, now.isFinite else { return text }
        let age = UInt64(min(Double(Int64.max), max(0, now - updatedAt)))
        return text + " · " + localized("gate.activity.updated", String(age))
    }
}

struct PhaseLogPresentation {
    enum Phase: String, Decodable { case processing, verification }
    enum Update: Equatable {
        case unchanged, invalid
        case changed(Phase, replace: Bool)
    }
    private struct Event: Decodable {
        let schema_version: Int
        let phase: Phase
    }
    private(set) var phase: Phase?
    private var lastReplacement: TimeInterval

    init(now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        lastReplacement = now
    }

    mutating func ingest(_ line: String, now: TimeInterval) -> Update? {
        let raw = line.hasPrefix("ERR: ") ? line.dropFirst(5) : line[...]
        let prefix = "MFB_LOG_PHASE="
        guard raw.hasPrefix(prefix) else { return nil }
        let payload = raw.dropFirst(prefix.count)
        guard payload.utf8.count <= 512,
              let next = try? JSONDecoder().decode(Event.self, from: Data(payload.utf8)),
              next.schema_version == 1 else { return .invalid }
        guard next.phase != phase else { return .unchanged }
        phase = next.phase
        // Merge short phases instead of flashing away output the user has just seen.
        let replace = now - lastReplacement >= 2
        if replace { lastReplacement = now }
        return .changed(next.phase, replace: replace)
    }
}

func processingCountTotal(_ values: [Int?]) -> Int? {
    guard !values.isEmpty else { return nil }
    var total = 0
    for value in values {
        guard let value, value >= 0 else { return nil }
        let sum = total.addingReportingOverflow(value)
        guard !sum.overflow else { return nil }
        total = sum.partialValue
    }
    return total
}

func hasPartialProcessingSuccess(succeeded: [Int?], failed: [Int?], countsComplete: Bool) -> Bool {
    guard countsComplete, succeeded.count == failed.count,
          let successes = processingCountTotal(succeeded), let failures = processingCountTotal(failed),
          !successes.addingReportingOverflow(failures).overflow else { return false }
    return successes > 0 && failures > 0
}

private struct BatchResults {
    struct Event: Decodable {
        let schemaVersion: Int
        let media: String
        let succeeded: Int?
        let skipped: Int?
        let failed: Int?
        let ignored: Int?
        let unprocessed: Int?
        let exitCode: Int?

        var counts: [Int?] { [succeeded, skipped, failed, ignored] }
        var countsAreValid: Bool {
            var total = 0
            for value in counts + [unprocessed] {
                guard let value else { continue }
                let sum = total.addingReportingOverflow(value)
                guard value >= 0, !sum.overflow else { return false }
                total = sum.partialValue
            }
            return true
        }
    }

    private(set) var totals: [String: [Int?]] = [:]
    private(set) var pendingTotals: [String: Int?] = [:]
    private(set) var hasFailure = false
    private(set) var hasFileFailures = false
    private(set) var hasUnprocessed = false
    private(set) var pendingTotalOverflowed = false
    private(set) var invalid = false
    var isIncomplete: Bool {
        if invalid || pendingTotalOverflowed || totals.values.contains(where: { $0.contains(where: { $0 == nil }) }) {
            return true
        }
        var total = 0
        for value in totals.values.flatMap({ $0 }) + Array(pendingTotals.values) {
            guard let value else { continue }
            let sum = total.addingReportingOverflow(value)
            if sum.overflow { return true }
            total = sum.partialValue
        }
        return false
    }

    var hasPartialSuccess: Bool {
        hasPartialProcessingSuccess(succeeded: totals.values.map { $0[0] },
                                    failed: totals.values.map { $0[2] },
                                    countsComplete: !isIncomplete && !hasUnprocessed
                                        && pendingTotals.count == totals.count
                                        && pendingTotals.values.allSatisfy { $0 == 0 })
    }

    var summary: String? {
        guard !totals.isEmpty else { return nil }
        let count = { (value: Int?) in value.map(String.init) ?? localized("result.unknown") }
        let counts = (0..<4).map { field in
            count(invalid ? nil : processingCountTotal(totals.values.map { $0[field] }))
        }
        return localized("result.counts", counts[0], counts[1], counts[2], counts[3])
            + " · " + localized("result.unprocessed", count(invalid ? nil
                : processingCountTotal(Array(pendingTotals.values))))
    }

    mutating func ingest(_ line: String) -> String? {
        let raw = line.hasPrefix("ERR: ") ? String(line.dropFirst(5)) : line
        let prefix = "MFB_BATCH_RESULT="
        guard raw.hasPrefix(prefix) else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let event = try? decoder.decode(Event.self, from: Data(raw.dropFirst(prefix.count).utf8)),
              event.schemaVersion == 1, ["img", "vid"].contains(event.media),
              event.countsAreValid else {
            invalid = true
            for media in Array(pendingTotals.keys) {
                pendingTotals.updateValue(nil, forKey: media)
            }
            return "[WARN] \(localized("result.invalid"))"
        }
        hasFailure = hasFailure || (event.exitCode.map { $0 != 0 } ?? false) || (event.failed ?? 0) > 0
        hasFileFailures = hasFileFailures || (event.failed ?? 0) > 0
        if event.exitCode == nil { invalid = true }
        let previous = totals[event.media] ?? [0, 0, 0, 0]
        totals[event.media] = zip(previous, event.counts).map { left, right in
            guard let left, let right else { return nil }
            let sum = left.addingReportingOverflow(right)
            return sum.overflow ? nil : sum.partialValue
        }
        if let unprocessed = event.unprocessed, unprocessed > 0 { hasUnprocessed = true }
        if invalid {
            pendingTotals.updateValue(nil, forKey: event.media)
        } else if pendingTotals.keys.contains(event.media) {
            let previous = pendingTotals[event.media] ?? nil
            if let previousValue = previous, let unprocessed = event.unprocessed {
                let sum = previousValue.addingReportingOverflow(unprocessed)
                if sum.overflow { pendingTotalOverflowed = true }
                pendingTotals.updateValue(sum.overflow ? nil : sum.partialValue, forKey: event.media)
            } else {
                pendingTotals.updateValue(nil, forKey: event.media)
            }
        } else {
            pendingTotals.updateValue(event.unprocessed, forKey: event.media)
        }
        let count = { (value: Int?) in value.map(String.init) ?? localized("result.unknown") }
        let summary = localized("result.counts", count(event.succeeded), count(event.skipped),
                                count(event.failed), count(event.ignored))
            + " · " + localized("result.unprocessed", count(event.unprocessed))
        let tag = (event.exitCode.map { $0 != 0 } ?? false) || (event.failed ?? 0) > 0 ? "FAIL"
            : (event.exitCode == nil || event.counts.contains { $0 == nil } || (event.unprocessed ?? 0) > 0 ? "WARN" : "SUMMARY")
        return "[\(tag)] \(event.media.uppercased()): \(summary) (exit=\(count(event.exitCode)))"
    }
}

private struct PhotosDiagnostics {
    struct Phase: Decodable {
        let calls: Int
        let seconds: Double
    }

    struct Profile: Decodable {
        let schemaVersion: Int
        let backend: String
        let succeeded: Bool
        let committedAssets: Int?
        let verifiedAssets: Int?
        let peakVerificationBacklog: Int?
        let totalSeconds: Double?
        let verifiedAssetsPerSecond: Double?
        let transactionSamples: Int?
        let transactionMeanSeconds: Double?
        let transactionP50Seconds: Double?
        let transactionP90Seconds: Double?
        let transactionP95Seconds: Double?
        let transactionP99Seconds: Double?
        let importBatchSize: Int?
        let verificationBatchSize: Int?
        let helperPeakRssBytes: Int?
        let phases: [String: Phase]?
    }

    private(set) var profile: Profile?
    private(set) var backend: String?
    private(set) var committedAssets: Int?
    private(set) var verifiedAssets: Int?
    private(set) var peakBacklog: Int?
    private(set) var failed = false

    var hasMeasurements: Bool { profile != nil || backend != nil }

    mutating func reset() { self = PhotosDiagnostics() }

    mutating func markFailed() {
        if hasMeasurements { failed = true }
    }

    @discardableResult
    mutating func ingest(_ line: String) -> Bool {
        let raw = line.hasPrefix("ERR: ") ? String(line.dropFirst(5)) : line
        let profilePrefix = "[PHOTOS PROFILE] "
        if raw.hasPrefix(profilePrefix) {
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            guard let decoded = try? decoder.decode(Profile.self, from: Data(raw.dropFirst(profilePrefix.count).utf8)),
                  decoded.schemaVersion == 1 else { return false }
            if backend != decoded.backend { reset() }
            profile = decoded
            backend = decoded.backend
            committedAssets = decoded.committedAssets ?? committedAssets
            verifiedAssets = decoded.verifiedAssets ?? verifiedAssets
            peakBacklog = decoded.peakVerificationBacklog ?? peakBacklog
            failed = !decoded.succeeded
            return true
        }
        let progressPrefix = "[PHOTOS PROGRESS] "
        guard raw.hasPrefix(progressPrefix) else { return false }
        let fields = raw.dropFirst(progressPrefix.count).split(separator: " ").reduce(into: [String: String]()) { result, item in
            let pair = item.split(separator: "=", maxSplits: 1)
            if pair.count == 2 { result[String(pair[0])] = String(pair[1]) }
        }
        guard let newBackend = fields["backend"] else { return false }
        if backend != newBackend || profile != nil { reset() }
        backend = newBackend
        if newBackend == "native", let value = fields["committed"].flatMap(Int.init), value >= 0 {
            committedAssets = value
        }
        if let value = fields["verified"].flatMap(Int.init), value >= 0 { verifiedAssets = value }
        if newBackend == "native", let value = fields["peak_backlog"].flatMap(Int.init), value >= 0 {
            peakBacklog = value
        }
        return true
    }

    func rendered() -> String {
        let unknown = localized("diagnostics.unknown")
        func count(_ value: Int?) -> String {
            guard let value, value >= 0 else { return unknown }
            return String(value)
        }
        func seconds(_ value: Double?) -> String {
            guard let value, value.isFinite, value >= 0 else { return unknown }
            return String(format: "%.2f s", value)
        }
        func rate(_ value: Double?) -> String {
            guard let value, value.isFinite, value >= 0 else { return unknown }
            return String(format: "%.2f %@", value, localized("diagnostics.assets_per_second"))
        }
        func row(_ key: String, _ value: String) -> String { "\(localized("diagnostics.\(key)")): \(value)" }
        let status = failed ? localized("diagnostics.failed")
            : profile?.succeeded == true ? localized("diagnostics.completed")
            : hasMeasurements ? localized("diagnostics.in_progress") : unknown
        let backendName = backend.map {
            ["native": localized("diagnostics.backend.native"),
             "applescript": localized("diagnostics.backend.applescript")][$0] ?? $0
        } ?? unknown
        let backlog: Int? = {
            guard backend == "native", let committedAssets, let verifiedAssets, verifiedAssets >= 0,
                  committedAssets >= verifiedAssets else { return nil }
            return committedAssets - verifiedAssets
        }()
        var rows = [
            row("status", status), row("backend", backendName),
            row("import_batch", count(profile?.importBatchSize)),
            row("verification_batch", count(profile?.verificationBatchSize)),
            row("transactions", count(profile?.transactionSamples)),
            row("committed", count(committedAssets)), row("verified", count(verifiedAssets)),
            row("throughput", rate(profile?.verifiedAssetsPerSecond)),
            row("total_time", seconds(profile?.totalSeconds)),
            row("average_transaction", seconds(profile?.transactionMeanSeconds)),
            row("backlog", count(backlog)), row("peak_backlog", count(peakBacklog)),
            row("p50", seconds(profile?.transactionP50Seconds)),
            row("p90", seconds(profile?.transactionP90Seconds)),
            row("p95", seconds(profile?.transactionP95Seconds)),
            row("p99", seconds(profile?.transactionP99Seconds)),
            row("rss", profile?.helperPeakRssBytes.flatMap {
                $0 >= 0 ? ByteCountFormatter.string(fromByteCount: Int64($0), countStyle: .memory) : nil
            } ?? unknown),
            "", localized("diagnostics.phases"),
        ]
        if let phases = profile?.phases, !phases.isEmpty {
            rows += phases.keys.sorted().compactMap { name in
                guard let phase = phases[name] else { return nil }
                return "\(name): \(count(phase.calls)) · \(seconds(phase.seconds))"
            }
        } else {
            rows.append(unknown)
        }
        return rows.joined(separator: "\n")
    }
}

enum LogTone: Equatable {
    case muted, normal, stage, result, warning, failure

    static func message(_ line: String) -> String {
        var text = line.replacingOccurrences(of: #"\x1B\[[0-?]*[ -/]*[@-~]"#,
                                             with: "", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("ERR:") { text = String(text.dropFirst(4)).trimmingCharacters(in: .whitespaces) }
        return text
    }

    static func classify(_ line: String) -> Self {
        let line = message(line)
        if line.hasPrefix(HistoryFileFailure.prefix) { return .muted }
        // The stderr transport prefix is not evidence of an error.
        if let count = countStatusValue(in: line) { return count.uppercased() == "MATCH" ? .result : .failure }
        if line.range(of: #"(?i)(?:\[(?:ERROR|FATAL|FAIL(?:ED)?)\s*\]|✗|^\s*(?:Error|Failed|Fatal|错误|失败|エラー|失敗):|^\s*(?:Permission denied|No space left on device|exiting with failures)\b|^\s*\[Summary\].*\bfailed=[1-9]\d*(?:\s|$)|^\s*Integrity Issues:\s*[1-9]\d*|^\s*Integrity:(?!\s*CLEAN\b)|^\s*\[GATE\s*\d+\s*\].*\bFAIL\b)"#, options: .regularExpression) != nil {
            return .failure
        }
        if line.range(of: #"(?i)(?:\[WARN(?:ING)?\s*\]|⚠|^\s*(?:warning|警告):)"#, options: .regularExpression) != nil { return .warning }
        if line.range(of: #"(?i)(?:\[(?:DONE|SUMMARY|SUCCESS|OK)\]|^\s*(?:Success rate:|Integrity:|Integrity Issues:|Total time:)|^\s*✓)"#, options: .regularExpression) != nil { return .result }
        if line.range(of: #"(?i)^\s*(?:ERR:\s*)?(?:#|\[(?:SCAN|COPY|ENCODE|VERIFY|CHECK|IMPORT|SKIP|RETAIN|RESTORE|RESUME|FINAL|STATS|ARCHIVE|PROGRESS|PHASE)\s*\])"#, options: .regularExpression) != nil { return .stage }
        if line.range(of: #"(?i)^\s*(?:INF|INFO|DBG|DEBUG|TRACE)\b"#, options: .regularExpression) != nil { return .muted }
        return .normal
    }

    var color: NSColor {
        switch self {
        case .muted: .tertiaryLabelColor
        case .normal: .labelColor
        case .stage: .controlAccentColor
        case .result: .systemTeal
        case .warning: .systemOrange
        case .failure: .systemRed
        }
    }

    var font: NSFont {
        .monospacedSystemFont(ofSize: 12, weight: self == .normal || self == .muted ? .regular : .semibold)
    }
}

struct ConsecutiveDiagnosticLogFolder: Equatable {
    enum Update: Equatable {
        case append(String)
        case repeated(String, occurrences: Int)
    }

    private static let maximumFoldableUTF16Length = 8_192
    private(set) var previousLine: String? = nil
    private(set) var occurrences = 0

    mutating func ingest(_ line: String) -> Update {
        let tone = LogTone.classify(line)
        guard tone == .warning || tone == .failure,
              line.utf16.count <= Self.maximumFoldableUTF16Length else {
            reset()
            return .append(line)
        }

        guard previousLine == line else {
            previousLine = line
            occurrences = 1
            return .append(line)
        }

        let next = occurrences.addingReportingOverflow(1)
        guard !next.overflow else {
            previousLine = line
            occurrences = 1
            return .append(line)
        }
        occurrences = next.partialValue
        return .repeated(line, occurrences: occurrences)
    }

    mutating func reset() {
        previousLine = nil
        occurrences = 0
    }
}

func trailingLogLineRange(_ expected: String, in value: NSString) -> NSRange? {
    guard !expected.isEmpty, value.length > 0, value.character(at: value.length - 1) != 0x0A else {
        return nil
    }
    let fullRange = NSRange(location: 0, length: value.length)
    let separator = value.range(of: "\n", options: .backwards, range: fullRange)
    let start = separator.location == NSNotFound ? 0 : NSMaxRange(separator)
    let lineRange = NSRange(location: start, length: value.length - start)
    let line = value.substring(with: lineRange)
    return line == expected || line.hasPrefix(expected + " · ") ? lineRange : nil
}

private struct LatestDiagnostics {
    private(set) var failure: String?
    private(set) var warning: String?
    private var failureIsSummary = false

    mutating func ingest(_ line: String) -> Bool {
        let tone = LogTone.classify(line)
        guard tone == .failure || tone == .warning else { return false }
        let message = LogTone.message(line)
        // Keep presentation bounded; the launcher retains the full output in history.
        let scalars = message.unicodeScalars
        let bounded = String(String.UnicodeScalarView(scalars.prefix(4_096)))
            + (scalars.count > 4_096 ? "…" : "")
        switch tone {
        case .failure:
            let isSummary = countStatusValue(in: message) != nil
                || message.range(of: #"(?i)^(?:\[Summary\]|\[GATE\s*\d+\s*\]|Integrity(?: Issues)?:|(?:(?:Error:|\[ERROR\s*\])\s*)?exiting with failures\b)"#,
                                 options: .regularExpression) != nil
            // A final aggregate must not hide the most recent actionable error.
            guard !isSummary || failure == nil || failureIsSummary else { return false }
            guard failure != bounded else { return false }
            failure = bounded
            failureIsSummary = isSummary
        case .warning:
            guard warning != bounded else { return false }
            warning = bounded
        default: return false
        }
        return true
    }
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
    private let maxCriticalBytes: Int
    private let maxCriticalEntries: Int
    private var pending = ""
    private var pendingBytes = 0
    private var pendingEntries = 0
    private var criticalBytes = 0
    private var criticalEntries = 0
    private var criticalOverflow = false
    private var pendingProgress: String?
    private var omittedEntries: UInt64 = 0
    private var deliveryInFlight = false

    init(maxBytes: Int = maxProcessLogBatchBytes, maxEntries: Int = maxProcessLogBatchEntries,
         maxCriticalBytes: Int = maxProcessLogBatchBytes, maxCriticalEntries: Int = maxProcessLogBatchEntries) {
        precondition(maxBytes >= 0 && maxEntries >= 0 && maxCriticalBytes >= 0 && maxCriticalEntries >= 0)
        self.maxBytes = maxBytes
        self.maxEntries = maxEntries
        self.maxCriticalBytes = maxCriticalBytes
        self.maxCriticalEntries = maxCriticalEntries
    }

    func enqueue(_ entry: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        // Control records and diagnostics have a separate bounded reserve, preserving arrival order.
        for line in entry.split(separator: "\n", omittingEmptySubsequences: false) {
            let text = String(line)
            // Progress is a replaceable snapshot, not a result or diagnostic.
            if let payload = FileStageProgress.payload(text) {
                pendingProgress = payload.utf8.count <= 1_024 ? text : "MFB_PROGRESS={}"
                continue
            }
            let tone = LogTone.classify(text)
            let critical = text.contains("MFB_")
                || text.contains("[PHOTOS PROFILE]") || countStatusValue(in: text) != nil
                || tone == .warning || tone == .failure || tone == .result
            let limit = critical ? maxCriticalBytes : maxBytes
            let used = critical ? criticalBytes : pendingBytes
            let entries = critical ? criticalEntries : pendingEntries
            let entryLimit = critical ? maxCriticalEntries : maxEntries
            let separatorBytes = pending.isEmpty ? 0 : 1
            let entryBytes = text.utf8.count
            if entries >= entryLimit || separatorBytes > limit - used
                || entryBytes > limit - used - separatorBytes {
                if critical { criticalOverflow = true }
                if omittedEntries < UInt64.max { omittedEntries += 1 }
                continue
            }
            if separatorBytes > 0 { pending.append("\n") }
            pending.append(text)
            if critical {
                criticalBytes += separatorBytes + entryBytes
                criticalEntries += 1
            } else {
                pendingBytes += separatorBytes + entryBytes
                pendingEntries += 1
            }
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
        if let progress = pendingProgress {
            payload = progress + (payload.isEmpty ? "" : "\n" + payload)
        }
        pendingProgress = nil
        if omittedEntries > 0 {
            if !payload.isEmpty { payload.append("\n") }
            payload.append(localized("log.omitted", omittedEntries))
        }
        if criticalOverflow {
            // Missing control records must invalidate completion, even if a later record is valid.
            if !payload.isEmpty { payload.append("\n") }
            payload.append("MFB_BATCH_RESULT={}\n[WARN] \(localized("log.critical_overflow"))")
        }
        pending.removeAll(keepingCapacity: true)
        pendingBytes = 0
        pendingEntries = 0
        criticalBytes = 0
        criticalEntries = 0
        criticalOverflow = false
        omittedEntries = 0
        return payload
    }

    func finishDelivery() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let hasPending = pendingEntries > 0 || criticalEntries > 0 || omittedEntries > 0 || pendingProgress != nil
        if !hasPending { deliveryInFlight = false }
        return hasPending
    }

    var isIdle: Bool {
        lock.lock()
        defer { lock.unlock() }
        return !deliveryInFlight && pendingEntries == 0 && criticalEntries == 0 && omittedEntries == 0 && pendingProgress == nil
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
        if UserDefaults.standard.bool(forKey: developerPreferenceKey),
           let configured = UserDefaults.standard.string(forKey: "MFBGuiCoreBinary") {
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

private enum ProcessorReadiness: Equatable, Sendable {
    case checking, ready, unavailable
    case failed(diagnostic: String, omitted: Bool)

    var message: String {
        switch self {
        case .checking: return localized("status.checking_processor")
        case .ready: return localized("status.processor_ready")
        case .unavailable: return localized("status.processor_unavailable")
        case let .failed(diagnostic, omitted):
            let detail = [diagnostic, omitted ? localized("log.omitted", UInt64(1)) : ""]
                .filter { !$0.isEmpty }.joined(separator: "\n")
            return localized("status.processor_failed") + (detail.isEmpty ? "" : ": " + detail)
        }
    }
}

@MainActor
private final class NativeHost {
    var onLog: ((String) -> Void)?
    var onCompletion: ((Result<String, Error>) -> Void)?
    private var activeProcess: Process?
    private var pendingLaunch = false
    private var launchGeneration = UUID()
    private var controlFile: URL?
    private var controlToken = UUID().uuidString
    private(set) var controlState = "running"
    private(set) var lastExitStatus: Int32?
    private let processLogs = ProcessLogBackpressure()
    private var pendingProcessCompletion: (() -> Void)?

    var isRunning: Bool { activeProcess != nil || pendingLaunch }
    var canPause: Bool { activeProcess != nil && controlState != "cancelled" }

    var isPaused: Bool {
        guard controlState == "paused", let controlFile else { return false }
        return (try? String(contentsOf: controlFile.appendingPathExtension("ack"), encoding: .utf8))
            == "paused\n\(controlToken)"
    }

    func setControlState(_ state: String) throws {
        guard ["running", "paused", "cancelled"].contains(state) else {
            throw HostError(message: "Invalid batch control state")
        }
        let token = UUID().uuidString
        if let controlFile {
            try Data("\(state)\n\(token)".utf8).write(to: controlFile, options: .atomic)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: controlFile.path)
        }
        controlState = state
        controlToken = token
        if state == "cancelled", pendingLaunch {
            pendingLaunch = false
            launchGeneration = UUID()
            completeProcessingAfterLogs(status: 130)
        }
    }

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

    nonisolated func checkVersionAlignment() -> ProcessorReadiness {
        guard let binary = ProcessorLocator.resolve(), ProcessorLocator.resolveTool(named: "img") != nil,
              ProcessorLocator.resolveTool(named: "vid") != nil else {
            return .unavailable
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
                return .ready
            }
            let diagnostic = String(decoding: capture.data, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return .failed(diagnostic: diagnostic, omitted: capture.exceeded)
        } catch {
            return .failed(diagnostic: error.localizedDescription, omitted: false)
        }
    }

    func startProcessing(_ request: ProcessorRequest, photosAutomationAuthorized: Bool = false) {
        guard !isRunning else {
            onCompletion?(.failure(HostError(message: localized("error.task_running"))))
            return
        }
        guard let binary = ProcessorLocator.resolve() else {
            onCompletion?(.failure(HostError(message: ProcessorLocator.missingError())))
            return
        }
        controlState = "running"
        lastExitStatus = nil
        if processingRequiresPhotosAutomation(request), !photosAutomationAuthorized {
            pendingLaunch = true
            launchGeneration = UUID()
            let generation = launchGeneration
            requestPhotosAutomationPermission { [weak self] result in
                guard let self, self.pendingLaunch, self.launchGeneration == generation else { return }
                self.pendingLaunch = false
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
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        onLog?(localized("log.backend_start", binary.path))
        do {
            let directory = FileManager.default.temporaryDirectory
                .appendingPathComponent("mfb-gui-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                                    attributes: [.posixPermissions: 0o700])
            controlFile = directory.appendingPathComponent("control")
            try setControlState("running")
            environment["MFB_BATCH_CONTROL_FILE"] = controlFile!.path
            process.environment = environment
            try process.run()
        }
        catch {
            removeControlFile()
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

    private func removeControlFile() {
        guard let controlFile else { return }
        do { try FileManager.default.removeItem(at: controlFile.deletingLastPathComponent()) }
        catch { onLog?(localized("error.control_cleanup", error.localizedDescription)) }
        self.controlFile = nil
    }

    func validateControlForSelfTest() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("mfb-control-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                                attributes: [.posixPermissions: 0o700])
        controlFile = directory.appendingPathComponent("control")
        defer { removeControlFile() }
        try setControlState("paused")
        let desired = try String(contentsOf: controlFile!, encoding: .utf8)
        guard desired == "paused\n\(controlToken)", !isPaused else {
            throw HostError(message: "Pause was acknowledged before reaching a safe boundary")
        }
        try Data(desired.utf8).write(to: controlFile!.appendingPathExtension("ack"), options: .atomic)
        guard isPaused else { throw HostError(message: "Valid pause acknowledgment was ignored") }
        try setControlState("running")
        try setControlState("paused")
        guard !isPaused else { throw HostError(message: "Stale acknowledgment paused a new transaction") }
        try setControlState("cancelled")
        guard controlState == "cancelled", !isPaused else { throw HostError(message: "Cancel state failed") }
        removeControlFile()
        pendingLaunch = true
        var completions = 0
        onCompletion = { _ in completions += 1 }
        try setControlState("cancelled")
        guard !isRunning, completions == 1 else {
            throw HostError(message: "Preflight cancellation could launch a late child")
        }
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
            self.pendingLaunch = false
            self.lastExitStatus = status
            self.removeControlFile()
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
    private let preferences: UserDefaults
    private let window: NSWindow
    private let titleLabel = NSTextField(labelWithString: "Modern Format Boost")
    private let aboutIcon = NSButton()
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
    private var processingRow: NSGridRow?
    private let languagePopup = NSPopUpButton()
    private let appearancePopup = NSPopUpButton()
    private let developerCheck = NSButton(checkboxWithTitle: "", target: nil, action: nil)
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
    private let logScroll = NSScrollView()
    private let countStatusLabel = NSTextField(labelWithString: "")
    private var countStatus: String?
    private let statusLabel = NSTextField(labelWithString: "")
    private let progressIndicator = NSProgressIndicator()
    private let fileProgressIndicator = NSProgressIndicator()
    private let fileProgressLabel = NSTextField(labelWithString: "")
    private let fileProgressRow = NSStackView()
    private var fileProgress = FileStageProgress()
    private var gateActivity = GateCheckActivity()
    private var logPhase = PhaseLogPresentation()
    private var diagnosticLogFolder = ConsecutiveDiagnosticLogFolder()
    private var latestDiagnostics = LatestDiagnostics()
    private let latestFailureLabel = NSTextField(labelWithString: "")
    private let latestWarningLabel = NSTextField(labelWithString: "")
    private let latestDiagnosticsRow = NSStackView()
    private let chooseButton = NSButton(title: "", target: nil, action: nil)
    private let backupButton = NSButton(title: "", target: nil, action: nil)
    private let backupRow = NSStackView()
    private let photosScopeButton = NSButton(title: "", target: nil, action: nil)
    private let photosScopeRow = NSStackView()
    private let options = NSStackView()
    private var optionColumns: [NSStackView] = []
    private let optionsSpacer = NSView()
    private let openButton = NSButton(title: "", target: nil, action: nil)
    private let copyButton = NSButton(title: "", target: nil, action: nil)
    private let runButton = NSButton(title: "", target: nil, action: nil)
    private let historyButton = NSButton(title: "", target: nil, action: nil)
    private let settingsButton = NSButton(title: "", target: nil, action: nil)
    private let helpButton = NSButton(title: "", target: nil, action: nil)
    private var settingsPanel: MediaSettingsPanel?
    private var historyPanel: ProcessingHistoryPanel?
    private let diagnosticsButton = NSButton(title: "", target: nil, action: nil)
    private let diagnosticsTextView = NSTextView()
    private var diagnosticsPanel: NSPanel?
    private var photosDiagnostics = PhotosDiagnostics()
    private var batchResults = BatchResults()
    private var resolvedHistoryDirectory = historyDirectory
    private let pauseButton = NSButton(title: "", target: nil, action: nil)
    private let stopButton = NSButton(title: "", target: nil, action: nil)
    private var aboutWindow: NSWindow?
    private var closeWhenFinished = false
    private var lastRequest: ProcessorRequest?
    private var sawResumeDecision = false
    private var configurationControlsEnabled = true
    private var processorStatus: ProcessorReadiness = .checking
    private enum StatusDisplay {
        case processor
        case localized(String, [CVarArg])
        case message(String)
    }
    private var statusDisplay: StatusDisplay = .processor
    private var selectedPhotosContainer: PhotosAuditContainer?
    private var processingStartedAt: TimeInterval?
    private var refreshTimer: Timer?
    private var processingActivity: NSObjectProtocol?

    private var developerMode: Bool { developerCheck.state == .on }

    private var optionControls: [(NSButton, String)] { [
        (verboseCheck, "verbose"), (shortestPathCheck, "shortestPath"),
        (resumeCheck, "resume"), (freshCheck, "fresh"),
        (archiveCheck, "archive"), (retryCheck, "retry"),
        (forceCheck, "force"), (dryRunCheck, "dryRun"),
        (plainCheck, "plain"), (inPlaceCheck, "inPlace"), (watchCheck, "watch"),
    ] }

    private func optionKey(_ name: String, for operation: OperationMode) -> String {
        "MFBGuiOption.\(operation.rawValue).\(name)"
    }

    private func restoreOptions(for operation: OperationMode) {
        for (control, name) in optionControls {
            let saved = preferences.object(forKey: optionKey(name, for: operation)) as? Bool
            let enabledByDefault = name == "verbose" || name == "archive"
                || (name == "fresh" && operation.capabilities.supportsResume)
                || (name == "shortestPath" && operation.backendMode == "fast-img")
            control.state = (saved ?? enabledByDefault) ? .on : .off
        }
        verboseCheck.state = .on
        if operation.capabilities.supportsResume {
            let resuming = freshCheck.state != .on
                && (resumeCheck.state == .on || retryCheck.state == .on)
            resumeCheck.state = resuming ? .on : .off
            freshCheck.state = resuming ? .off : .on
            if !resuming { retryCheck.state = .off }
        }
    }

    private func saveOption(_ control: NSButton) {
        guard let name = optionControls.first(where: { $0.0 === control })?.1 else { return }
        preferences.set(control.state == .on, forKey: optionKey(name, for: selectedOperation))
    }

    init(preferences: UserDefaults = .standard) {
        self.preferences = preferences
        resolvedHistoryDirectory = initialHistoryDirectory(preferences: preferences)
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
        processorStatus = .checking
        statusDisplay = .processor
        refreshStatus()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else { return }
            let status = self.host.checkVersionAlignment()
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.processorStatus = status
                if self.configurationControlsEnabled, case .processor = self.statusDisplay { self.refreshStatus() }
            }
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        requestClose()
    }

    func requestClose() -> Bool {
        guard !host.isRunning else {
            closeWhenFinished = true
            stopProcessing()
            return false
        }
        return true
    }

    func windowDidBecomeKey(_ notification: Notification) {
        refreshProcessingStatus()
    }

    private func refreshStatus() {
        switch statusDisplay {
        case .processor: statusLabel.stringValue = processorStatus.message
        case let .localized(key, arguments):
            let format = LocalizationCatalog.shared.text(key)
            statusLabel.stringValue = arguments.isEmpty ? format : String(format: format, locale: Locale.current, arguments: arguments)
        case let .message(message): statusLabel.stringValue = message
        }
    }

    private func setLocalizedStatus(_ key: String, _ arguments: CVarArg...) {
        statusDisplay = .localized(key, arguments)
        refreshStatus()
    }

    private func setStatus(_ message: String) {
        statusDisplay = .message(message)
        refreshStatus()
    }

    @objc private func refreshProcessingStatus() {
        guard let startedAt = processingStartedAt else { return }
        refreshFileProgress()
        pauseButton.isEnabled = host.canPause
        if host.controlState == "cancelled" {
            setLocalizedStatus("status.stopping")
            return
        }
        if host.controlState == "paused" {
            setLocalizedStatus(host.isPaused ? "status.paused" : "status.pausing")
            return
        }
        let elapsed = max(0, Int(ProcessInfo.processInfo.systemUptime - startedAt))
        setLocalizedStatus("status.running_elapsed", elapsed / 60, elapsed % 60)
    }

    private func configureWindow() {
        window.title = "Modern Format Boost · \(appVersion)"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .visible
        window.setContentSize(mainWindowContentSize)
        window.contentMinSize = mainWindowContentSize
        window.contentMaxSize = mainWindowContentSize
        window.standardWindowButton(.zoomButton)?.isEnabled = false
        window.tabbingMode = .disallowed
        window.delegate = self

        let root = NativeDropView()
        root.material = .windowBackground
        root.blendingMode = .behindWindow
        root.state = .active
        root.onDrop = { [weak self] path in self?.acceptTarget(path) }
        window.contentView = root

        let icon = aboutIcon
        icon.isBordered = false
        icon.target = self
        icon.action = #selector(showAbout)
        icon.toolTip = localized("menu.about")
        icon.setAccessibilityLabel(localized("menu.about"))
        icon.image = NSApp.applicationIconImage
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.widthAnchor.constraint(equalToConstant: 36).isActive = true
        icon.heightAnchor.constraint(equalToConstant: 36).isActive = true
        icon.setContentHuggingPriority(.required, for: .horizontal)
        titleLabel.font = .systemFont(ofSize: 18, weight: .semibold)
        subtitleLabel.font = .systemFont(ofSize: 12)
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
        developerCheck.target = self
        developerCheck.action = #selector(developerModeChanged)
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
        targetRow.alignment = .centerY
        targetRow.spacing = 8
        targetField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        targetField.widthAnchor.constraint(equalTo: targetRow.widthAnchor, constant: -100).isActive = true
        chooseButton.widthAnchor.constraint(equalToConstant: 92).isActive = true

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
        operationPopup.action = #selector(operationChanged)
        let grid = NSGridView(views: [
            [mediaLabel, processingPopup],
            [operationLabel, operationPopup],
        ])
        processingRow = grid.row(at: 0)
        grid.rowSpacing = 8
        grid.columnSpacing = 12
        grid.column(at: 0).xPlacement = .leading
        grid.column(at: 0).width = 120
        grid.column(at: 1).xPlacement = .fill
        for popup in [processingPopup, operationPopup] {
            popup.setContentHuggingPriority(.defaultLow, for: .horizontal)
            popup.widthAnchor.constraint(equalTo: grid.widthAnchor, constant: -132).isActive = true
        }
        mediaLabel.widthAnchor.constraint(equalToConstant: 120).isActive = true
        operationLabel.widthAnchor.constraint(equalTo: mediaLabel.widthAnchor).isActive = true

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

        for control in [
            ultimateCheck, verboseCheck, shortestPathCheck, archiveCheck,
            forceCheck, dryRunCheck, plainCheck, inPlaceCheck, watchCheck,
        ] {
            control.target = self
            control.action = #selector(optionChanged(_:))
        }
        for control in [resumeCheck, freshCheck, retryCheck] {
            control.target = self
            control.action = #selector(resumeChoiceChanged(_:))
        }
        resumeCheck.setButtonType(.radio)
        freshCheck.setButtonType(.radio)
        optionColumns = [
            [ultimateCheck, freshCheck, resumeCheck, dryRunCheck],
            [shortestPathCheck, forceCheck, plainCheck, inPlaceCheck],
            [verboseCheck, archiveCheck, retryCheck, watchCheck],
        ].map { controls -> NSStackView in
            let column = NSStackView(views: controls)
            column.orientation = .vertical
            column.alignment = .leading
            column.spacing = 5
            column.setContentHuggingPriority(.defaultHigh, for: .horizontal)
            return column
        }
        for column in optionColumns { options.addArrangedSubview(column) }
        optionsSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        options.addArrangedSubview(optionsSpacer)
        options.orientation = .horizontal
        options.distribution = .fill
        options.alignment = .top
        options.spacing = 12

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
        historyButton.target = self
        historyButton.action = #selector(openHistory)
        historyButton.image = NSImage(systemSymbolName: "clock.arrow.circlepath", accessibilityDescription: nil)
        historyButton.imagePosition = .imageLeading
        settingsButton.target = self
        settingsButton.action = #selector(showSettings)
        settingsButton.image = NSImage(systemSymbolName: "gearshape", accessibilityDescription: nil)
        settingsButton.imagePosition = .imageOnly
        settingsButton.widthAnchor.constraint(equalToConstant: 32).isActive = true
        helpButton.target = self
        helpButton.action = #selector(openHelp)
        helpButton.bezelStyle = .helpButton
        helpButton.toolTip = localized("button.help")
        helpButton.setAccessibilityLabel(localized("button.help"))
        diagnosticsButton.target = self
        diagnosticsButton.action = #selector(showDiagnostics)
        diagnosticsButton.image = NSImage(systemSymbolName: "chart.bar.xaxis", accessibilityDescription: nil)
        diagnosticsButton.imagePosition = .imageOnly
        diagnosticsButton.widthAnchor.constraint(equalToConstant: 32).isActive = true
        pauseButton.target = self
        pauseButton.action = #selector(togglePause)
        stopButton.target = self
        stopButton.action = #selector(stopProcessing)
        pauseButton.isEnabled = false
        stopButton.isEnabled = false
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let actionRow = NSStackView(views: [settingsButton, helpButton, historyButton, diagnosticsButton, openButton, copyButton, spacer, pauseButton, stopButton, runButton])
        actionRow.orientation = .horizontal
        actionRow.alignment = .centerY
        actionRow.spacing = 8

        logView.isEditable = false
        logView.isSelectable = true
        logView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        logView.textContainerInset = NSSize(width: 8, height: 8)
        logView.backgroundColor = .textBackgroundColor
        logScroll.documentView = logView
        logScroll.hasVerticalScroller = true
        logScroll.borderType = .lineBorder
        logScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 260).isActive = true

        countStatusLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        countStatusLabel.isHidden = true
        countStatusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        fileProgressIndicator.style = .bar
        fileProgressIndicator.minValue = 0
        fileProgressIndicator.maxValue = 100
        fileProgressIndicator.isIndeterminate = false
        fileProgressIndicator.widthAnchor.constraint(equalToConstant: 180).isActive = true
        fileProgressLabel.font = .monospacedDigitSystemFont(ofSize: 12, weight: .medium)
        fileProgressLabel.lineBreakMode = .byWordWrapping
        fileProgressLabel.maximumNumberOfLines = 2
        fileProgressLabel.cell?.wraps = true
        fileProgressLabel.cell?.usesSingleLineMode = false
        fileProgressLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        fileProgressRow.orientation = .horizontal
        fileProgressRow.alignment = .centerY
        fileProgressRow.spacing = 12
        fileProgressRow.addArrangedSubview(fileProgressIndicator)
        fileProgressRow.addArrangedSubview(fileProgressLabel)
        fileProgressRow.isHidden = true

        latestDiagnosticsRow.orientation = .vertical
        latestDiagnosticsRow.alignment = .leading
        latestDiagnosticsRow.spacing = 4
        for (label, color) in [(latestFailureLabel, NSColor.systemRed), (latestWarningLabel, NSColor.systemOrange)] {
            label.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .medium)
            label.textColor = color
            label.lineBreakMode = .byTruncatingMiddle
            label.isSelectable = true
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            latestDiagnosticsRow.addArrangedSubview(label)
            label.widthAnchor.constraint(equalTo: latestDiagnosticsRow.widthAnchor).isActive = true
        }
        latestDiagnosticsRow.isHidden = true

        statusLabel.textColor = .secondaryLabelColor
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        statusLabel.addGestureRecognizer(NSClickGestureRecognizer(target: self, action: #selector(repairCore)))
        statusLabel.toolTip = localized("status.core_help")
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
            countStatusLabel, fileProgressRow, logScroll, latestDiagnosticsRow, statusRow,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.setCustomSpacing(16, after: header)
        stack.translatesAutoresizingMaskIntoConstraints = false
        for view in [
            header, targetRow, grid, backupRow, photosScopeRow, metadataSafetyLabel, options, commandField, actionRow,
            countStatusLabel, fileProgressRow, logScroll, latestDiagnosticsRow, statusRow,
        ] {
            view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            stack.topAnchor.constraint(equalTo: root.safeAreaLayoutGuide.topAnchor, constant: 16),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -16),
        ])
        applyLocalization()
        selectSavedPreferences()
        restoreOptions(for: .adjacent)
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
        setLocalizedStatus("status.photos_scope_loading")
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
            setLocalizedStatus("status.ready")
        }
    }

    @objc private func configurationChanged() {
        applyCapabilityState()
        updateMetadataSafetyNotice()
        guard configurationControlsEnabled else { return }
        guard !targetField.stringValue.isEmpty else {
            commandField.stringValue = ""
            statusDisplay = .processor
            refreshStatus()
            return
        }
        do {
            commandField.stringValue = try host.terminalCommand(for: request())
            setLocalizedStatus("status.ready")
        } catch {
            commandField.stringValue = ""
            setStatus(error.localizedDescription)
        }
    }

    @objc private func optionChanged(_ sender: NSButton) {
        saveOption(sender)
        configurationChanged()
    }

    @objc private func operationChanged() {
        restoreOptions(for: selectedOperation)
        configurationChanged()
    }

    @objc private func developerModeChanged() {
        preferences.set(developerMode, forKey: developerPreferenceKey)
        updateOptionTitles()
        refreshOperationPopup()
        watchCheck.state = developerMode
            && (preferences.object(forKey: optionKey("watch", for: selectedOperation)) as? Bool ?? false)
            ? .on : .off
        configurationChanged()
        if !developerMode { diagnosticsPanel?.orderOut(nil) }
    }

    @objc private func resumeChoiceChanged(_ sender: NSButton) {
        if sender === freshCheck {
            freshCheck.state = .on
            resumeCheck.state = .off
            retryCheck.state = .off
        } else if sender === resumeCheck || sender.state == .on {
            resumeCheck.state = .on
            freshCheck.state = .off
            if selectedOperation.backendMode == "fast-img" { retryCheck.state = .on }
        } else if selectedOperation.backendMode == "fast-img" {
            // Fast-img aliases resume/retry; disabling retry selects a fresh run.
            resumeCheck.state = .off
            freshCheck.state = .on
        }
        for control in [resumeCheck, freshCheck, retryCheck] { saveOption(control) }
        configurationChanged()
    }

    @objc private func openInTerminal() {
        guard developerMode, configurationControlsEnabled else { return }
        do { setStatus(try host.openInTerminal(request())) }
        catch { present(error) }
    }

    @objc private func copyCommand() {
        do {
            let command = try host.terminalCommand(for: request())
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(command, forType: .string)
            setLocalizedStatus("status.command_copied")
        } catch { present(error) }
    }

    private func clearBatchLog() {
        logView.string = ""
        logPhase = PhaseLogPresentation()
        diagnosticLogFolder.reset()
        batchResults = BatchResults()
        latestDiagnostics = LatestDiagnostics()
        refreshLatestDiagnostics()
        fileProgress = FileStageProgress()
        gateActivity = GateCheckActivity()
        refreshFileProgress()
        photosDiagnostics.reset()
        refreshDiagnostics()
        countStatus = nil
        countStatusLabel.isHidden = true
        appendLog(localized("log.history", resolvedHistoryDirectory.path))
    }

    @objc private func openHistory() {
        if historyPanel == nil { historyPanel = ProcessingHistoryPanel(directory: resolvedHistoryDirectory) }
        historyPanel?.refresh(directory: resolvedHistoryDirectory)
        historyPanel?.show()
    }

    @objc private func showDiagnostics() {
        guard developerMode else { return }
        if diagnosticsPanel == nil {
            let panel = NSPanel(
                contentRect: NSRect(x: 0, y: 0, width: 440, height: 480),
                styleMask: [.titled, .closable, .resizable, .utilityWindow],
                backing: .buffered, defer: false
            )
            panel.contentMinSize = NSSize(width: 360, height: 300)
            panel.title = localized("button.photos_diagnostics")
            panel.isReleasedWhenClosed = false
            diagnosticsTextView.isEditable = false
            diagnosticsTextView.isSelectable = true
            diagnosticsTextView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
            diagnosticsTextView.textContainerInset = NSSize(width: 12, height: 12)
            let scroll = NSScrollView(frame: panel.contentView!.bounds)
            scroll.autoresizingMask = [.width, .height]
            scroll.hasVerticalScroller = true
            diagnosticsTextView.frame = scroll.contentView.bounds
            diagnosticsTextView.isVerticallyResizable = true
            diagnosticsTextView.textContainer?.widthTracksTextView = true
            scroll.documentView = diagnosticsTextView
            panel.contentView?.addSubview(scroll)
            panel.center()
            diagnosticsPanel = panel
        }
        refreshDiagnostics()
        diagnosticsPanel?.makeKeyAndOrderFront(self)
    }

    private func refreshDiagnostics() {
        diagnosticsTextView.string = photosDiagnostics.rendered()
    }

    @objc private func togglePause() {
        guard host.canPause else { return }
        do {
            try host.setControlState(host.controlState == "paused" ? "running" : "paused")
            pauseButton.title = localized(host.controlState == "paused" ? "button.resume" : "button.pause")
            refreshProcessingStatus()
            appendLog(statusLabel.stringValue)
        } catch { present(error) }
    }

    @objc private func stopProcessing() {
        guard host.isRunning else { return }
        do {
            try host.setControlState("cancelled")
            pauseButton.isEnabled = false
            stopButton.isEnabled = false
            refreshProcessingStatus()
            appendLog(statusLabel.stringValue)
        } catch { present(error) }
    }

    @objc func showAbout() {
        do {
            let text = try bundledLicenseText()
            let panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 680, height: 520),
                                 styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
            panel.isReleasedWhenClosed = false
            panel.title = "\(localized("menu.about")) · \(appVersion)"
            var scrollFrame = panel.contentView!.bounds
            scrollFrame.origin.y = 48
            scrollFrame.size.height -= 48
            let scroll = NSScrollView(frame: scrollFrame)
            scroll.autoresizingMask = [.width, .height]
            scroll.hasVerticalScroller = true
            let textView = NSTextView(frame: scroll.bounds)
            textView.isEditable = false
            textView.isSelectable = true
            textView.font = .systemFont(ofSize: 13)
            textView.textContainerInset = NSSize(width: 20, height: 16)
            textView.autoresizingMask = [.width]
            textView.textContainer?.widthTracksTextView = true
            textView.string = text
            scroll.documentView = textView
            panel.contentView?.addSubview(scroll)
            developerCheck.removeFromSuperview()
            developerCheck.frame = NSRect(x: 20, y: 14, width: 520, height: 24)
            developerCheck.autoresizingMask = [.width, .maxYMargin]
            panel.contentView?.addSubview(developerCheck)
            aboutWindow?.close()
            aboutWindow = panel
            panel.center()
            panel.makeKeyAndOrderFront(nil)
        } catch { present(error) }
    }

    @objc private func runHere() {
        guard !host.isRunning else { return }
        do {
            let request = try request()
            lastRequest = request
            sawResumeDecision = false
            clearBatchLog()
            try host.setControlState("running")
            setProcessing(true)
            appendLog("▶︎ \(try host.terminalCommand(for: request))")
            host.startProcessing(request)
        } catch {
            setProcessing(false)
            present(error)
        }
    }

    private var selectedOperation: OperationMode {
        operationPopup.selectedItem?.representedObject
            .flatMap { OperationMode(rawValue: $0 as? String ?? "") } ?? .adjacent
    }

    private func applyCapabilityState() {
        let capabilities = selectedOperation.capabilities
        settingsButton.isEnabled = configurationControlsEnabled
        settingsButton.toolTip = localized("settings.title")
        if let fixed = capabilities.fixedProcessingMode {
            processingPopup.selectItem(
                at: ProcessingMode.allCases.firstIndex { $0.rawValue == fixed.rawValue } ?? 0,
            )
        }
        processingPopup.isEnabled = configurationControlsEnabled && capabilities.usesProcessingSelection
        processingRow?.isHidden = !capabilities.usesProcessingSelection
        // The launcher always enables --ultimate for supported operations.
        // Show the effective policy instead of an off switch it cannot honor.
        ultimateCheck.isEnabled = false
        ultimateCheck.state = capabilities.supportsUltimate ? .on : .off
        verboseCheck.state = .on
        verboseCheck.isEnabled = false
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
        ultimateCheck.isHidden = !developerMode || !capabilities.supportsUltimate
        verboseCheck.isHidden = !developerMode
        resumeCheck.isHidden = true
        freshCheck.isHidden = true
        retryCheck.isHidden = true
        shortestPathCheck.isHidden = !capabilities.supportsShortestPath
        archiveCheck.isHidden = !developerMode || !capabilities.supportsArchive
        for control in [forceCheck, plainCheck, inPlaceCheck] {
            control.isHidden = !developerMode || !capabilities.supportsStandardOptions
        }
        watchCheck.isHidden = !developerMode
        watchCheck.isEnabled = configurationControlsEnabled && developerMode
        if !developerMode { watchCheck.state = .off }
        copyButton.isHidden = !developerMode
        openButton.isHidden = !developerMode
        diagnosticsButton.isHidden = !developerMode
        commandField.isHidden = !developerMode
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
        for column in optionColumns {
            column.isHidden = column.arrangedSubviews.allSatisfy(\.isHidden)
        }
        optionsSpacer.isHidden = developerMode
        options.distribution = developerMode ? .fillEqually : .fill
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
            verbose: true,
            shortestPath: shortestPathCheck.state == .on,
            resume: developerMode && resumeCheck.state == .on,
            fresh: developerMode && freshCheck.state == .on,
            archive: archiveCheck.state == .on,
            retry: developerMode && retryCheck.state == .on,
            force: forceCheck.state == .on,
            dryRun: dryRunCheck.state == .on,
            plain: plainCheck.state == .on,
            inPlace: inPlaceCheck.state == .on,
            watch: watchCheck.state == .on,
            photosContainer: selectedPhotosContainer,
            mediaSettings: MediaSettings(preferences: preferences),
        )
    }

    private func appendLog(_ text: String, captureDiagnostics: Bool = true,
                           now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        var latestChanged = false
        var replacePresentation = false
        var presentationChanged = false
        var displayLines: [String] = []
        var lastBufferedDiagnostic: (line: String, index: Int)?
        var rawLines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if text.hasSuffix("\n"), !rawLines.isEmpty { rawLines.removeLast() }
        for line in rawLines {
            if let phaseUpdate = logPhase.ingest(line, now: now) {
                diagnosticLogFolder.reset()
                lastBufferedDiagnostic = nil
                switch phaseUpdate {
                case .changed(let phase, let replace):
                    if phase == .processing { gateActivity = GateCheckActivity() }
                    if replace {
                        displayLines.removeAll(keepingCapacity: true)
                        replacePresentation = true
                    }
                    displayLines.append("[PHASE] " + localized("log.phase.\(phase.rawValue)"))
                    refreshFileProgress(now: now)
                case .invalid:
                    let warning = "[WARN] " + localized("log.phase.invalid")
                    displayLines.append(warning)
                    if captureDiagnostics && latestDiagnostics.ingest(warning) { latestChanged = true }
                case .unchanged: break
                }
                continue
            }
            if gateActivity.ingest(line, now: now) {
                diagnosticLogFolder.reset()
                lastBufferedDiagnostic = nil
                refreshFileProgress(now: now)
                if gateActivity.invalid {
                    let warning = "[WARN] " + localized("gate.activity.invalid")
                    displayLines.append(warning)
                    if captureDiagnostics && latestDiagnostics.ingest(warning) { latestChanged = true }
                }
                continue
            }
            if fileProgress.ingest(line) {
                diagnosticLogFolder.reset()
                lastBufferedDiagnostic = nil
                refreshFileProgress()
                continue
            }
            if let summary = batchResults.ingest(line) {
                diagnosticLogFolder.reset()
                lastBufferedDiagnostic = nil
                displayLines.append(developerMode ? "\(line)\n\(summary)" : summary)
                continue
            }
            var visibleLine = line
            if line.hasPrefix(HistoryFileFailure.prefix) {
                do {
                    if let failure = try HistoryFileFailure.decode(line) {
                        visibleLine = "[FAIL] \(failure.source_path): \(failure.message)"
                        if developerMode { displayLines.append(line) }
                    }
                } catch { visibleLine = "[WARN] " + localized("history.batch_issues.invalid") }
                if captureDiagnostics && latestDiagnostics.ingest(visibleLine) { latestChanged = true }
                displayLines.append(visibleLine)
                diagnosticLogFolder.reset()
                lastBufferedDiagnostic = nil
                continue
            }
            if captureDiagnostics && latestDiagnostics.ingest(visibleLine) { latestChanged = true }
            switch diagnosticLogFolder.ingest(visibleLine) {
            case .append(let raw):
                displayLines.append(raw)
                let index = displayLines.count - 1
                if LogTone.classify(raw) == .warning || LogTone.classify(raw) == .failure {
                    lastBufferedDiagnostic = (raw, index)
                } else {
                    lastBufferedDiagnostic = nil
                }
            case .repeated(let raw, let occurrences):
                let repeated = raw + " · " + localized("log.repeated_occurrences", String(occurrences))
                if let buffered = lastBufferedDiagnostic,
                   buffered.line == raw, buffered.index == displayLines.count - 1 {
                    displayLines[buffered.index] = repeated
                } else if !replacePresentation,
                          let range = trailingLogLineRange(raw, in: logView.string as NSString),
                          let storage = logView.textStorage {
                    storage.replaceCharacters(in: range, with: styledLog(repeated))
                    presentationChanged = true
                } else {
                    diagnosticLogFolder.reset()
                    displayLines.append(raw)
                    lastBufferedDiagnostic = nil
                }
            }
        }
        let displayText = displayLines.joined(separator: "\n")
        if latestChanged { refreshLatestDiagnostics() }
        var diagnosticsChanged = false
        for line in text.split(separator: "\n") {
            if photosDiagnostics.ingest(String(line)) { diagnosticsChanged = true }
        }
        if diagnosticsChanged { refreshDiagnostics() }
        for line in text.split(separator: "\n") where line.hasPrefix("MFB_LOG_DIRECTORY=") {
            let encoded = Data(line.dropFirst("MFB_LOG_DIRECTORY=".count).utf8)
            if let path = try? JSONDecoder().decode(String.self, from: encoded), path.hasPrefix("/") {
                resolvedHistoryDirectory = URL(fileURLWithPath: path, isDirectory: true)
                preferences.set(path, forKey: historyPreferenceKey)
                historyButton.toolTip = path
            }
        }
        if text.contains("MFB_RESUME_DECISION_REQUIRED") { sawResumeDecision = true }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if let value = countStatusValue(in: String(line)) {
                countStatus = value
                refreshCountStatus()
            }
        }
        guard !displayText.isEmpty || presentationChanged else { return }
        let storage = logView.textStorage!
        if !displayText.isEmpty {
            if replacePresentation { storage.setAttributedString(NSAttributedString(string: "")) }
            if !storage.string.isEmpty { storage.append(NSAttributedString(string: "\n")) }
            storage.append(styledLog(displayText))
            let lines = storage.string.split(separator: "\n", omittingEmptySubsequences: false)
            if lines.count > 3_000 {
                let retained = lines.suffix(3_000).joined(separator: "\n")
                storage.setAttributedString(styledLog(retained))
            }
        }
        logView.scrollToEndOfDocument(nil)
    }

    private func styledLog(_ text: String) -> NSAttributedString {
        let result = NSMutableAttributedString(string: "")
        for (index, line) in text.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
            if index > 0 { result.append(NSAttributedString(string: "\n")) }
            let tone = LogTone.classify(String(line))
            result.append(NSAttributedString(string: String(line), attributes: [
                .foregroundColor: tone.color, .font: tone.font,
            ]))
        }
        return result
    }

    private func refreshCountStatus() {
        guard let countStatus else { countStatusLabel.isHidden = true; return }
        countStatusLabel.stringValue = localized("status.count", countStatus)
        countStatusLabel.textColor = countStatus.uppercased() == "MATCH" ? .systemTeal : .systemRed
        countStatusLabel.isHidden = false
    }

    private func refreshLatestDiagnostics() {
        for (label, value, key) in [
            (latestFailureLabel, latestDiagnostics.failure, "log.latest_failure"),
            (latestWarningLabel, latestDiagnostics.warning, "log.latest_warning"),
        ] {
            label.isHidden = value == nil
            label.stringValue = value.map { localized(key, $0) } ?? ""
            label.toolTip = label.stringValue
            label.setAccessibilityLabel(label.stringValue)
        }
        latestDiagnosticsRow.isHidden = latestDiagnostics.failure == nil && latestDiagnostics.warning == nil
    }

    private func refreshFileProgress(now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        if logPhase.phase == .verification, gateActivity.event != nil || gateActivity.invalid {
            fileProgressRow.isHidden = false
            fileProgressIndicator.isHidden = true
            fileProgressIndicator.stopAnimation(nil)
            let label = gateActivity.label(now: now)
            fileProgressLabel.stringValue = label
            fileProgressLabel.toolTip = label
            fileProgressLabel.setAccessibilityLabel(label)
            fileProgressLabel.textColor = gateActivity.event?.state == "failed" ? .systemRed : .secondaryLabelColor
            return
        }
        fileProgressIndicator.isHidden = false
        fileProgressLabel.textColor = .labelColor
        fileProgressRow.isHidden = logPhase.phase == .verification || (fileProgress.event == nil && !fileProgress.invalid)
        fileProgressLabel.stringValue = fileProgress.label
        fileProgressLabel.toolTip = fileProgress.label
        fileProgressIndicator.isIndeterminate = fileProgress.invalid || fileProgress.event?.percentage == nil
            || fileProgress.event?.state == "finished"
        fileProgressIndicator.doubleValue = fileProgress.event?.percentage ?? 0
        fileProgressIndicator.setAccessibilityLabel(fileProgress.label)
        if fileProgressIndicator.isIndeterminate && !configurationControlsEnabled {
            fileProgressIndicator.startAnimation(nil)
        } else {
            fileProgressIndicator.stopAnimation(nil)
        }
    }

    private func applyResumeDecision(fresh: Bool, to request: inout ProcessorRequest) {
        request.resume = !fresh
        request.fresh = fresh
        request.retry = !fresh && (request.retry || request.operationMode.backendMode == "fast-img")
        resumeCheck.state = request.resume ? .on : .off
        freshCheck.state = request.fresh ? .on : .off
        retryCheck.state = request.retry ? .on : .off
        for control in [resumeCheck, freshCheck, retryCheck] { saveOption(control) }
    }

    private func processingCompleted(_ result: Result<String, Error>) {
        let countSummary = batchResults.summary.map { "\n[RESULT] \($0)" } ?? ""
        if host.controlState == "cancelled", host.lastExitStatus == 130 {
            photosDiagnostics.markFailed()
            refreshDiagnostics()
            setProcessing(false)
            if var retry = lastRequest, retry.operationMode.capabilities.supportsResume {
                applyResumeDecision(fresh: false, to: &retry)
                lastRequest = retry
            }
            setLocalizedStatus("status.stopped")
            appendLog(localized("status.stopped") + countSummary)
            if closeWhenFinished { NSApp.terminate(nil) }
            return
        }
        switch result {
        case let .success(message):
            setProcessing(false)
            if batchResults.hasFailure {
                setLocalizedStatus(batchResults.hasPartialSuccess ? "result.partially_completed"
                    : (batchResults.hasFileFailures ? "result.finished_with_failures" : "result.stopped_with_error"))
                appendLog("[FAIL] \(statusLabel.stringValue)" + countSummary,
                          captureDiagnostics: latestDiagnostics.failure == nil)
            } else if batchResults.isIncomplete
                || (batchResults.hasUnprocessed && host.controlState != "paused")
                || (batchResults.totals.isEmpty
                && lastRequest.map { !$0.dryRun && [.adjacent, .fastImgJxl, .fastImgAvif, .fastVid].contains($0.operationMode) } == true) {
                setLocalizedStatus(batchResults.isIncomplete || batchResults.totals.isEmpty
                    ? "result.incomplete" : "result.unfinished")
                appendLog("[WARN] \(statusLabel.stringValue)" + countSummary,
                          captureDiagnostics: latestDiagnostics.warning == nil)
            } else {
                setStatus(message)
                appendLog("[DONE] \(message)" + countSummary)
            }
        case let .failure(error):
            photosDiagnostics.markFailed()
            refreshDiagnostics()
            let failureMessage = batchResults.hasPartialSuccess
                ? localized("result.partially_completed") + " · " + error.localizedDescription
                : error.localizedDescription
            appendLog("[FAIL] \(failureMessage)" + countSummary,
                      captureDiagnostics: latestDiagnostics.failure == nil)
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
                    setStatus(error.localizedDescription)
                    return
                }
                lastRequest = retry
                sawResumeDecision = false
                batchResults = BatchResults()
                logPhase = PhaseLogPresentation()
                diagnosticLogFolder.reset()
                latestDiagnostics = LatestDiagnostics()
                refreshLatestDiagnostics()
                fileProgress = FileStageProgress()
                gateActivity = GateCheckActivity()
                refreshFileProgress()
                photosDiagnostics.reset()
                refreshDiagnostics()
                setProcessing(true)
                host.startProcessing(retry)
            } else {
                setProcessing(false)
                if batchResults.hasPartialSuccess { setLocalizedStatus("result.partially_completed") }
                else if batchResults.hasFileFailures { setLocalizedStatus("result.finished_with_failures") }
                else { setStatus(error.localizedDescription) }
            }
        }
    }

    @objc private func showSettings() {
        guard configurationControlsEnabled else { return }
        settingsPanel = MediaSettingsPanel(preferences: preferences, developer: developerMode,
            fast: selectedOperation.backendMode == "fast-img",
            videos: selectedOperation == .adjacent ? processingPopup.indexOfSelectedItem == 2
                : selectedOperation.backendMode != "fast-img") { [weak self] in
            self?.configurationChanged()
        }
        settingsPanel?.show(for: window)
    }

    @objc private func openHelp() {
        if let url = URL(string: "https://github.com/nowaytouse/modern-format-boost#readme") { NSWorkspace.shared.open(url) }
    }

    @objc private func repairCore() {
        guard configurationControlsEnabled, processorStatus != .ready else { return }
        if developerMode {
            let chooser = NSOpenPanel()
            chooser.canChooseDirectories = false
            chooser.allowsMultipleSelection = false
            chooser.beginSheetModal(for: window) { [weak self] response in
                guard response == .OK, let self, let url = chooser.url else { return }
                guard FileManager.default.isExecutableFile(atPath: url.path) else {
                    self.present(HostError(message: localized("status.processor_unavailable")))
                    return
                }
                UserDefaults.standard.set(url.path, forKey: "MFBGuiCoreBinary")
                self.show()
            }
        } else if let url = URL(string: "https://github.com/nowaytouse/modern-format-boost/releases/tag/nightly") {
            NSWorkspace.shared.open(url)
        }
    }

    @objc private func languageChanged() {
        let language = AppLanguage.allCases[safe: languagePopup.indexOfSelectedItem] ?? .system
        LocalizationCatalog.shared.select(language)
        applyLocalization()
        (NSApp.delegate as? AppDelegate)?.configureMenus()
        updateMetadataSafetyNotice()
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
        developerCheck.state = preferences.bool(forKey: developerPreferenceKey) ? .on : .off
        updateOptionTitles()
        refreshOperationPopup()
    }

    private func replaceTitles(_ popup: NSPopUpButton, with titles: [String]) {
        let selected = max(0, popup.indexOfSelectedItem)
        popup.removeAllItems()
        popup.addItems(withTitles: titles)
        popup.selectItem(at: min(selected, max(0, titles.count - 1)))
    }

    private func applyLocalization() {
        aboutIcon.toolTip = localized("menu.about")
        aboutIcon.setAccessibilityLabel(localized("menu.about"))
        subtitleLabel.stringValue = localized("app.subtitle")
        targetField.placeholderString = localized("field.target.placeholder")
        backupField.placeholderString = localized("field.backup.placeholder")
        backupLabel.stringValue = localized("field.backup")
        mediaLabel.stringValue = localized("field.media")
        operationLabel.stringValue = localized("field.operation")
        photosScopeLabel.stringValue = localized("field.photos_scope")
        languageLabel.stringValue = localized("field.language")
        appearanceLabel.stringValue = localized("field.appearance")
        developerCheck.title = localized("option.developer")
        chooseButton.title = localized("button.choose")
        backupButton.title = localized("button.choose")
        openButton.title = localized("button.open_terminal")
        copyButton.title = localized("button.copy_command")
        runButton.title = localized("button.run")
        historyButton.title = localized("button.history")
        settingsButton.toolTip = localized("settings.title")
        settingsButton.setAccessibilityLabel(localized("settings.title"))
        helpButton.toolTip = localized("button.help")
        helpButton.setAccessibilityLabel(localized("button.help"))
        diagnosticsButton.toolTip = localized("button.photos_diagnostics")
        diagnosticsButton.setAccessibilityLabel(localized("button.photos_diagnostics"))
        diagnosticsPanel?.title = localized("button.photos_diagnostics")
        historyButton.toolTip = resolvedHistoryDirectory.path
        pauseButton.title = localized(host.controlState == "paused" ? "button.resume" : "button.pause")
        stopButton.title = localized("button.stop")
        statusLabel.toolTip = localized("status.core_help")
        updateOptionTitles()
        commandField.placeholderString = localized("command.placeholder")
        replaceTitles(processingPopup, with: [
            localized("media.both"), localized("media.images"), localized("media.videos"),
        ])
        refreshOperationPopup()
        replaceTitles(languagePopup, with: AppLanguage.allCases.map(\.nativeTitle))
        replaceTitles(appearancePopup, with: AppAppearance.allCases.map(\.localizedTitle))
        refreshCountStatus()
        refreshFileProgress()
        refreshLatestDiagnostics()
        refreshDiagnostics()
        refreshStatus()
        refreshProcessingStatus()
    }

    private func updateOptionTitles() {
        for (control, key, flag) in [
            (ultimateCheck, "option.ultimate", "--ultimate"),
            (verboseCheck, "option.verbose", "--verbose"),
            (shortestPathCheck, "option.shortest_path", "--shortest-path"),
            (resumeCheck, "option.resume", "--resume"),
            (freshCheck, "option.fresh", "--no-resume"),
            (archiveCheck, "option.archive", "--archive"),
            (retryCheck, "option.retry", "--retry"),
            (forceCheck, "option.force", "--force"),
            (dryRunCheck, "option.dry_run", "--dry-run"),
            (plainCheck, "option.plain", "--plain"),
            (inPlaceCheck, "option.in_place", "--in-place"),
            (watchCheck, "option.watch", "--watch"),
        ] { control.title = localized(key) + (developerMode ? " (\(flag))" : "") }
        for (control, key) in [
            (ultimateCheck, "option.ultimate.help"), (verboseCheck, "option.verbose.help"),
            (shortestPathCheck, "option.shortest_path.help"), (resumeCheck, "option.resume.help"),
            (freshCheck, "option.fresh.help"), (archiveCheck, "option.archive.help"),
            (retryCheck, "option.retry.help"), (forceCheck, "option.force.help"),
            (dryRunCheck, "option.dry_run.help"), (plainCheck, "option.plain.help"),
            (inPlaceCheck, "option.in_place.help"), (watchCheck, "option.watch.help"),
        ] { control.toolTip = localized(key) }
    }

    private func refreshOperationPopup() {
        let selected = selectedOperation
        let keys = [
            "adjacent", "fast_jxl", "fast_avif", "fast_video", "restore_jpeg", "collect",
            "compare", "merge_xmp", "icloud_import", "diagnostic", "cache_clean", "database",
        ]
        operationPopup.removeAllItems()
        for (operation, key) in zip(OperationMode.allCases, keys)
            where developerMode || !operation.developerOnly {
            operationPopup.addItem(withTitle: localized("operation.\(key)"))
            operationPopup.lastItem?.representedObject = operation.rawValue
        }
        let retained = operationPopup.itemArray.first {
            ($0.representedObject as? String) == selected.rawValue
        }
        operationPopup.select(retained ?? operationPopup.itemArray.first)
        if selectedOperation != selected { restoreOptions(for: selectedOperation) }
    }

    private func setProcessing(_ processing: Bool) {
        configurationControlsEnabled = !processing
        (window.contentView as? NativeDropView)?.acceptsDrops = !processing
        for control in [
            chooseButton, backupButton, operationPopup, developerCheck, openButton, copyButton, runButton,
            languagePopup, appearancePopup, settingsButton,
        ] {
            control.isEnabled = !processing
        }
        applyCapabilityState()
        pauseButton.isEnabled = processing && host.controlState != "cancelled"
        stopButton.isEnabled = processing && host.controlState != "cancelled"
        pauseButton.title = localized("button.pause")
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
            fileProgress = FileStageProgress()
            gateActivity = GateCheckActivity()
            refreshFileProgress()
        }
    }

    private func validateGateActivityForSelfTest() throws {
        let oldPhase = logPhase
        let oldActivity = gateActivity
        let oldWarnings = latestDiagnostics
        let oldLog = logView.string
        defer {
            logPhase = oldPhase
            gateActivity = oldActivity
            latestDiagnostics = oldWarnings
            logView.string = oldLog
            refreshLatestDiagnostics()
            refreshFileProgress()
        }
        let phase = #"MFB_LOG_PHASE={"schema_version":1,"phase":"verification"}"#
        let activity = #"MFB_GATE_ACTIVITY={"schema_version":1,"stage_id":"unit-gate","gate":"local","check":"metadata","completed":3,"total":6,"state":"running"}"#
        appendLog(phase + "\n" + activity, now: 10)
        refreshFileProgress(now: 75)
        guard !fileProgressRow.isHidden, fileProgressIndicator.isHidden,
              fileProgressLabel.stringValue == localized("gate.activity.running", localized("gate.check.metadata"), "3", "6")
                + " · " + localized("gate.activity.updated", "65"),
              !logView.string.contains("MFB_GATE_ACTIVITY="), fileProgressLabel.accessibilityLabel() == fileProgressLabel.stringValue else {
            throw HostError(message: "Gate activity fabricated file percentages, lost elapsed receipt age or accessibility")
        }
        let failed = activity.replacingOccurrences(of: "\"completed\":3", with: "\"completed\":4")
            .replacingOccurrences(of: "\"state\":\"running\"", with: "\"state\":\"failed\"")
        appendLog(failed, now: 80)
        guard gateActivity.event?.state == "failed", gateActivity.event?.completed == 4,
              fileProgressLabel.stringValue == localized("gate.activity.failed", localized("gate.check.metadata"), "4", "6", "2") else {
            throw HostError(message: "Gate failure hid unexecuted checks or claimed six completed checks")
        }
        appendLog(activity, now: 81)
        guard gateActivity.invalid, fileProgressIndicator.isHidden,
              fileProgressLabel.stringValue == localized("gate.activity.invalid"),
              latestDiagnostics.warning == "[WARN] " + localized("gate.activity.invalid") else {
            throw HostError(message: "Contradictory Gate activity was silently accepted")
        }
        appendLog(#"MFB_LOG_PHASE={"schema_version":1,"phase":"processing"}"#, now: 90)
        guard gateActivity.event == nil, !gateActivity.invalid else {
            throw HostError(message: "A new processing phase retained obsolete verification activity")
        }
        gateActivity = GateCheckActivity()
        _ = gateActivity.ingest(activity, now: 100)
        setProcessing(false)
        guard gateActivity.event == nil, !gateActivity.invalid else {
            throw HostError(message: "Ending a run retained live Gate activity")
        }
    }

    private func validateStatusLocalizationForSelfTest() throws {
        let catalog = LocalizationCatalog.shared
        let language = catalog.language
        let previousProcessor = processorStatus
        let previousDisplay = statusDisplay
        let previousStartedAt = processingStartedAt
        let previousTarget = targetField.stringValue
        let previousBackup = backupField.stringValue
        let previousCommand = commandField.stringValue
        let optionStates = optionControls.map { $0.0.state }
        defer {
            processorStatus = previousProcessor
            statusDisplay = previousDisplay
            processingStartedAt = previousStartedAt
            targetField.stringValue = previousTarget
            backupField.stringValue = previousBackup
            commandField.stringValue = previousCommand
            languagePopup.selectItem(at: AppLanguage.allCases.firstIndex(of: language) ?? 0)
            languageChanged()
        }
        targetField.stringValue = "/synthetic/language-draft"
        backupField.stringValue = "/synthetic/language-backup"
        commandField.stringValue = "synthetic command preview"
        func changeLanguage(_ language: AppLanguage) {
            languagePopup.selectItem(at: AppLanguage.allCases.firstIndex(of: language)!)
            languageChanged()
        }
        for language in [AppLanguage.english, .simplifiedChinese, .japanese] {
            changeLanguage(language)
            let repeatLabel = localized("log.repeated_occurrences", "3")
            guard repeatLabel != "log.repeated_occurrences", repeatLabel.contains("3") else {
                throw HostError(message: "Repeated-log count was not localized for every language")
            }
            for (state, key) in [(ProcessorReadiness.checking, "status.checking_processor"),
                                 (.ready, "status.processor_ready"), (.unavailable, "status.processor_unavailable")] {
                processorStatus = state
                statusDisplay = .processor
                changeLanguage(language)
                guard statusLabel.stringValue == localized(key), processorStatus == state,
                      aboutIcon.toolTip == localized("menu.about"), aboutIcon.accessibilityLabel() == localized("menu.about") else {
                    throw HostError(message: "Language change retained stale core readiness wording or changed readiness")
                }
            }
            processorStatus = .failed(diagnostic: "synthetic original failure", omitted: true)
            changeLanguage(language)
            guard statusLabel.stringValue == localized("status.processor_failed") + ": synthetic original failure\n"
                + localized("log.omitted", UInt64(1)), processorStatus != .ready else {
                throw HostError(message: "Language change lost the core failure evidence or relabeled it ready")
            }
            setStatus("synthetic original diagnostic")
            changeLanguage(language)
            guard statusLabel.stringValue == "synthetic original diagnostic" else {
                throw HostError(message: "Language change replaced an actionable error with core readiness")
            }
            for key in ["status.ready", "result.partially_completed", "result.finished_with_failures", "status.stopped"] {
                setLocalizedStatus(key)
                changeLanguage(language)
                guard statusLabel.stringValue == localized(key) else {
                    throw HostError(message: "Language change reset a completed, failed, or stopped status")
                }
            }
        }
        processingStartedAt = ProcessInfo.processInfo.systemUptime - 65
        let startedAt = processingStartedAt
        changeLanguage(.japanese)
        let elapsed = max(0, Int(ProcessInfo.processInfo.systemUptime - startedAt!))
        guard statusLabel.stringValue == localized("status.running_elapsed", elapsed / 60, elapsed % 60),
              processingStartedAt == startedAt, targetField.stringValue == "/synthetic/language-draft",
              backupField.stringValue == "/synthetic/language-backup", commandField.stringValue == "synthetic command preview",
              optionControls.map({ $0.0.state }) == optionStates else {
            throw HostError(message: "Language change reset running state or the user's command draft")
        }
    }

    private func validateDiagnosticFoldingForSelfTest() throws {
        var folder = ConsecutiveDiagnosticLogFolder()
        let first = "[WARN] /tmp/clip-1.mov frame=17"
        let otherPath = "[WARN] /tmp/clip-2.mov frame=17"
        let otherNumber = "[WARN] /tmp/clip-1.mov frame=18"
        guard folder.ingest(first) == .append(first),
              folder.ingest(first) == .repeated(first, occurrences: 2),
              folder.ingest(otherPath) == .append(otherPath),
              folder.ingest(otherNumber) == .append(otherNumber),
              folder.ingest("routine detail") == .append("routine detail"),
              folder.ingest(otherNumber) == .append(otherNumber) else {
            throw HostError(message: "Diagnostic folding merged distinct lines or crossed a normal log line")
        }
        folder.reset()
        guard folder.ingest(first) == .append(first) else {
            throw HostError(message: "Diagnostic folding did not reset for a new run")
        }

        let unicodeLine = "[ERROR] 🚀 /tmp/映像-7.mov frame=71"
        let retained = NSMutableAttributedString(string: "trimmed history 🧪\n" + unicodeLine)
        guard let range = trailingLogLineRange(unicodeLine, in: retained.string as NSString) else {
            throw HostError(message: "UTF-16 tail range did not find a retained Unicode diagnostic")
        }
        let summary = unicodeLine + " · " + localized("log.repeated_occurrences", "2")
        retained.replaceCharacters(in: range, with: styledLog(summary))
        guard retained.string.hasSuffix(summary),
              trailingLogLineRange(unicodeLine, in: retained.string as NSString) != nil,
              trailingLogLineRange(unicodeLine, in: NSString(string: "trimmed unrelated tail")) == nil else {
            throw HostError(message: "Trimmed-log replacement lost its row or matched an unrelated tail")
        }

        let oldLog = NSAttributedString(attributedString: logView.textStorage!)
        let oldFolder = diagnosticLogFolder
        let oldPhase = logPhase
        let oldLatest = latestDiagnostics
        let oldBatch = batchResults
        let oldProgress = fileProgress
        let oldGate = gateActivity
        let oldPhotosDiagnostics = photosDiagnostics
        let oldCountStatus = countStatus
        let oldCountStatusText = countStatusLabel.stringValue
        let oldCountStatusHidden = countStatusLabel.isHidden
        defer {
            logView.textStorage?.setAttributedString(oldLog)
            diagnosticLogFolder = oldFolder
            logPhase = oldPhase
            latestDiagnostics = oldLatest
            batchResults = oldBatch
            fileProgress = oldProgress
            gateActivity = oldGate
            photosDiagnostics = oldPhotosDiagnostics
            countStatus = oldCountStatus
            countStatusLabel.stringValue = oldCountStatusText
            countStatusLabel.isHidden = oldCountStatusHidden
            refreshLatestDiagnostics()
            refreshFileProgress()
            refreshDiagnostics()
        }

        clearBatchLog()
        appendLog((0..<3_001).map { "detail \($0)" }.joined(separator: "\n"))
        let error = "[ERROR] 🚀 /tmp/映像-7.mov frame=71"
        appendLog(error + "\n" + error)
        let repeatedTwo = error + " · " + localized("log.repeated_occurrences", "2")
        guard logView.string.split(separator: "\n", omittingEmptySubsequences: false).count <= 3_000,
              logView.string.components(separatedBy: "\n").filter({ $0 == repeatedTwo }).count == 1,
              latestDiagnostics.failure == LogTone.message(error),
              !batchResults.hasFailure, batchResults.totals.isEmpty else {
            throw HostError(message: "Consecutive diagnostics were not folded safely after log trimming")
        }
        appendLog(error)
        let repeatedThree = error + " · " + localized("log.repeated_occurrences", "3")
        guard logView.string.components(separatedBy: "\n").filter({ $0 == repeatedThree }).count == 1,
              latestDiagnostics.failure == LogTone.message(error),
              !batchResults.hasFailure, batchResults.totals.isEmpty else {
            throw HostError(message: "Repeated rows across append chunks changed diagnostics or batch counts")
        }
        appendLog("ordinary separator")
        appendLog(error)
        guard logView.string.components(separatedBy: "\n").filter({ $0 == error }).count == 1 else {
            throw HostError(message: "A non-diagnostic row did not break consecutive folding")
        }
        appendLog("[Summary] succeeded=0 failed=1")
        guard latestDiagnostics.failure == LogTone.message(error) else {
            throw HostError(message: "A repeated diagnostic or terminal summary replaced the actionable error")
        }

        logPhase = PhaseLogPresentation(now: 0)
        diagnosticLogFolder.reset()
        logView.string = ""
        let processing = #"MFB_LOG_PHASE={"schema_version":1,"phase":"processing"}"#
        let verification = #"MFB_LOG_PHASE={"schema_version":1,"phase":"verification"}"#
        appendLog(processing, now: 3)
        appendLog(error + "\n" + error, now: 3)
        appendLog(verification, now: 4)
        appendLog(error, now: 4)
        let visibleErrors = logView.string.components(separatedBy: "\n").filter {
            $0 == error || $0 == repeatedTwo
        }
        guard visibleErrors == [repeatedTwo, error] else {
            throw HostError(message: "A phase presentation transition did not reset diagnostic folding")
        }

        clearBatchLog()
        appendLog(error)
        guard !logView.string.contains(localized("log.repeated_occurrences", "2")) else {
            throw HostError(message: "A new batch retained the previous batch's repeat count")
        }
        clearBatchLog()
        let fileFailure = HistoryFileFailure(schema_version: 1, pipeline: "img", source_path: "/synthetic/file.jpg",
            absolute_path: "/synthetic/file.jpg", message: "synthetic decoder failed", run_id: "run-synthetic",
            invocation_id: "invocation-synthetic", reason_code: nil)
        let marker = HistoryFileFailure.prefix + String(decoding: try JSONEncoder().encode(fileFailure), as: UTF8.self)
        func largeFailure(_ bytes: Int) -> HistoryFileFailure {
            HistoryFileFailure(schema_version: 1, pipeline: "img", source_path: "synthetic.jpg", absolute_path: nil,
                message: String(repeating: "x", count: bytes), run_id: "run", invocation_id: "invocation", reason_code: nil)
        }
        let overhead = try JSONEncoder().encode(largeFailure(0)).count
        let messageBudget = maxProcessLogChunkBytes - HistoryFileFailure.prefix.utf8.count - 1 - overhead
        let largest = HistoryFileFailure.prefix + String(decoding: try JSONEncoder().encode(largeFailure(messageBudget)), as: UTF8.self)
        var transport = Data((largest + "\n").utf8)
        guard transport.count == maxProcessLogChunkBytes,
              drainProcessLogChunks(&transport, flush: false) == [largest], transport.isEmpty,
              try HistoryFileFailure.decode(largest) != nil else {
            throw HostError(message: "File failure wire budget split a valid JSON observation")
        }
        let tooLarge = HistoryFileFailure.prefix + String(decoding: try JSONEncoder().encode(largeFailure(messageBudget + 1)), as: UTF8.self)
        let oversizedResult = Swift.Result { try HistoryFileFailure.decode(tooLarge) }
        guard case .failure = oversizedResult else { throw HostError(message: "Oversized file failure wire payload was accepted") }
        appendLog(marker)
        let visibleFailure = "[FAIL] /synthetic/file.jpg: synthetic decoder failed"
        guard latestDiagnostics.failure == visibleFailure,
              developerMode || !logView.string.contains(HistoryFileFailure.prefix),
              !batchResults.hasFailure, batchResults.totals.isEmpty else {
            throw HostError(message: "File failure marker appeared as raw JSON or changed authoritative media counts")
        }
        appendLog(marker)
        guard logView.string.components(separatedBy: "\n").filter({ $0 == visibleFailure }).count == 2 else {
            throw HostError(message: "File observation presentation was folded by diagnostic text instead of exact observation identity")
        }
        appendLog("MFB_FILE_FAILURE={invalid}")
        guard latestDiagnostics.warning == "[WARN] " + localized("history.batch_issues.invalid"),
              latestDiagnostics.failure == visibleFailure else {
            throw HostError(message: "Malformed file failure marker hid the previous actionable error")
        }
    }

    func validateInterfaceForSelfTest() throws {
        try validateDiagnosticFoldingForSelfTest()
        try validateStatusLocalizationForSelfTest()
        try validateGateActivityForSelfTest()
        let previousAppearance = NSApp.appearance
        var logBackgrounds: [NSColor] = []
        for name in [NSAppearance.Name.aqua, .darkAqua] {
            guard let appearance = NSAppearance(named: name) else {
                throw HostError(message: "Missing native log appearance")
            }
            NSApp.appearance = appearance
            appearance.performAsCurrentDrawingAppearance {
                if let color = logView.backgroundColor.usingColorSpace(.sRGB) { logBackgrounds.append(color) }
            }
        }
        NSApp.appearance = previousAppearance
        guard logBackgrounds.count == 2, logBackgrounds.allSatisfy({ $0.alphaComponent == 1 }),
              !logBackgrounds[0].isEqual(logBackgrounds[1]) else {
            throw HostError(message: "Log background lost native light/dark contrast")
        }
        guard let content = window.contentView else { throw HostError(message: "Missing content view") }
        guard !developerMode, copyButton.isHidden, openButton.isHidden, diagnosticsButton.isHidden,
              commandField.isHidden, watchCheck.isHidden,
              !watchCheck.isEnabled, operationPopup.itemArray.count == 6,
              !verboseCheck.isEnabled,
              verboseCheck.state == .on, archiveCheck.state == .on, freshCheck.state == .on,
              !verboseCheck.title.contains("--verbose"), !freshCheck.title.contains("--no-resume")
        else { throw HostError(message: "Default options or developer gating failed") }
        content.layoutSubtreeIfNeeded()
        guard logScroll.frame.height >= 260,
              targetField.frame.width >= content.bounds.width * 0.65,
              operationPopup.frame.width >= content.bounds.width * 0.65,
              window.title.contains(appVersion) else {
            throw HostError(message: "Main form width, log area or visible version regressed")
        }
        appendLog("previous batch sentinel")
        let progressLine = #"MFB_PROGRESS={"schema_version":1,"stage_id":"synthetic-1","stage":"fast_img_encode","processed":1,"total":3,"state":"running"}"#
        appendLog(progressLine)
        content.layoutSubtreeIfNeeded()
        guard !fileProgressRow.isHidden, !fileProgressIndicator.isIndeterminate,
              abs(fileProgressIndicator.doubleValue - 100.0 / 3) < 0.001,
              !logView.string.contains("MFB_PROGRESS="), batchResults.totals.isEmpty,
              content.bounds.contains(content.convert(fileProgressRow.bounds, from: fileProgressRow)),
              logScroll.frame.height >= 260 else {
            throw HostError(message: "File progress layout or separation from outcomes failed")
        }
        appendLog(progressLine.replacingOccurrences(of: "running", with: "finished"))
        guard fileProgressIndicator.isIndeterminate, fileProgress.event?.processed == 1,
              batchResults.totals.isEmpty else {
            throw HostError(message: "An incomplete stage was shown as batch completion")
        }
        clearBatchLog()
        guard fileProgressRow.isHidden, fileProgress.event == nil,
              !logView.string.contains("previous batch sentinel"),
              logView.string.contains(initialHistoryDirectory(preferences: preferences).path) else {
            throw HostError(message: "Batch logs were not cleared with a history location")
        }
        appendLog("MFB_LOG_DIRECTORY=\"/tmp/backend-resolved-logs\"")
        guard resolvedHistoryDirectory.path == "/tmp/backend-resolved-logs",
              historyButton.toolTip == "/tmp/backend-resolved-logs" else {
            throw HostError(message: "History folder ignored the backend's resolved directory")
        }
        appendLog("INF routine detail\n[CHECK] Integrity summary\n    Count status:    MATCH\n[Summary] succeeded=4 failed=0")
        guard countStatusLabel.stringValue == localized("status.count", "MATCH"),
              countStatusLabel.textColor == .systemTeal,
              !countStatusLabel.isHidden,
              logView.string.contains("INF routine detail"),
              logView.string.contains("[Summary] succeeded=4 failed=0"),
              LogTone.classify("INF routine detail") == .muted,
              LogTone.classify("[CHECK] Integrity summary") == .stage,
              LogTone.classify("ERR: [VERIFY  ] original custody") == .stage,
              LogTone.classify("ERR: Count status: MATCH") == .result,
              LogTone.classify("Integrity: CLEAN") == .result,
              LogTone.classify("[Summary] succeeded=4 failed=0") == .result,
              LogTone.classify("ERR: Permission denied") == .failure,
              LogTone.classify("ERR: frame=20 fps=10") == .normal,
              LogTone.classify("ERR: [WARN   ] metadata could not be copied") == .warning,
              LogTone.classify("\u{1B}[31mError: encoder unavailable\u{1B}[0m") == .failure,
              LogTone.classify("[GATE 2] verification FAIL") == .failure,
              LogTone.classify("Converted /tmp/failed-file.jpg") == .normal,
              LogTone.classify("Converted /tmp/failed=1.jpg") == .normal
        else { throw HostError(message: "Log priority or MATCH status presentation failed") }
        let summaryOffset = (logView.string as NSString).range(of: "[Summary] succeeded=4 failed=0").location
        guard summaryOffset != NSNotFound,
              logView.textStorage?.attribute(.foregroundColor, at: summaryOffset, effectiveRange: nil) as? NSColor
                  == .systemTeal else {
            throw HostError(message: "Summary log color was not rendered")
        }
        appendLog("ERR: Permission denied\n    Count status:    MISMATCH\n[Summary] succeeded=3 failed=1")
        guard countStatusLabel.stringValue == localized("status.count", "MISMATCH"),
              countStatusLabel.textColor == .systemRed,
              LogTone.classify("[Summary] succeeded=3 failed=1") == .failure,
              logView.string.contains("ERR: Permission denied"),
              logView.string.contains("    Count status:    MATCH"),
              latestDiagnostics.failure == "Permission denied"
        else { throw HostError(message: "Non-MATCH status hid preceding diagnostics") }
        appendLog("ERR: [WARN] synthetic metadata warning\nERR: frame=20 fps=10\nError: exiting with failures")
        appendLog(progressLine)
        content.layoutSubtreeIfNeeded()
        guard latestDiagnostics.failure == "Permission denied",
              latestDiagnostics.warning == "[WARN] synthetic metadata warning",
              !latestDiagnosticsRow.isHidden, !latestFailureLabel.isHidden, !latestWarningLabel.isHidden,
              latestFailureLabel.stringValue == localized("log.latest_failure", "Permission denied"),
              content.bounds.contains(content.convert(latestDiagnosticsRow.bounds, from: latestDiagnosticsRow)),
              logScroll.frame.height >= 260,
              batchResults.totals.isEmpty else {
            throw HostError(message: "Latest diagnostics lost their cause, altered counts or overflowed the main layout")
        }
        processingCompleted(.failure(HostError(message: "Worker exited with code 1")))
        guard latestDiagnostics.failure == "Permission denied",
              logView.string.contains("Worker exited with code 1") else {
            throw HostError(message: "Completion boilerplate replaced the actionable error")
        }
        var boundedDiagnostics = LatestDiagnostics()
        _ = boundedDiagnostics.ingest("[Summary] succeeded=0 failed=1")
        _ = boundedDiagnostics.ingest("[ERROR] " + String(repeating: "中", count: 5_000))
        guard boundedDiagnostics.failure?.unicodeScalars.count == 4_097,
              boundedDiagnostics.failure?.hasSuffix("…") == true else {
            throw HostError(message: "Latest diagnostic text was not Unicode-safe and bounded")
        }
        _ = boundedDiagnostics.ingest("Error: a later concrete failure")
        guard boundedDiagnostics.failure == "Error: a later concrete failure" else {
            throw HostError(message: "A later concrete failure was not shown")
        }
        let processingPhase = #"MFB_LOG_PHASE={"schema_version":1,"phase":"processing"}"#
        let verificationPhase = #"MFB_LOG_PHASE={"schema_version":1,"phase":"verification"}"#
        logPhase = PhaseLogPresentation(now: 0)
        appendLog(processingPhase, now: 3)
        appendLog(progressLine)
        appendLog("old processing detail\n"
            + #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":1,"failed":1,"ignored":0,"exit_code":0}"#
            + "\n" + verificationPhase + "\n[GATE 1] verification FAIL", now: 6)
        guard !logView.string.contains("old processing detail"), !logView.string.contains("MFB_LOG_PHASE="),
              logView.string.contains(localized("log.phase.verification")),
              logView.string.contains("[GATE 1] verification FAIL"), fileProgressRow.isHidden,
              batchResults.hasFileFailures, countStatus == "MISMATCH",
              latestDiagnostics.failure == "Permission denied",
              latestDiagnostics.warning == "[WARN] synthetic metadata warning" else {
            throw HostError(message: "Phase presentation lost results, warnings, count status or same-delivery output")
        }
        appendLog(verificationPhase + "\n[GATE 2] retained evidence\n"
            + verificationPhase + "\n[GATE 3] retained evidence", now: 9)
        guard logView.string.contains("[GATE 1] verification FAIL"),
              logView.string.contains("[GATE 2] retained evidence"),
              logView.string.contains("[GATE 3] retained evidence") else {
            throw HostError(message: "Repeated verification events cleared earlier gate evidence")
        }
        appendLog("MFB_LOG_PHASE={}\nfile MFB_LOG_PHASE=unrelated", now: 12)
        guard logView.string.contains("[GATE 1] verification FAIL"),
              logView.string.contains("file MFB_LOG_PHASE=unrelated"),
              logPhase.phase == .verification, batchResults.hasFileFailures,
              latestDiagnostics.warning == "[WARN] " + localized("log.phase.invalid") else {
            throw HostError(message: "Invalid or incidental phase text replaced valid output or outcomes")
        }
        clearBatchLog()
        guard countStatusLabel.isHidden, countStatus == nil, latestDiagnosticsRow.isHidden,
              latestDiagnostics.failure == nil, latestDiagnostics.warning == nil, logPhase.phase == nil else {
            throw HostError(message: "Previous batch count status leaked into the next batch")
        }
        appendLog(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":1,"failed":1,"ignored":0,"exit_code":0}"#)
        processingCompleted(.success(localized("status.completed")))
        guard statusLabel.stringValue == localized("result.finished_with_failures"),
              logView.string.contains("[FAIL] IMG:"), batchResults.hasFailure else {
            throw HostError(message: "GUI treated reported file failures as success")
        }
        clearBatchLog()
        appendLog("[ERROR] synthetic encoder failure\n"
            + #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":1,"failed":1,"ignored":0,"unprocessed":0,"exit_code":1}"#)
        processingCompleted(.failure(HostError(message: "synthetic exit=1")))
        guard statusLabel.stringValue == localized("result.partially_completed"), batchResults.hasFailure,
              latestDiagnostics.failure == "[ERROR] synthetic encoder failure",
              logView.string.contains("synthetic exit=1"),
              logView.string.contains("[RESULT] " + localized("result.counts", "3", "1", "1", "0")) else {
            throw HostError(message: "Partial completion hid the failure, exit status or final counts")
        }
        clearBatchLog()
        appendLog(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":1,"skipped":0,"failed":0,"ignored":0,"unprocessed":2,"exit_code":0}"#)
        guard logView.string.contains(localized("result.unprocessed", "2")), batchResults.hasUnprocessed else {
            throw HostError(message: "GUI did not report remaining unprocessed files")
        }
        processingCompleted(.success(localized("status.completed")))
        guard statusLabel.stringValue == localized("result.unfinished") else {
            throw HostError(message: "GUI reported success with unprocessed files")
        }
        clearBatchLog()
        guard batchResults.totals.isEmpty, batchResults.pendingTotals.isEmpty,
              !batchResults.hasFailure, !batchResults.hasUnprocessed else {
            throw HostError(message: "Batch counters leaked into a new run")
        }
        let attribution = try bundledLicenseText()
        guard attribution.contains("Modern Format Boost"), attribution.contains("Apache") else {
            throw HostError(message: "Bundled attribution is incomplete")
        }
        for controls in [[ultimateCheck, freshCheck, resumeCheck, dryRunCheck],
                         [shortestPathCheck, forceCheck, plainCheck, inPlaceCheck],
                         [verboseCheck, archiveCheck, retryCheck]] {
            let visible = controls.filter { !$0.isHidden }
            guard let first = visible.first else { continue }
            let x = first.convert(first.bounds, to: content).minX
            guard visible.allSatisfy({ abs($0.convert($0.bounds, to: content).minX - x) < 1 }) else {
                throw HostError(message: "Option column alignment drifted")
            }
        }
        developerCheck.state = .on
        developerModeChanged()
        guard operationPopup.itemArray.count == OperationMode.allCases.count,
               !copyButton.isHidden, !openButton.isHidden, !diagnosticsButton.isHidden,
               !watchCheck.isHidden, watchCheck.isEnabled,
              preferences.bool(forKey: developerPreferenceKey),
              verboseCheck.title.contains("--verbose"), freshCheck.title.contains("--no-resume")
        else { throw HostError(message: "Developer mode did not reveal advanced controls") }
        showDiagnostics()
        guard diagnosticsPanel?.isVisible == true,
              diagnosticsTextView.string.contains(localized("diagnostics.unknown")) else {
            throw HostError(message: "Developer Photos diagnostics panel did not open")
        }
        content.layoutSubtreeIfNeeded()
        let buttons = [ultimateCheck] + optionControls.map { $0.0 }
        for (index, button) in buttons.enumerated() where !button.isHidden {
            let rect = button.convert(button.bounds, to: content)
            guard content.bounds.contains(rect),
                  button.bounds.width + 1 >= button.intrinsicContentSize.width,
                  buttons.dropFirst(index + 1).filter({ !$0.isHidden }).allSatisfy({
                      !rect.intersects($0.convert($0.bounds, to: content))
                  })
            else { throw HostError(message: "Option clipped or overlapped: \(button.title)") }
        }
        developerCheck.state = .off
        developerModeChanged()
        guard operationPopup.itemArray.count == 6, diagnosticsButton.isHidden,
              diagnosticsPanel?.isVisible == false,
              watchCheck.isHidden, !watchCheck.isEnabled,
              !verboseCheck.title.contains("--verbose"), !freshCheck.title.contains("--no-resume") else {
            throw HostError(message: "Developer mode did not hide advanced controls")
        }
        targetField.stringValue = "/tmp/media"
        let defaultArguments = try ProcessorCommand.arguments(from: request())
        guard defaultArguments.contains("--verbose"), defaultArguments.contains("--archive"),
              !defaultArguments.contains("--no-resume"), !defaultArguments.contains("--resume"),
              !defaultArguments.contains("--retry"), !defaultArguments.contains("--watch")
        else { throw HostError(message: "Default checkbox flags disagree with the command") }
        let originalFrame = window.frame
        developerCheck.state = .on
        developerModeChanged()
        guard try ProcessorCommand.arguments(from: request()).contains("--no-resume") else {
            throw HostError(message: "Developer fresh-run selection was not forwarded")
        }
        let longPath = "/tmp/" + String(repeating: "long folder name/", count: 160)
        targetField.stringValue = longPath + "test.photoslibrary"
        backupField.stringValue = longPath + "backup"
        commandField.stringValue = longPath + "command"
        setStatus(longPath)
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

        developerCheck.state = .off
        developerModeChanged()
        operationPopup.selectItem(at: OperationMode.allCases.firstIndex(of: .fastImgJxl)!)
        operationChanged()
        guard ultimateCheck.state == .on, !ultimateCheck.isEnabled,
              archiveCheck.isEnabled, retryCheck.isEnabled, !forceCheck.isEnabled,
              verboseCheck.state == .on, archiveCheck.state == .on,
              shortestPathCheck.state == .on, freshCheck.state == .on
        else { throw HostError(message: "Incorrect option capabilities or defaults") }
        guard try ProcessorCommand.arguments(from: request()).contains("--shortest-path") else {
            throw HostError(message: "Fast-img Photos import default was not forwarded")
        }
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
        guard resumeCheck.state == .off, retryCheck.state == .off, freshCheck.state == .on else {
            throw HostError(message: "Fast-img resume alias stayed active after deselection")
        }
        operationPopup.selectItem(at: 0)
        operationChanged()
        guard freshCheck.state == .on, verboseCheck.state == .on else {
            throw HostError(message: "Saved options were lost after switching modes")
        }
        verboseCheck.state = .off
        optionChanged(verboseCheck)
        guard verboseCheck.state == .on, !verboseCheck.isEnabled, try request().verbose else {
            throw HostError(message: "GUI verbose policy could be disabled")
        }
        operationPopup.selectItem(at: 1)
        operationChanged()
        shortestPathCheck.state = .off
        optionChanged(shortestPathCheck)
        developerCheck.state = .on
        developerModeChanged()
        watchCheck.state = .on
        optionChanged(watchCheck)
        let reopened = AppController(preferences: preferences)
        guard reopened.developerMode, !reopened.copyButton.isHidden,
              reopened.verboseCheck.state == .on, !reopened.verboseCheck.isEnabled else {
            throw HostError(message: "Developer mode or options did not persist")
        }
        reopened.operationPopup.selectItem(at: 1)
        reopened.operationChanged()
        guard reopened.shortestPathCheck.state == .off, reopened.watchCheck.state == .on else {
            throw HostError(message: "Fast-img options did not persist")
        }
        for developer in [false, true] {
            developerCheck.state = developer ? .on : .off
            developerModeChanged()
            for item in operationPopup.itemArray {
                operationPopup.select(item)
                operationChanged()
                try validateOptionLayoutForSelfTest()
                guard settingsButton.isEnabled else {
                    throw HostError(message: "Settings unavailable in \(selectedOperation)")
                }
            }
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
              !verboseCheck.isEnabled, !runButton.isEnabled, !dryRunCheck.isEnabled,
              !languagePopup.isEnabled, !appearancePopup.isEnabled,
              optionControls.allSatisfy({ !$0.0.isEnabled })
        else { throw HostError(message: "Background status or running controls lost their state") }
    }

    private func validateOptionLayoutForSelfTest() throws {
        guard let content = window.contentView else { throw HostError(message: "Missing content view") }
        content.layoutSubtreeIfNeeded()
        let buttons = ([ultimateCheck] + optionControls.map { $0.0 }).filter { !$0.isHidden }
        let operation = operationPopup.convert(operationPopup.bounds, to: content)
        let log = logScroll.convert(logScroll.bounds, to: content)
        for (index, button) in buttons.enumerated() {
            let rect = button.convert(button.bounds, to: content)
            guard content.bounds.contains(rect),
                  button.bounds.width + 1 >= button.intrinsicContentSize.width,
                  !rect.intersects(operation), !rect.intersects(log),
                  buttons.dropFirst(index + 1).allSatisfy({
                      !rect.intersects($0.convert($0.bounds, to: content))
                  }) else {
                throw HostError(message: "Option clipped or overlapped in \(selectedOperation): \(button.title)")
            }
        }
        if !developerMode, !shortestPathCheck.isHidden {
            let preview = dryRunCheck.convert(dryRunCheck.bounds, to: content)
            let photos = shortestPathCheck.convert(shortestPathCheck.bounds, to: content)
            let gap = photos.minX - preview.maxX
            guard abs(preview.minY - photos.minY) < 1, (8...24).contains(gap) else {
                throw HostError(message: "Compact preview/Photos options drifted in \(selectedOperation)")
            }
        }
    }

    private func present(_ error: Error) {
        setStatus(error.localizedDescription)
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

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        controller?.requestClose() == false ? .terminateCancel : .terminateNow
    }

    @objc private func showAbout() { controller?.showAbout() }

    func configureMenus() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        let about = appMenu.addItem(withTitle: localized("menu.about"), action: #selector(showAbout), keyEquivalent: "")
        about.target = self
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
        try runProcessingHistorySelfTests()
        let sourceDocument: [String: Any] = [
            "config": ["img": ["jpeg_effort": 9]],
            "sources": ["img.jpeg_effort": "CLI"],
            "source_chain": ["img.jpeg_effort": ["default", "/synthetic/project.json", "CLI"]],
        ]
        let sourceData = try JSONSerialization.data(withJSONObject: sourceDocument)
        let sourceSettings = try EffectiveRuntimeSettings.decode(sourceData)
        let sourcePreview = try sourceSettings.originDescription(key: "img.jpeg_effort", guiOverride: true)
        guard sourcePreview.contains("img.jpeg_effort = 9"),
              sourcePreview.contains("1. default\n2. /synthetic/project.json\n3. CLI (GUI)") else {
            throw HostError(message: "Configuration preview lost its real values or override order")
        }
        for invalidChains: Any in [NSNull(), [:], ["img.jpeg_effort": []],
                                  ["img.jpeg_effort": ["default"]], ["img.jpeg_effort": ["", "CLI"]],
                                  ["img.jpeg_effort": ["CLI"], "extra": ["default"]]] {
            var invalid = sourceDocument
            invalid["source_chain"] = invalidChains
            let data = try JSONSerialization.data(withJSONObject: invalid)
            guard (try? EffectiveRuntimeSettings.decode(data)) == nil else {
                throw HostError(message: "Invalid configuration provenance was accepted")
            }
        }
        if Bundle.main.bundleURL.pathExtension == "app" {
            guard let iconURL = Bundle.main.url(forResource: "icon", withExtension: "icns"),
                  let image = NSImage(contentsOf: iconURL), image.isValid,
                  image.representations.contains(where: { $0.pixelsWide == 1024 && $0.pixelsHigh == 1024 })
            else {
                fputs("native-host self-test bundled application icon failed\n", stderr)
                return 1
            }
        }
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
        var configured = request
        configured.mediaSettings.values = [.imgFallback: "same-semantics", .imgJpegEffort: "11",
                                           .fastFallback: "strict", .fastJpegEffort: "9", .fastDatabase: "false",
                                           .photosBackend: "native", .photosNativeBatch: "200", .photosPreserveTree: "false",
                                           .imgDatabase: "false", .imgSizeTolerance: "true", .imgSizeToleranceBytes: "4096", .imgErrorMode: "log-and-continue",
                                           .vidCodec: "av1", .vidErrorMode: "fail-fast"]
        let configuredArguments = try ProcessorCommand.arguments(from: configured)
        guard configuredArguments.contains("--img-fallback-policy"),
              configuredArguments.contains("strict"), !configuredArguments.contains("same-semantics"),
              configuredArguments.contains("--photos-native-batch-size"),
              configuredArguments.contains("--preserve-folder-structure=false"),
              configuredArguments.contains("--img-allow-database=false"),
              !configuredArguments.contains("--img-error-mode"),
              !configuredArguments.contains("--img-allow-size-tolerance=true"),
               !configuredArguments.contains("--img-size-tolerance-bytes"),
              !configuredArguments.contains("--vid-codec"), configuredArguments.last == request.targetPath,
              try configured.mediaSettings.arguments(operation: .adjacent, processing: .videosOnly)
                == ["--vid-codec", "av1", "--vid-error-mode", "fail-fast"],
              try configured.mediaSettings.arguments(operation: .fastVid, processing: .videosOnly).isEmpty,
              try configured.mediaSettings.arguments(operation: .restoreJpeg, processing: .imagesOnly).isEmpty
        else { throw HostError(message: "Media settings leaked across processing modes") }
        let effective = try queryRuntimeSettings(arguments: ["--no-config"]
            + configured.mediaSettings.runtimeArguments(fast: true, inheritedOnly: false))
        guard effective["img.jpeg_effort"] == "9", effective["img.allow_database"] == "false",
              effective["photos.native_batch_size"] == "200", effective["photos.backend"] == "native",
              effective["photos.preserve_folder_structure"] == "false",
              effective.sourceChain["img.jpeg_effort"]?.first == "default",
              effective.sourceChain["img.jpeg_effort"]?.last == "CLI",
              effective.sourceChain["photos.native_batch_size"]?.last == "CLI" else {
            throw HostError(message: "GUI settings do not match the effective backend configuration")
        }
        let videoEffective = try queryRuntimeSettings(arguments: ["--no-config"]
            + configured.mediaSettings.videoRuntimeArguments(inheritedOnly: false), tool: "vid")
        guard videoEffective["vid.codec"] == "av1", videoEffective.sources["vid.codec"] == "CLI",
              videoEffective.sourceChain["vid.codec"] == ["default", "CLI"],
              videoEffective["execution.error_mode"] == "fail-fast",
              videoEffective.sourceChain["execution.error_mode"]?.last == "CLI",
              (try videoEffective.originDescription(key: "execution.error_mode", guiOverride: true)).contains("CLI (GUI)") else {
            throw HostError(message: "Video settings do not match the effective backend configuration")
        }
        let imageEffective = try queryRuntimeSettings(arguments: ["--no-config"]
            + configured.mediaSettings.runtimeArguments(fast: false, inheritedOnly: false))
        guard imageEffective["execution.error_mode"] == "log-and-continue",
              imageEffective["img.allow_size_tolerance"] == "true",
               imageEffective["img.size_tolerance_bytes"] == "4096",
               imageEffective.sourceChain["img.size_tolerance_bytes"]?.last == "CLI",
              imageEffective.sourceChain["img.allow_size_tolerance"]?.last == "CLI",
              try configured.mediaSettings.arguments(operation: .adjacent, processing: .imagesOnly)
                .contains("--img-allow-size-tolerance=true"),
              imageEffective.sourceChain["execution.error_mode"]?.last == "CLI",
              (try imageEffective.originDescription(key: "execution.error_mode", guiOverride: true)).contains("CLI (GUI)"),
              !configured.mediaSettings.runtimeArguments(fast: false, inheritedOnly: true).contains("--error-mode"),
              !configured.mediaSettings.videoRuntimeArguments(inheritedOnly: true).contains("--error-mode"),
              !configured.mediaSettings.runtimeArguments(fast: true, inheritedOnly: false).contains("--error-mode") else {
            throw HostError(message: "Failure query arguments lost overrides or invented inherited and Fast policies")
        }
        configured.mediaSettings.values[.fastToolPolicy] = "single"
        let toolEffective = try queryRuntimeSettings(arguments: ["--no-config"]
            + configured.mediaSettings.runtimeArguments(fast: true, inheritedOnly: false))
        guard toolEffective["tools.policy"] == "single", toolEffective.sources["tools.policy"] == "CLI",
              try configured.mediaSettings.arguments(operation: .adjacent, processing: .videosOnly)
                == ["--vid-codec", "av1", "--vid-error-mode", "fail-fast"] else {
            throw HostError(message: "Tool selection was not resolved or leaked into video settings")
        }
        var cacheSettings = MediaSettings()
        cacheSettings.values = [.cacheMaxBytes: "123456789", .cacheTtlSeconds: "654321"]
        let cacheArguments = ["--cache-max-bytes", "123456789", "--cache-ttl-seconds", "654321"]
        for mode in [OperationMode.adjacent, .fastImgJxl, .fastImgAvif, .fastVid] {
            guard try cacheSettings.arguments(operation: mode, processing: .videosOnly) == cacheArguments else {
                throw HostError(message: "Cache settings did not reach a processing mode")
            }
        }
        guard try cacheSettings.arguments(operation: .restoreJpeg, processing: .imagesOnly).isEmpty,
              cacheSettings.runtimeArguments(fast: false, inheritedOnly: true).isEmpty,
              cacheSettings.runtimeArguments(fast: true, inheritedOnly: true).isEmpty,
              cacheSettings.videoRuntimeArguments(inheritedOnly: true).isEmpty else {
            throw HostError(message: "Cache overrides leaked into inherited config or restoration")
        }
        for (tool, arguments) in [
            ("img", cacheSettings.runtimeArguments(fast: false, inheritedOnly: false)),
            ("img", cacheSettings.runtimeArguments(fast: true, inheritedOnly: false)),
            ("vid", cacheSettings.videoRuntimeArguments(inheritedOnly: false)),
        ] {
            let resolved = try queryRuntimeSettings(arguments: ["--no-config"] + arguments, tool: tool)
            guard resolved["cache.path_tree_max_bytes"] == "123456789",
                  resolved["cache.path_tree_ttl_seconds"] == "654321",
                  resolved.sources["cache.path_tree_max_bytes"] == "CLI",
                  resolved.sources["cache.path_tree_ttl_seconds"] == "CLI" else {
                throw HostError(message: "Cache controls disagree with the effective backend configuration")
            }
        }
        configured.mediaSettings.values[.fastJpegEffort] = "12"
        do {
            _ = try ProcessorCommand.arguments(from: configured)
            throw HostError(message: "Out-of-range JPEG effort accepted")
        } catch let error as HostError where error.message.hasPrefix("Out-of-range") { throw error }
        catch {}
        var results = BatchResults()
        let mixedResult = #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":2,"failed":1,"ignored":0,"unprocessed":0,"exit_code":1}"#
        var mixed = BatchResults()
        _ = mixed.ingest(mixedResult)
        guard mixed.hasPartialSuccess, mixed.hasFailure, !mixed.isIncomplete,
              mixed.summary == localized("result.counts", "3", "2", "1", "0")
                + " · " + localized("result.unprocessed", "0"),
              !hasPartialProcessingSuccess(succeeded: [nil], failed: [1], countsComplete: true),
              !hasPartialProcessingSuccess(succeeded: [Int.max], failed: [1], countsComplete: true),
              !hasPartialProcessingSuccess(succeeded: [1], failed: [1], countsComplete: false),
              !hasPartialProcessingSuccess(succeeded: [], failed: [], countsComplete: true) else {
            throw HostError(message: "Partial success or checked final counts were incorrectly classified")
        }
        for event in [
            #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":2,"failed":1,"ignored":0,"unprocessed":0,"exit_code":1}"#,
            #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":0,"failed":1,"ignored":0,"unprocessed":null,"exit_code":1}"#,
            #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":0,"failed":1,"ignored":0,"unprocessed":1,"exit_code":1}"#,
        ] {
            var notPartial = BatchResults()
            _ = notPartial.ingest(event)
            guard !notPartial.hasPartialSuccess, notPartial.hasFailure else {
                throw HostError(message: "Skipped, unknown or unfinished items became partial success")
            }
        }
        _ = mixed.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"vid","succeeded":1,"skipped":0,"failed":0,"ignored":1,"unprocessed":0,"exit_code":0}"#)
        guard mixed.hasPartialSuccess, mixed.summary == localized("result.counts", "4", "2", "1", "1")
            + " · " + localized("result.unprocessed", "0") else {
            throw HostError(message: "Combined media final counts lost their receipt scope")
        }
        _ = mixed.ingest("MFB_BATCH_RESULT={}")
        guard !mixed.hasPartialSuccess, mixed.isIncomplete,
              mixed.summary == localized("result.counts", localized("result.unknown"), localized("result.unknown"),
                                         localized("result.unknown"), localized("result.unknown"))
                + " · " + localized("result.unprocessed", localized("result.unknown")) else {
            throw HostError(message: "Malformed receipt left partial counts appearing complete")
        }
        let skipped = #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":2,"failed":0,"ignored":1,"exit_code":0}"#
        let legacyLine = results.ingest(skipped)
        guard legacyLine?.contains(localized("result.unprocessed", localized("result.unknown"))) == true,
              !results.hasFailure, !results.isIncomplete,
              results.totals["img"] == [3, 2, 0, 1],
              results.pendingTotals["img"] != nil, (results.pendingTotals["img"] ?? nil) == nil,
              !results.hasUnprocessed else {
            throw HostError(message: "Skipped files counted as failures")
        }
        var nullPending = BatchResults()
        let nullPendingLine = nullPending.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":null,"exit_code":0}"#)
        guard nullPendingLine?.contains(localized("result.unprocessed", localized("result.unknown"))) == true,
              !nullPending.isIncomplete, nullPending.pendingTotals["img"] != nil,
              (nullPending.pendingTotals["img"] ?? nil) == nil else {
            throw HostError(message: "Legacy null pending count became zero or incomplete")
        }
        var pending = BatchResults()
        let pendingLine = pending.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":2,"exit_code":0}"#)
        _ = pending.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":3,"exit_code":0}"#)
        guard pendingLine?.contains(localized("result.unprocessed", "2")) == true,
              (pending.pendingTotals["img"] ?? nil) == 5, pending.hasUnprocessed, !pending.isIncomplete else {
            throw HostError(message: "Pending counts were not reported and aggregated separately")
        }
        var pendingOverflow = BatchResults()
        let maxPending = String(Int.max)
        _ = pendingOverflow.ingest("MFB_BATCH_RESULT={\"schema_version\":1,\"media\":\"img\",\"succeeded\":0,\"skipped\":0,\"failed\":0,\"ignored\":0,\"unprocessed\":\(maxPending),\"exit_code\":0}")
        _ = pendingOverflow.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":1,"exit_code":0}"#)
        guard pendingOverflow.pendingTotalOverflowed, pendingOverflow.isIncomplete,
              (pendingOverflow.pendingTotals["img"] ?? nil) == nil else {
            throw HostError(message: "Overflowing pending aggregate was treated as a known total")
        }
        var inventoryOverflow = BatchResults()
        _ = inventoryOverflow.ingest("MFB_BATCH_RESULT={\"schema_version\":1,\"media\":\"img\",\"succeeded\":\(Int.max),\"skipped\":1,\"failed\":0,\"ignored\":0,\"exit_code\":0}")
        guard inventoryOverflow.invalid, inventoryOverflow.isIncomplete else {
            throw HostError(message: "Overflowing event inventory was accepted")
        }
        var aggregateOverflow = BatchResults()
        _ = aggregateOverflow.ingest("MFB_BATCH_RESULT={\"schema_version\":1,\"media\":\"img\",\"succeeded\":\(Int.max),\"skipped\":0,\"failed\":0,\"ignored\":0,\"exit_code\":0}")
        _ = aggregateOverflow.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"vid","succeeded":0,"skipped":1,"failed":0,"ignored":0,"exit_code":0}"#)
        guard aggregateOverflow.isIncomplete else {
            throw HostError(message: "Overflowing combined inventory was accepted")
        }
        for event in [
            #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":-1,"exit_code":0}"#,
            #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":"bad","exit_code":0}"#,
            #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":9223372036854775808,"exit_code":0}"#,
        ] {
            var invalidPending = BatchResults()
            _ = invalidPending.ingest(event)
            guard invalidPending.invalid, invalidPending.isIncomplete, invalidPending.pendingTotals.isEmpty else {
                throw HostError(message: "Malformed or negative pending count was accepted")
            }
        }
        var invalidAfterPending = BatchResults()
        _ = invalidAfterPending.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":2,"exit_code":0}"#)
        _ = invalidAfterPending.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":0,"skipped":0,"failed":0,"ignored":0,"unprocessed":-1,"exit_code":0}"#)
        guard invalidAfterPending.invalid, invalidAfterPending.isIncomplete,
              invalidAfterPending.pendingTotals["img"] != nil,
              (invalidAfterPending.pendingTotals["img"] ?? nil) == nil else {
            throw HostError(message: "Invalid pending count preserved a partial aggregate as known")
        }
        _ = results.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":null,"skipped":0,"failed":1,"ignored":null,"exit_code":1}"#)
        guard results.hasFailure, results.isIncomplete, results.totals["img"] == [nil, 2, 1, nil] else {
            throw HostError(message: "Batch result hid a failure or invented missing counts")
        }
        _ = results.ingest(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":-1,"exit_code":0}"#)
        guard results.invalid, results.totals["img"] == [nil, 2, 1, nil] else {
            throw HostError(message: "Malformed results replaced verified counts")
        }
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
                      != "button.run",
                  bundle.localizedString(forKey: "status.count", value: "status.count", table: nil)
                      .contains("%@")
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
        let processingPhase = #"MFB_LOG_PHASE={"schema_version":1,"phase":"processing"}"#
        let verificationPhase = #"MFB_LOG_PHASE={"schema_version":1,"phase":"verification"}"#
        var phaseLog = PhaseLogPresentation(now: 0)
        guard phaseLog.ingest(processingPhase, now: 0.5) == .changed(.processing, replace: false),
              phaseLog.ingest(verificationPhase, now: 1) == .changed(.verification, replace: false),
              phaseLog.ingest(verificationPhase, now: 4) == .unchanged,
              phaseLog.ingest("ERR: " + processingPhase, now: 5) == .changed(.processing, replace: true),
              phaseLog.ingest(verificationPhase, now: 6) == .changed(.verification, replace: false),
              phaseLog.ingest("[INFO] " + processingPhase, now: 8) == nil else {
            throw HostError(message: "Log phases flashed, repeated or interpreted incidental text")
        }
        for invalid in ["MFB_LOG_PHASE={}", processingPhase.replacingOccurrences(of: ":1", with: ":2"),
                        processingPhase.replacingOccurrences(of: "processing", with: "completed"),
                        "MFB_LOG_PHASE=" + String(repeating: "x", count: 513)] {
            guard phaseLog.ingest(invalid, now: 9) == .invalid, phaseLog.phase == .verification else {
                throw HostError(message: "Malformed phase event changed the current phase")
            }
        }
        let activityLine = #"MFB_GATE_ACTIVITY={"schema_version":1,"stage_id":"gate-test","gate":"local","check":"metadata","completed":3,"total":6,"state":"running"}"#
        var gate = GateCheckActivity()
        guard !gate.ingest("[INFO] " + activityLine, now: 1), gate.event == nil,
              gate.ingest(activityLine, now: 1), gate.event?.completed == 3,
              gate.label(now: 66).hasSuffix(localized("gate.activity.updated", "65")),
              gate.ingest(activityLine.replacingOccurrences(of: "\"completed\":3", with: "\"completed\":4")
                .replacingOccurrences(of: "\"state\":\"running\"", with: "\"state\":\"failed\""), now: 70),
              gate.event?.state == "failed", !gate.label(now: 80).contains("100%"),
              gate.ingest(activityLine, now: 81), gate.invalid, gate.event == nil else {
            throw HostError(message: "Gate activity accepted incidental output, regressed checks or claimed batch percentages")
        }
        for malformed in ["MFB_GATE_ACTIVITY={}", activityLine.replacingOccurrences(of: ":1,", with: ":2,"),
                          activityLine.replacingOccurrences(of: "metadata", with: "unknown"),
                          activityLine.replacingOccurrences(of: "\"total\":6", with: "\"total\":7"),
                          activityLine.replacingOccurrences(of: "\"completed\":3", with: "\"completed\":-1"),
                          activityLine.replacingOccurrences(of: "running", with: "passed"),
                          "MFB_GATE_ACTIVITY=" + String(repeating: "x", count: 1_025)] {
            guard gate.ingest(malformed, now: 90), gate.invalid, gate.event == nil else {
                throw HostError(message: "Invalid Gate schema, check, counts or outcome were accepted")
            }
        }
        let allPassed = activityLine.replacingOccurrences(of: "gate-test", with: "new-gate")
            .replacingOccurrences(of: "metadata", with: "decode")
            .replacingOccurrences(of: "\"completed\":3", with: "\"completed\":6")
            .replacingOccurrences(of: "running", with: "passed")
        guard gate.ingest(allPassed, now: 100), !gate.invalid, gate.event?.state == "passed", gate.event?.completed == 6 else {
            throw HostError(message: "A valid independent Gate did not replace stale invalid activity")
        }
        let inventoryRunning = #"MFB_GATE_ACTIVITY={"schema_version":1,"stage_id":"inventory-test","gate":"local","check":"inventory","completed":0,"total":6,"state":"running"}"#
        let inventoryFailed = inventoryRunning.replacingOccurrences(of: "running", with: "failed")
        guard gate.ingest(inventoryRunning, now: 101), !gate.invalid,
              gate.ingest(inventoryFailed, now: 102), !gate.invalid,
              gate.event?.check == "inventory", gate.event?.state == "failed", gate.event?.completed == 0,
              gate.label(now: 103) == localized("gate.activity.failed", localized("gate.check.inventory"), "0", "6", "6"),
              gate.ingest(inventoryRunning, now: 104), gate.invalid else {
            throw HostError(message: "Inventory failure was rejected as malformed activity or reopened its terminal Gate")
        }
        let phaseQueue = ProcessLogBackpressure(maxBytes: 0, maxEntries: 0)
        _ = phaseQueue.enqueue(processingPhase + "\n[WARN] retained warning\n" + verificationPhase)
        guard phaseQueue.takeDelivery() == processingPhase + "\n[WARN] retained warning\n" + verificationPhase,
              !phaseQueue.finishDelivery(), phaseQueue.isIdle else {
            throw HostError(message: "Backpressure reordered or discarded phase boundaries and diagnostics")
        }
        _ = phaseQueue.enqueue(verificationPhase + "\n" + activityLine + "\n[WARN] gate example")
        guard phaseQueue.takeDelivery() == verificationPhase + "\n" + activityLine + "\n[WARN] gate example",
              !phaseQueue.finishDelivery(), phaseQueue.isIdle else {
            throw HostError(message: "Backpressure dropped or reordered Gate activity after exhausting ordinary output")
        }
        var fileProgress = FileStageProgress()
        let stageStart = #"MFB_PROGRESS={"schema_version":1,"stage_id":"unit-1","stage":"image_processing","processed":0,"total":20000,"state":"running"}"#
        let almostComplete = stageStart.replacingOccurrences(of: "\"processed\":0", with: "\"processed\":19999")
        guard fileProgress.ingest(stageStart), fileProgress.event?.percentage == 0,
              fileProgress.ingest(almostComplete), fileProgress.event?.percentage == 99.99,
              fileProgress.ingest(almostComplete.replacingOccurrences(of: "running", with: "finished")),
              fileProgress.event?.processed == 19999, fileProgress.event?.state == "finished",
              fileProgress.ingest(stageStart), fileProgress.invalid, fileProgress.event == nil else {
            throw HostError(message: "Progress regressed, rounded unfinished work to 100%, or fabricated completion")
        }
        for invalid in [
            stageStart.replacingOccurrences(of: "\"schema_version\":1", with: "\"schema_version\":2"),
            stageStart.replacingOccurrences(of: "\"processed\":0", with: "\"processed\":-1"),
            stageStart.replacingOccurrences(of: "\"processed\":0", with: "\"processed\":20001"),
            stageStart.replacingOccurrences(of: "20000", with: "18446744073709551616"),
            stageStart.replacingOccurrences(of: "image_processing", with: "unknown_stage"),
            "MFB_PROGRESS={}",
        ] {
            guard fileProgress.ingest(invalid), fileProgress.invalid, fileProgress.event == nil else {
                throw HostError(message: "Invalid progress invented a percentage")
            }
        }
        let emptyStage = stageStart.replacingOccurrences(of: "20000", with: "0")
        guard fileProgress.ingest(emptyStage), fileProgress.event?.percentage == nil,
              fileProgress.ingest(stageStart.replacingOccurrences(of: "unit-1", with: "unit-2")),
              fileProgress.event?.percentage == 0 else {
            throw HostError(message: "An empty or subsequent stage inherited a fake percentage")
        }
        let coalesced = ProcessLogBackpressure(maxBytes: 0, maxEntries: 0, maxCriticalBytes: 1_024, maxCriticalEntries: 1)
        for _ in 0..<1_000 { _ = coalesced.enqueue(stageStart) }
        _ = coalesced.enqueue(almostComplete)
        _ = coalesced.enqueue(skipped)
        guard let snapshot = coalesced.takeDelivery(), snapshot.contains(almostComplete),
              !snapshot.contains(stageStart), snapshot.contains(skipped), !snapshot.contains("MFB_BATCH_RESULT={}"),
              !coalesced.finishDelivery(), coalesced.isIdle else {
            throw HostError(message: "Progress flooded the diagnostic reserve or displaced a result")
        }
        let expectedBackpressure = "first\nsecond\n\(localized("log.omitted", UInt64(1)))"
        guard backpressure.enqueue("first"), !backpressure.enqueue("second"),
              !backpressure.enqueue("omitted"),
              backpressure.takeDelivery() == expectedBackpressure,
              !backpressure.finishDelivery(), backpressure.isIdle
        else {
            fputs("native-host self-test log backpressure failed\n", stderr)
            return 1
        }
        let reserved = ProcessLogBackpressure(maxBytes: 8, maxEntries: 1)
        _ = reserved.enqueue("routine")
        _ = reserved.enqueue("omitted")
        _ = reserved.enqueue(skipped + "\nERR: Permission denied")
        guard let delivered = reserved.takeDelivery(), delivered.contains(skipped),
              delivered.contains("ERR: Permission denied"), !reserved.finishDelivery(), reserved.isIdle else {
            fputs("native-host self-test critical log reserve failed\n", stderr)
            return 1
        }
        let exhausted = ProcessLogBackpressure(maxBytes: 8, maxEntries: 1, maxCriticalBytes: 8, maxCriticalEntries: 1)
        _ = exhausted.enqueue(skipped)
        var lostResults = BatchResults()
        for line in (exhausted.takeDelivery() ?? "").split(separator: "\n") {
            _ = lostResults.ingest(String(line))
        }
        _ = lostResults.ingest(skipped)
        guard lostResults.invalid, lostResults.isIncomplete, !exhausted.finishDelivery(), exhausted.isIdle else {
            fputs("native-host self-test critical log overflow must fail closed\n", stderr)
            return 1
        }
        var diagnostics = PhotosDiagnostics()
        diagnostics.ingest("ERR: [PHOTOS PROGRESS] backend=native committed=250 verified=100 peak_backlog=150")
        guard diagnostics.committedAssets == 250, diagnostics.verifiedAssets == 100,
              diagnostics.peakBacklog == 150, diagnostics.profile == nil else {
            fputs("native-host self-test Photos progress parsing failed\n", stderr)
            return 1
        }
        diagnostics.ingest("ERR: [PHOTOS PROFILE] {\"schema_version\":1,\"backend\":\"native\",\"succeeded\":false,\"committed_assets\":250,\"verified_assets\":100,\"total_seconds\":20.0,\"verified_assets_per_second\":5.0,\"transaction_samples\":2,\"transaction_p95_seconds\":4.0,\"helper_peak_rss_bytes\":1048576,\"phases\":{\"native_transaction\":{\"calls\":2,\"seconds\":7.0}}}")
        guard diagnostics.failed, diagnostics.profile?.transactionSamples == 2,
              diagnostics.profile?.transactionP95Seconds == 4,
              diagnostics.profile?.helperPeakRssBytes == 1_048_576,
              diagnostics.profile?.importBatchSize == nil,
              diagnostics.rendered().contains(localized("diagnostics.unknown")) else {
            fputs("native-host self-test Photos profile or missing-data handling failed\n", stderr)
            return 1
        }
        diagnostics.ingest("ERR: [PHOTOS PROFILE] malformed")
        diagnostics.markFailed()
        guard diagnostics.profile?.transactionSamples == 2, diagnostics.failed else {
            fputs("native-host self-test failed Photos diagnostics were lost\n", stderr)
            return 1
        }
        diagnostics.reset()
        diagnostics.ingest("ERR: [PHOTOS PROGRESS] backend=applescript committed=0 verified=3 peak_backlog=0")
        guard diagnostics.committedAssets == nil, diagnostics.verifiedAssets == 3,
              diagnostics.peakBacklog == nil, !diagnostics.failed else {
            fputs("native-host self-test Photos diagnostics leaked across batches\n", stderr)
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
            let suite = "MFBGuiSelfTest.\(UUID().uuidString)"
            let preferences = UserDefaults(suiteName: suite)!
            defer { preferences.removePersistentDomain(forName: suite) }
            guard initialHistoryDirectory(preferences: preferences, environment: [:]) == historyDirectory else {
                throw HostError(message: "History default did not resolve the logs directory")
            }
            preferences.set("/tmp/saved-history", forKey: historyPreferenceKey)
            guard initialHistoryDirectory(preferences: preferences, environment: [:]).path == "/tmp/saved-history",
                  initialHistoryDirectory(preferences: preferences, environment: ["MFB_HOME_ROOT": "/tmp/custom-home"]).path == "/tmp/custom-home/logs",
                  initialHistoryDirectory(preferences: preferences, environment: ["MFB_LOG_DIR": "/tmp/explicit-logs", "MFB_HOME_ROOT": "/tmp/custom-home"]).path == "/tmp/explicit-logs" else {
                throw HostError(message: "History directory overrides or remembered backend path regressed")
            }
            preferences.removeObject(forKey: historyPreferenceKey)
            preferences.set("8", forKey: MediaSetting.imgJpegEffort.preferenceKey)
            let beforeSettingsRead = preferences.persistentDomain(forName: suite)! as NSDictionary
            let independent = MediaSettings(preferences: preferences)
            guard independent.values[.imgJpegEffort] == "8", independent.values[.fastJpegEffort] == nil,
                  beforeSettingsRead.isEqual(to: preferences.persistentDomain(forName: suite)!) else {
                throw HostError(message: "Reading settings wrote preferences or copied standard IMG into Fast IMG")
            }
            preferences.set("10", forKey: MediaSetting.fastJpegEffort.preferenceKey)
            guard MediaSettings(preferences: preferences).values[.fastJpegEffort] == "10",
                  MediaSettings(preferences: preferences).values[.imgJpegEffort] == "8" else {
                throw HostError(message: "Current independent image preferences were not retained")
            }
            try MediaSettings().save(to: preferences)
            let controlHost = NativeHost()
            try controlHost.validateControlForSelfTest()
            try AppController(preferences: preferences).validateInterfaceForSelfTest()
            try ProcessingHistoryPanel(directory: URL(fileURLWithPath: "/tmp/history-self-test")).validateForSelfTest()
            try MediaSettingsPanel(preferences: preferences, applied: {}).validateForSelfTest()
            try MediaSettingsPanel(preferences: preferences, developer: true, applied: {}).validateForSelfTest()
            try MediaSettingsPanel(preferences: preferences, fast: true, applied: {}).validateForSelfTest()
            try MediaSettingsPanel(preferences: preferences, developer: true, fast: true, applied: {}).validateForSelfTest()
            for developer in [false, true] {
                let videoPanel = MediaSettingsPanel(preferences: preferences, developer: developer, videos: true, applied: {})
                try videoPanel.validatePhotosForSelfTest()
                try videoPanel.validateCachePolicyForSelfTest()
            }
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
