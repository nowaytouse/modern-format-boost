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
private let mainWindowContentSize = NSSize(width: 980, height: 720)
private let mainWindowStyleMask: NSWindow.StyleMask = [
    .titled, .closable, .miniaturizable, .fullSizeContentView,
]

private var appVersion: String {
    Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development"
}

private var historyDirectory: URL {
    FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".modern_format_boost", isDirectory: true)
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
    case imgConfig, imgFallback, imgJpegEffort, imgHeuristic, imgDatabase, imgErrorMode
    case vidCodec, vidErrorMode

    var isImage: Bool { rawValue.hasPrefix("img") }
    var flag: String {
        switch self {
        case .imgConfig: "--img-config"
        case .imgFallback: "--img-fallback-policy"
        case .imgJpegEffort: "--img-jpeg-effort"
        case .imgHeuristic: "--img-quality-heuristic"
        case .imgDatabase: "--img-allow-database"
        case .imgErrorMode: "--img-error-mode"
        case .vidCodec: "--vid-codec"
        case .vidErrorMode: "--vid-error-mode"
        }
    }
    var choices: [String] {
        switch self {
        case .imgFallback: ["strict", "same-semantics", "repair"]
        case .imgHeuristic, .imgDatabase: ["true", "false"]
        case .imgErrorMode, .vidErrorMode: ["log-and-continue", "fail-fast"]
        case .vidCodec: ["hevc", "av1"]
        case .imgConfig, .imgJpegEffort: []
        }
    }
    var preferenceKey: String { "MFBGuiMediaSettings.\(rawValue)" }
    var title: String { localized("settings.\(rawValue)") }
}

private struct MediaSettings {
    var values: [MediaSetting: String] = [:]

    init(preferences: UserDefaults? = nil) {
        if let preferences {
            for field in MediaSetting.allCases {
                if let value = preferences.string(forKey: field.preferenceKey) {
                    values[field] = value
                }
            }
        }
    }

    func validate(_ fields: [MediaSetting] = MediaSetting.allCases) throws {
        for field in fields {
            guard let value = values[field] else { continue }
            let valid: Bool
            switch field {
            case .imgConfig:
                var isDirectory: ObjCBool = false
                valid = value.hasPrefix("/")
                    && FileManager.default.fileExists(atPath: value, isDirectory: &isDirectory)
                    && !isDirectory.boolValue && FileManager.default.isReadableFile(atPath: value)
            case .imgJpegEffort:
                valid = Int(value).map { (1...11).contains($0) } ?? false
            default:
                valid = field.choices.contains(value)
            }
            guard valid else { throw HostError(message: localized("settings.invalid", field.title, value)) }
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
        let images = operation.backendMode == "fast-img"
            || (operation == .adjacent && processing != .videosOnly)
        let videos = operation == .adjacent && processing != .imagesOnly
        let fields = MediaSetting.allCases.filter {
            ($0.isImage ? images : videos) && ($0 != .imgErrorMode || operation == .adjacent)
        }
        try validate(fields)
        return fields.flatMap { field -> [String] in
            guard let value = values[field] else { return [] }
            if field == .imgHeuristic || field == .imgDatabase { return ["\(field.flag)=\(value)"] }
            return [field.flag, value]
        }
    }
}

@MainActor
private final class MediaSettingsPanel: NSObject {
    private let panel: NSPanel
    private let preferences: UserDefaults
    private let applied: () -> Void
    private var popups: [MediaSetting: NSPopUpButton] = [:]
    private let configField = NSTextField()
    private let effortField = NSTextField()
    private let effortOverride = NSButton(checkboxWithTitle: "", target: nil, action: nil)
    private let effortStepper = NSStepper()
    private let tabs = NSTabView()

    init(preferences: UserDefaults, applied: @escaping () -> Void) {
        self.preferences = preferences
        self.applied = applied
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 660, height: 420),
                        styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        panel.title = localized("settings.title")
        let root = NSStackView()
        root.orientation = .vertical
        root.alignment = .width
        root.spacing = 16
        root.edgeInsets = NSEdgeInsets(top: 16, left: 20, bottom: 16, right: 20)
        panel.contentView = root
        tabs.translatesAutoresizingMaskIntoConstraints = false
        for isImage in [true, false] {
            let tab = NSTabViewItem(identifier: isImage ? "img" : "vid")
            tab.label = localized(isImage ? "media.images" : "media.videos")
            let grid = NSGridView()
            grid.rowSpacing = 12
            grid.columnSpacing = 14
            for field in MediaSetting.allCases where field.isImage == isImage {
                let control: NSView
                switch field {
                case .imgConfig:
                    configField.placeholderString = localized("settings.inherit")
                    configField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
                    let browse = NSButton(image: NSImage(systemSymbolName: "folder", accessibilityDescription: nil)!,
                                          target: self, action: #selector(browseConfig))
                    browse.toolTip = localized("settings.choose_config")
                    browse.setAccessibilityLabel(localized("settings.choose_config"))
                    control = NSStackView(views: [configField, browse])
                case .imgJpegEffort:
                    effortOverride.title = localized("settings.override")
                    effortOverride.target = self
                    effortOverride.action = #selector(effortChanged)
                    effortField.widthAnchor.constraint(equalToConstant: 48).isActive = true
                    effortStepper.minValue = 1
                    effortStepper.maxValue = 11
                    effortStepper.increment = 1
                    effortStepper.target = self
                    effortStepper.action = #selector(stepEffort)
                    control = NSStackView(views: [effortOverride, effortField, effortStepper])
                default:
                    let popup = NSPopUpButton()
                    popup.addItem(withTitle: localized("settings.inherit"))
                    for value in field.choices {
                        popup.addItem(withTitle: localized("settings.value.\(value)"))
                        popup.lastItem?.representedObject = value
                    }
                    popups[field] = popup
                    control = popup
                }
                control.setAccessibilityLabel(field.title)
                control.toolTip = localized("settings.\(field.rawValue).help")
                let row = grid.addRow(with: [NSTextField(labelWithString: field.title), control])
                row.yPlacement = .center
            }
            grid.column(at: 0).xPlacement = .trailing
            grid.column(at: 1).xPlacement = .fill
            let content = NSView()
            grid.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(grid)
            NSLayoutConstraint.activate([
                grid.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
                grid.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
                grid.topAnchor.constraint(equalTo: content.topAnchor, constant: 20),
            ])
            tab.view = content
            tabs.addTabViewItem(tab)
        }
        root.addArrangedSubview(tabs)
        let reset = NSButton(title: localized("settings.reset"), target: self, action: #selector(resetTab))
        let cancel = NSButton(title: localized("alert.cancel"), target: self, action: #selector(cancel))
        cancel.keyEquivalent = "\u{1b}"
        let apply = NSButton(title: localized("settings.apply"), target: self, action: #selector(apply))
        apply.keyEquivalent = "\r"
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let actions = NSStackView(views: [reset, spacer, cancel, apply])
        root.addArrangedSubview(actions)
        restore(MediaSettings(preferences: preferences))
    }

    func show(for window: NSWindow, videos: Bool) {
        tabs.selectTabViewItem(at: videos ? 1 : 0)
        window.beginSheet(panel)
    }

    private func restore(_ settings: MediaSettings) {
        configField.stringValue = settings.values[.imgConfig] ?? ""
        effortOverride.state = settings.values[.imgJpegEffort] == nil ? .off : .on
        effortField.stringValue = settings.values[.imgJpegEffort] ?? "11"
        effortChanged()
        for (field, popup) in popups {
            popup.selectItem(at: 0)
            if let value = settings.values[field] {
                if let item = popup.itemArray.first(where: { ($0.representedObject as? String) == value }) {
                    popup.select(item)
                } else {
                    popup.addItem(withTitle: localized("settings.invalid", field.title, value))
                    popup.lastItem?.representedObject = value
                    popup.select(popup.lastItem)
                }
            }
        }
    }

    private func draft() -> MediaSettings {
        var settings = MediaSettings()
        if !configField.stringValue.isEmpty { settings.values[.imgConfig] = configField.stringValue }
        if effortOverride.state == .on { settings.values[.imgJpegEffort] = effortField.stringValue }
        for (field, popup) in popups {
            settings.values[field] = popup.selectedItem?.representedObject as? String
        }
        return settings
    }

    @objc private func effortChanged() {
        effortField.isEnabled = effortOverride.state == .on
        effortStepper.isEnabled = effortOverride.state == .on
        effortStepper.integerValue = Int(effortField.stringValue) ?? 11
    }

    @objc private func stepEffort() { effortField.integerValue = effortStepper.integerValue }

    @objc private func browseConfig() {
        let picker = NSOpenPanel()
        picker.canChooseDirectories = false
        picker.allowsMultipleSelection = false
        picker.beginSheetModal(for: panel) { [weak self] response in
            if response == .OK, let url = picker.url { self?.configField.stringValue = url.path }
        }
    }

    @objc private func resetTab() {
        var settings = draft()
        let images = tabs.indexOfTabViewItem(tabs.selectedTabViewItem!) == 0
        for field in MediaSetting.allCases where field.isImage == images { settings.values.removeValue(forKey: field) }
        restore(settings)
    }

    @objc private func cancel() { panel.sheetParent?.endSheet(panel) }

    @objc private func apply() {
        do {
            try draft().save(to: preferences)
            applied()
            cancel()
        } catch { NSAlert(error: error).beginSheetModal(for: panel) }
    }

    func validateForSelfTest() throws {
        var settings = MediaSettings()
        settings.values = [.imgFallback: "strict", .imgJpegEffort: "9", .imgDatabase: "false",
                           .vidCodec: "av1", .vidErrorMode: "fail-fast"]
        restore(settings)
        guard draft().values == settings.values, effortField.isEnabled else {
            throw HostError(message: "Settings controls did not restore explicit overrides")
        }
        try draft().save(to: preferences)
        guard MediaSettings(preferences: preferences).values == settings.values else {
            throw HostError(message: "Media settings did not persist independently")
        }
        tabs.selectTabViewItem(at: 0)
        resetTab()
        guard draft().values == [.vidCodec: "av1", .vidErrorMode: "fail-fast"], !effortField.isEnabled else {
            throw HostError(message: "Resetting image settings changed video settings")
        }
        for index in 0...1 {
            tabs.selectTabViewItem(at: index)
            panel.contentView?.layoutSubtreeIfNeeded()
            guard let content = tabs.selectedTabViewItem?.view, let grid = content.subviews.first as? NSGridView,
                  content.bounds.contains(grid.frame) else {
                throw HostError(message: "Settings grid extends outside its tab")
            }
            for rowIndex in 0..<grid.numberOfRows {
                let row = grid.row(at: rowIndex)
                guard let label = row.cell(at: 0).contentView, let control = row.cell(at: 1).contentView,
                      label.frame.width + 1 >= label.intrinsicContentSize.width,
                      !label.frame.intersects(control.frame) else {
                    throw HostError(message: "Settings labels overlap or clip")
                }
            }
            for (field, popup) in popups where field.isImage == (index == 0) {
                guard popup.bounds.width + 1 >= popup.intrinsicContentSize.width else {
                    throw HostError(message: "Settings choice clipped: \(field.rawValue)")
                }
            }
        }
        try MediaSettings().save(to: preferences)
        guard MediaSettings(preferences: preferences).values.isEmpty else {
            throw HostError(message: "Reset settings still override inherited configuration")
        }
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

private struct BatchResults {
    struct Event: Decodable {
        let schemaVersion: Int
        let media: String
        let succeeded: Int?
        let skipped: Int?
        let failed: Int?
        let ignored: Int?
        let exitCode: Int?

        var counts: [Int?] { [succeeded, skipped, failed, ignored] }
    }

    private(set) var totals: [String: [Int?]] = [:]
    private(set) var hasFailure = false
    private(set) var hasFileFailures = false
    private(set) var invalid = false
    var isIncomplete: Bool { invalid || totals.values.contains { $0.contains(where: { $0 == nil }) } }

    mutating func ingest(_ line: String) -> String? {
        let raw = line.hasPrefix("ERR: ") ? String(line.dropFirst(5)) : line
        let prefix = "MFB_BATCH_RESULT="
        guard raw.hasPrefix(prefix) else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let event = try? decoder.decode(Event.self, from: Data(raw.dropFirst(prefix.count).utf8)),
              event.schemaVersion == 1, ["img", "vid"].contains(event.media),
              event.counts.allSatisfy({ $0.map { $0 >= 0 } ?? true }) else {
            invalid = true
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
        let count = { (value: Int?) in value.map(String.init) ?? localized("result.unknown") }
        let summary = localized("result.counts", count(event.succeeded), count(event.skipped),
                                count(event.failed), count(event.ignored))
        let tag = (event.exitCode.map { $0 != 0 } ?? false) || (event.failed ?? 0) > 0 ? "FAIL"
            : (event.exitCode == nil || event.counts.contains { $0 == nil } ? "WARN" : "SUMMARY")
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

private enum LogTone: Equatable {
    case muted, normal, stage, result, warning, failure

    static func classify(_ line: String) -> Self {
        if let count = countStatusValue(in: line) { return count.uppercased() == "MATCH" ? .result : .failure }
        if line.range(of: #"(?i)(?:\[ERROR\s*\]|\[FAIL(?:ED)?\s*\]|✗|^\s*ERR:(?!\s*\[)|\bfailed=[1-9]\d*|^\s*Integrity Issues:\s*[1-9]\d*|^\s*Integrity:(?!\s*CLEAN\b))"#, options: .regularExpression) != nil {
            return .failure
        }
        if line.range(of: #"(?i)(?:\[WARN(?:ING)?\]|⚠|\bwarning:)"#, options: .regularExpression) != nil { return .warning }
        if line.range(of: #"(?i)(?:\[(?:DONE|SUMMARY|SUCCESS|OK)\]|^\s*(?:Success rate:|Integrity:|Integrity Issues:|Total time:)|^\s*✓)"#, options: .regularExpression) != nil { return .result }
        if line.range(of: #"(?i)^\s*(?:ERR:\s*)?(?:#|\[(?:SCAN|COPY|ENCODE|VERIFY|CHECK|IMPORT|SKIP|RETAIN|RESTORE|RESUME|FINAL|STATS|ARCHIVE|PROGRESS)\s*\])"#, options: .regularExpression) != nil { return .stage }
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
    private let chooseButton = NSButton(title: "", target: nil, action: nil)
    private let backupButton = NSButton(title: "", target: nil, action: nil)
    private let backupRow = NSStackView()
    private let photosScopeButton = NSButton(title: "", target: nil, action: nil)
    private let photosScopeRow = NSStackView()
    private let openButton = NSButton(title: "", target: nil, action: nil)
    private let copyButton = NSButton(title: "", target: nil, action: nil)
    private let runButton = NSButton(title: "", target: nil, action: nil)
    private let historyButton = NSButton(title: "", target: nil, action: nil)
    private let settingsButton = NSButton(title: "", target: nil, action: nil)
    private var settingsPanel: MediaSettingsPanel?
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
    private var processorStatus = ""
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

    @objc private func refreshProcessingStatus() {
        guard let startedAt = processingStartedAt else { return }
        pauseButton.isEnabled = host.canPause
        if host.controlState == "cancelled" {
            statusLabel.stringValue = localized("status.stopping")
            return
        }
        if host.controlState == "paused" {
            statusLabel.stringValue = localized(host.isPaused ? "status.paused" : "status.pausing")
            return
        }
        let elapsed = max(0, Int(ProcessInfo.processInfo.systemUptime - startedAt))
        statusLabel.stringValue = localized("status.running_elapsed", elapsed / 60, elapsed % 60)
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
        root.material = .underWindowBackground
        root.blendingMode = .behindWindow
        root.state = .active
        root.onDrop = { [weak self] path in self?.acceptTarget(path) }
        window.contentView = root

        let icon = NSButton()
        icon.isBordered = false
        icon.target = self
        icon.action = #selector(showAbout)
        icon.toolTip = localized("menu.about")
        icon.setAccessibilityLabel(localized("menu.about"))
        icon.image = NSImage(
            systemSymbolName: "photo.stack.fill",
            accessibilityDescription: "Modern Format Boost",
        )
        icon.image = icon.image?.withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: 28, weight: .medium))
        icon.contentTintColor = .controlAccentColor
        icon.setContentHuggingPriority(.required, for: .horizontal)
        let heading = NSFont.systemFont(ofSize: 23, weight: .semibold)
        titleLabel.font = heading.fontDescriptor.withDesign(.rounded).flatMap { NSFont(descriptor: $0, size: 23) } ?? heading
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
            [NSView(), developerCheck],
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
        grid.rowSpacing = 5
        grid.columnSpacing = 12
        grid.column(at: 0).xPlacement = .trailing
        grid.column(at: 1).xPlacement = .fill
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
        let columns = [
            [ultimateCheck, freshCheck, resumeCheck, dryRunCheck],
            [shortestPathCheck, forceCheck, plainCheck, inPlaceCheck],
            [verboseCheck, archiveCheck, retryCheck, watchCheck],
        ].map { controls -> NSStackView in
            let column = NSStackView(views: controls)
            column.orientation = .vertical
            column.alignment = .leading
            column.spacing = 5
            return column
        }
        let options = NSStackView(views: columns)
        options.orientation = .horizontal
        options.distribution = .fillEqually
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
        settingsButton.target = self
        settingsButton.action = #selector(showSettings)
        settingsButton.image = NSImage(systemSymbolName: "gearshape", accessibilityDescription: nil)
        settingsButton.imagePosition = .imageOnly
        settingsButton.widthAnchor.constraint(equalToConstant: 32).isActive = true
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
        let actionRow = NSStackView(views: [settingsButton, historyButton, diagnosticsButton, openButton, copyButton, spacer, pauseButton, stopButton, runButton])
        actionRow.orientation = .horizontal
        actionRow.alignment = .centerY
        actionRow.spacing = 8

        logView.isEditable = false
        logView.isSelectable = true
        logView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        logView.textContainerInset = NSSize(width: 8, height: 8)
        logView.backgroundColor = .textBackgroundColor.withAlphaComponent(0.72)
        logScroll.documentView = logView
        logScroll.hasVerticalScroller = true
        logScroll.borderType = .bezelBorder
        logScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 260).isActive = true

        countStatusLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        countStatusLabel.isHidden = true
        countStatusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

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
            countStatusLabel, logScroll, statusRow,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.translatesAutoresizingMaskIntoConstraints = false
        for view in [
            header, targetRow, grid, backupRow, photosScopeRow, metadataSafetyLabel, options, commandField, actionRow,
            countStatusLabel, logScroll, statusRow,
        ] {
            view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -28),
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

    private func clearBatchLog() {
        logView.string = ""
        batchResults = BatchResults()
        photosDiagnostics.reset()
        refreshDiagnostics()
        countStatus = nil
        countStatusLabel.isHidden = true
        appendLog(localized("log.history", resolvedHistoryDirectory.path))
    }

    @objc private func openHistory() {
        do {
            if !NSWorkspace.shared.open(resolvedHistoryDirectory) {
                throw HostError(message: localized("error.open_history", resolvedHistoryDirectory.path))
            }
        } catch { present(error) }
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
            let scroll = NSScrollView(frame: panel.contentView!.bounds)
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
        let hasMediaSettings = [.adjacent, .fastImgJxl, .fastImgAvif].contains(selectedOperation)
        settingsButton.isEnabled = configurationControlsEnabled && hasMediaSettings
        settingsButton.toolTip = localized(hasMediaSettings ? "settings.title" : "settings.unavailable")
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
            mediaSettings: MediaSettings(preferences: preferences),
        )
    }

    private func appendLog(_ text: String) {
        let displayText = text.split(separator: "\n", omittingEmptySubsequences: false).map { line -> String in
            if let summary = batchResults.ingest(String(line)) {
                return developerMode ? "\(line)\n\(summary)" : summary
            }
            return String(line)
        }.joined(separator: "\n")
        var diagnosticsChanged = false
        for line in text.split(separator: "\n") {
            if photosDiagnostics.ingest(String(line)) { diagnosticsChanged = true }
        }
        if diagnosticsChanged { refreshDiagnostics() }
        for line in text.split(separator: "\n") where line.hasPrefix("MFB_LOG_DIRECTORY=") {
            let encoded = Data(line.dropFirst("MFB_LOG_DIRECTORY=".count).utf8)
            if let path = try? JSONDecoder().decode(String.self, from: encoded), path.hasPrefix("/") {
                resolvedHistoryDirectory = URL(fileURLWithPath: path, isDirectory: true)
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
        let storage = logView.textStorage!
        if !storage.string.isEmpty { storage.append(NSAttributedString(string: "\n")) }
        storage.append(styledLog(displayText))
        let lines = storage.string.split(separator: "\n", omittingEmptySubsequences: false)
        if lines.count > 3_000 {
            let retained = lines.suffix(3_000).joined(separator: "\n")
            storage.setAttributedString(styledLog(retained))
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
        if host.controlState == "cancelled", host.lastExitStatus == 130 {
            photosDiagnostics.markFailed()
            refreshDiagnostics()
            setProcessing(false)
            if var retry = lastRequest, retry.operationMode.capabilities.supportsResume {
                applyResumeDecision(fresh: false, to: &retry)
                lastRequest = retry
            }
            statusLabel.stringValue = localized("status.stopped")
            appendLog(localized("status.stopped"))
            if closeWhenFinished { NSApp.terminate(nil) }
            return
        }
        switch result {
        case let .success(message):
            setProcessing(false)
            if batchResults.hasFailure {
                statusLabel.stringValue = localized(batchResults.hasFileFailures
                    ? "result.finished_with_failures" : "result.stopped_with_error")
                appendLog("[FAIL] \(statusLabel.stringValue)")
            } else if batchResults.isIncomplete || (batchResults.totals.isEmpty
                && lastRequest.map { !$0.dryRun && [.adjacent, .fastImgJxl, .fastImgAvif, .fastVid].contains($0.operationMode) } == true) {
                statusLabel.stringValue = localized("result.incomplete")
                appendLog("[WARN] \(statusLabel.stringValue)")
            } else {
                statusLabel.stringValue = message
                appendLog("✓ \(message)")
            }
        case let .failure(error):
            photosDiagnostics.markFailed()
            refreshDiagnostics()
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
                batchResults = BatchResults()
                photosDiagnostics.reset()
                refreshDiagnostics()
                setProcessing(true)
                host.startProcessing(retry)
            } else {
                setProcessing(false)
                statusLabel.stringValue = batchResults.hasFileFailures
                    ? localized("result.finished_with_failures") + " · " + error.localizedDescription
                    : error.localizedDescription
            }
        }
    }

    @objc private func showSettings() {
        guard configurationControlsEnabled,
              [.adjacent, .fastImgJxl, .fastImgAvif].contains(selectedOperation) else { return }
        settingsPanel = MediaSettingsPanel(preferences: preferences) { [weak self] in
            self?.configurationChanged()
        }
        settingsPanel?.show(for: window, videos: selectedOperation == .fastVid
            || (selectedOperation == .adjacent && processingPopup.indexOfSelectedItem == 2))
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
        diagnosticsButton.toolTip = localized("button.photos_diagnostics")
        diagnosticsButton.setAccessibilityLabel(localized("button.photos_diagnostics"))
        diagnosticsPanel?.title = localized("button.photos_diagnostics")
        historyButton.toolTip = resolvedHistoryDirectory.path
        pauseButton.title = localized(host.controlState == "paused" ? "button.resume" : "button.pause")
        stopButton.title = localized("button.stop")
        updateOptionTitles()
        commandField.placeholderString = localized("command.placeholder")
        replaceTitles(processingPopup, with: [
            localized("media.both"), localized("media.images"), localized("media.videos"),
        ])
        refreshOperationPopup()
        replaceTitles(languagePopup, with: AppLanguage.allCases.map(\.nativeTitle))
        replaceTitles(appearancePopup, with: AppAppearance.allCases.map(\.localizedTitle))
        refreshCountStatus()
        refreshDiagnostics()
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
        }
    }

    func validateInterfaceForSelfTest() throws {
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
              window.title.contains(appVersion) else {
            throw HostError(message: "Log area or visible version regressed")
        }
        appendLog("previous batch sentinel")
        clearBatchLog()
        guard !logView.string.contains("previous batch sentinel"), logView.string.contains(historyDirectory.path) else {
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
              LogTone.classify("ERR: Permission denied") == .failure
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
              logView.string.contains("    Count status:    MATCH")
        else { throw HostError(message: "Non-MATCH status hid preceding diagnostics") }
        clearBatchLog()
        guard countStatusLabel.isHidden, countStatus == nil else {
            throw HostError(message: "Previous batch count status leaked into the next batch")
        }
        appendLog(#"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":1,"failed":1,"ignored":0,"exit_code":0}"#)
        processingCompleted(.success(localized("status.completed")))
        guard statusLabel.stringValue == localized("result.finished_with_failures"),
              logView.string.contains("[FAIL] IMG:"), batchResults.hasFailure else {
            throw HostError(message: "GUI treated reported file failures as success")
        }
        clearBatchLog()
        guard batchResults.totals.isEmpty, !batchResults.hasFailure else {
            throw HostError(message: "Batch counters leaked into a new run")
        }
        let attribution = try bundledLicenseText()
        guard attribution.contains("Modern Format Boost"), attribution.contains("Apache") else {
            throw HostError(message: "Bundled attribution is incomplete")
        }
        for controls in [[ultimateCheck, freshCheck, resumeCheck, dryRunCheck],
                         [shortestPathCheck, forceCheck, plainCheck, inPlaceCheck],
                         [verboseCheck, archiveCheck, retryCheck]] {
            let x = controls[0].convert(controls[0].bounds, to: content).minX
            guard controls.allSatisfy({ abs($0.convert($0.bounds, to: content).minX - x) < 1 }) else {
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
              defaultArguments.contains("--no-resume"), !defaultArguments.contains("--watch")
        else { throw HostError(message: "Default checkbox flags disagree with the command") }
        let originalFrame = window.frame
        developerCheck.state = .on
        developerModeChanged()
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
                                           .imgDatabase: "false", .imgErrorMode: "log-and-continue",
                                           .vidCodec: "av1", .vidErrorMode: "fail-fast"]
        let configuredArguments = try ProcessorCommand.arguments(from: configured)
        guard configuredArguments.contains("--img-fallback-policy"),
              configuredArguments.contains("--img-allow-database=false"),
              !configuredArguments.contains("--img-error-mode"),
              !configuredArguments.contains("--vid-codec"), configuredArguments.last == request.targetPath,
              try configured.mediaSettings.arguments(operation: .adjacent, processing: .videosOnly)
                == ["--vid-codec", "av1", "--vid-error-mode", "fail-fast"],
              try configured.mediaSettings.arguments(operation: .fastVid, processing: .videosOnly).isEmpty,
              try configured.mediaSettings.arguments(operation: .restoreJpeg, processing: .imagesOnly).isEmpty
        else { throw HostError(message: "Media settings leaked across processing modes") }
        configured.mediaSettings.values[.imgJpegEffort] = "12"
        do {
            _ = try ProcessorCommand.arguments(from: configured)
            throw HostError(message: "Out-of-range JPEG effort accepted")
        } catch let error as HostError where error.message.hasPrefix("Out-of-range") { throw error }
        catch {}
        var results = BatchResults()
        let skipped = #"MFB_BATCH_RESULT={"schema_version":1,"media":"img","succeeded":3,"skipped":2,"failed":0,"ignored":1,"exit_code":0}"#
        guard results.ingest(skipped) != nil, !results.hasFailure, !results.isIncomplete,
              results.totals["img"] == [3, 2, 0, 1] else {
            throw HostError(message: "Skipped files counted as failures")
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
        let expectedBackpressure = "first\nsecond\n\(localized("log.omitted", UInt64(1)))"
        guard backpressure.enqueue("first"), !backpressure.enqueue("second"),
              !backpressure.enqueue("omitted"),
              backpressure.takeDelivery() == expectedBackpressure,
              !backpressure.finishDelivery(), backpressure.isIdle
        else {
            fputs("native-host self-test log backpressure failed\n", stderr)
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
            let controlHost = NativeHost()
            try controlHost.validateControlForSelfTest()
            try AppController(preferences: preferences).validateInterfaceForSelfTest()
            try MediaSettingsPanel(preferences: preferences, applied: {}).validateForSelfTest()
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
