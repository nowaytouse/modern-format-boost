import AppKit

private struct PreflightEntry: Decodable {
    let relative_path: String
    let route: String
    let selected: Bool
    let bytes: UInt64
    let action: String
    let reason: String
}

private struct PreflightRelationship: Decodable {
    let kind: String
    let source_relative_path: String
    let match_basis: String
    let status: String
    let candidate_relative_paths: [String]
    let candidate_count: Int
    let candidates_truncated: Bool
    let verified: Bool
}

private struct PreflightPlan: Decodable {
    let input: String
    let output_requested: String?
    let in_place_requested: Bool
    let files_discovered: Int
    let selected_images: Int
    let selected_videos: Int
    let filtered_media: Int
    let sidecars: Int
    let other_files: Int
    let selected_media_bytes: UInt64
    let entries: [PreflightEntry]
    let relationships: [PreflightRelationship]
}

private struct PreflightWorker: Decodable {
    let name: String
    let path: String
    let status: String
    let bytes: UInt64?
    let executable_check: String
}

private struct PreflightSnapshot: Decodable {
    let schema_version: Int
    let scope: String
    let read_only: Bool
    let execution_started: Bool
    let plans: [PreflightPlan]
    let workers: [PreflightWorker]

    private static func require(_ value: Bool, _ field: String) throws {
        guard value else { throw NSError(domain: "MFBPreflight", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "Invalid preflight report: \(field)"]) }
    }

    private static func keys(_ object: [String: Any], _ expected: [String]) throws {
        try require(Set(object.keys) == Set(expected), "missing or unknown fields")
    }

    private static func pathKey(_ path: String) -> Data { Data(path.utf8) }
    private static func stemKey(_ path: String) -> Data {
        pathKey((path as NSString).deletingPathExtension)
    }

    private static func validRelativePath(_ path: String) -> Bool {
        !path.isEmpty && !path.hasPrefix("/") && !path.contains("\0")
            && path.split(separator: "/", omittingEmptySubsequences: false)
                .allSatisfy { !$0.isEmpty && $0 != "." && $0 != ".." }
    }

    static func decode(_ data: Data, expectedInput: String) throws -> PreflightSnapshot {
        try require(!data.isEmpty && data.count <= 1_048_576, "report exceeds 1 MiB")
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let plans = object["plans"] as? [[String: Any]],
              let workers = object["workers"] as? [[String: Any]] else {
            try require(false, "report shape")
            throw NSError(domain: "MFBPreflight", code: 1)
        }
        try keys(object, ["schema_version", "scope", "read_only", "execution_started", "plans", "workers"])
        for plan in plans {
            try keys(plan, ["input", "output_requested", "in_place_requested", "files_discovered",
                "selected_images", "selected_videos", "filtered_media", "sidecars", "other_files",
                "selected_media_bytes", "entries", "relationships"])
            guard let entries = plan["entries"] as? [[String: Any]],
                  let relationships = plan["relationships"] as? [[String: Any]] else {
                try require(false, "inventory shape")
                continue
            }
            for entry in entries {
                try keys(entry, ["relative_path", "route", "selected", "bytes", "action", "reason"])
            }
            for relationship in relationships {
                try keys(relationship, ["kind", "source_relative_path", "match_basis", "status",
                    "candidate_relative_paths", "candidate_count", "candidates_truncated", "verified"])
            }
        }
        for worker in workers {
            try keys(worker, ["name", "path", "status", "bytes", "executable_check"])
        }
        let snapshot = try JSONDecoder().decode(Self.self, from: data)
        try require(snapshot.schema_version == 2 && snapshot.scope == "launcher_routing_snapshot"
            && snapshot.read_only && !snapshot.execution_started, "read-only schema")
        try require(snapshot.plans.count == 1, "one selected input is required")
        let plan = snapshot.plans[0]
        try require(pathKey(plan.input) == pathKey(expectedInput) && plan.input.hasPrefix("/")
            && !plan.input.contains("\0"), "input identity")
        if let output = plan.output_requested {
            try require(output.hasPrefix("/") && !output.contains("\0"), "output path")
        }
        try require(plan.entries.count <= 2_000 && plan.files_discovered == plan.entries.count,
            "inventory exceeds 2,000 entries or is incomplete")
        var byPath: [Data: PreflightEntry] = [:]
        var byStem: [Data: [PreflightEntry]] = [:]
        var images = 0, videos = 0, filtered = 0, sidecars = 0, other = 0
        var selectedBytes: UInt64 = 0
        for entry in plan.entries {
            let key = pathKey(entry.relative_path)
            try require(validRelativePath(entry.relative_path) && byPath[key] == nil, "relative path identity")
            byPath[key] = entry
            let action: String
            let reason: String
            switch entry.route {
            case "image", "video":
                byStem[stemKey(entry.relative_path), default: []].append(entry)
                if entry.selected {
                    if entry.route == "image" { images += 1 } else { videos += 1 }
                    let addition = selectedBytes.addingReportingOverflow(entry.bytes)
                    try require(!addition.overflow, "selected byte total overflow")
                    selectedBytes = addition.partialValue
                    action = entry.route == "image" ? "submit_img" : "submit_vid"
                    reason = entry.route == "image" ? "content_routed_to_image_worker" : "content_routed_to_video_worker"
                } else {
                    filtered += 1
                    action = "retain"
                    reason = "excluded_by_selected_media_filter"
                }
            case "sidecar":
                try require(!entry.selected, "sidecar selected as media")
                sidecars += 1
                action = "preserve_sidecar"
                reason = "metadata_sidecar_not_a_separate_media_task"
            case "copy_only":
                try require(!entry.selected, "other file selected as media")
                other += 1
                action = plan.in_place_requested ? "retain" : "copy_after_success"
                reason = plan.in_place_requested ? "non_media_source_retained_in_place"
                    : "non_media_copy_is_conditional_on_successful_adjacent_processing"
            default:
                try require(false, "unknown media route")
                continue
            }
            try require(entry.action == action && entry.reason == reason, "routing action")
        }
        try require(images == plan.selected_images && videos == plan.selected_videos
            && filtered == plan.filtered_media && sidecars == plan.sidecars && other == plan.other_files
            && selectedBytes == plan.selected_media_bytes, "counts or bytes do not reconcile")
        var observedSources = Set<Data>()
        for relationship in plan.relationships {
            let key = pathKey(relationship.source_relative_path)
            guard let source = byPath[key] else {
                try require(false, "relationship source is absent")
                continue
            }
            try require(observedSources.insert(key).inserted && !relationship.verified,
                "duplicate or verified filename relationship")
            let candidates: [PreflightEntry]
            let ambiguous: Bool
            let basis: String
            switch relationship.kind {
            case "xmp_filename":
                try require(source.route == "sidecar", "sidecar relationship route")
                let stem = stemKey(source.relative_path)
                if let exact = byPath[stem], exact.route == "image" || exact.route == "video" {
                    candidates = [exact]
                    basis = "exact_filename"
                } else {
                    candidates = byStem[stem] ?? []
                    basis = "same_directory_stem"
                }
                ambiguous = candidates.count > 1
            case "live_photo_filename":
                try require(source.route == "image", "Live Photo filename source is not an image")
                let group = byStem[stemKey(source.relative_path)] ?? []
                candidates = group.filter { $0.route == "video" }
                try require(!candidates.isEmpty, "Live Photo filename candidate is absent")
                ambiguous = candidates.count > 1 || group.filter { $0.route == "image" }.count > 1
                basis = "same_directory_stem"
            default:
                try require(false, "unknown filename relationship")
                continue
            }
            let expectedPaths = candidates.prefix(64).map { pathKey($0.relative_path) }
            let status = candidates.isEmpty ? "unmatched" : (ambiguous ? "ambiguous" : "candidate")
            try require(relationship.match_basis == basis && relationship.status == status
                && relationship.candidate_count == candidates.count
                && relationship.candidates_truncated == (candidates.count > 64)
                && relationship.candidate_relative_paths.map(pathKey) == expectedPaths, "filename candidates do not reconcile")
        }
        let expectedSources = Set(plan.entries.filter { entry in
            entry.route == "sidecar" || (entry.route == "image"
                && (byStem[stemKey(entry.relative_path)] ?? []).contains { $0.route == "video" })
        }.map { pathKey($0.relative_path) })
        try require(observedSources == expectedSources, "filename observations are incomplete")
        let expectedWorkers = Set((images > 0 ? ["img"] : []) + (videos > 0 ? ["vid"] : []))
        try require(Set(snapshot.workers.map(\.name)) == expectedWorkers
            && snapshot.workers.count == expectedWorkers.count, "worker observations are incomplete")
        for worker in snapshot.workers {
            try require(worker.path.hasPrefix("/") && !worker.path.contains("\0")
                && ["available", "missing", "unreadable", "not_regular_file", "not_executable", "executable_unknown"].contains(worker.status)
                && ["unix_mode_bits", "unsupported_platform"].contains(worker.executable_check), "worker observation")
            if worker.status == "available" {
                try require(worker.bytes != nil && worker.executable_check == "unix_mode_bits", "available worker evidence")
            }
            if worker.status == "executable_unknown" {
                try require(worker.executable_check == "unsupported_platform", "unknown executable check")
            }
            if worker.status == "not_executable" {
                try require(worker.executable_check == "unix_mode_bits", "Unix executable check")
            }
        }
        return snapshot
    }
}

@MainActor
final class PreflightInspectorPanel: NSWindowController, NSTableViewDataSource, NSTableViewDelegate,
    NSSearchFieldDelegate, NSWindowDelegate {
    var onClose: (() -> Void)?
    private let language: String
    private let input = NSTextField(labelWithString: "")
    private let summary = NSTextField(wrappingLabelWithString: "")
    private let workers = NSTextField(wrappingLabelWithString: "")
    private let search = NSSearchField()
    private let table = NSTableView()
    private let detail = NSTextView()
    private let status = NSTextField(labelWithString: "")
    private let progress = NSProgressIndicator()
    private var plan: PreflightPlan?
    private var rows: [PreflightEntry] = []

    private func text(_ english: String, _ chinese: String, _ japanese: String) -> String {
        if language == "zh-Hans" || language == "simplifiedChinese" { return chinese }
        if language == "ja" || language == "japanese" { return japanese }
        return english
    }

    init(language: String) {
        self.language = language
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 980, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        super.init(window: window)
        window.minSize = NSSize(width: 760, height: 650)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.title = text("Routing Inspection", "处理预检", "処理前の確認")
        buildInterface()
    }

    required init?(coder: NSCoder) { nil }

    private func buildInterface() {
        let content = NSView()
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        window?.contentView = content
        input.lineBreakMode = .byTruncatingMiddle
        input.font = .systemFont(ofSize: 13, weight: .semibold)
        summary.font = .systemFont(ofSize: 12)
        summary.maximumNumberOfLines = 4
        summary.lineBreakMode = .byTruncatingMiddle
        workers.font = .systemFont(ofSize: 12)
        workers.maximumNumberOfLines = 3
        workers.lineBreakMode = .byTruncatingMiddle
        search.placeholderString = text("Path, route, action or reason", "搜索路径、类型、动作或原因", "パス・種類・処理・理由を検索")
        search.setAccessibilityLabel(search.placeholderString)
        search.delegate = self
        table.delegate = self
        table.dataSource = self
        table.rowHeight = 24
        table.usesAlternatingRowBackgroundColors = true
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        for (identifier, title, width) in [
            ("path", text("File", "文件", "ファイル"), 290.0),
            ("route", text("Type", "类型", "種類"), 85.0),
            ("action", text("Action", "处理动作", "処理"), 155.0),
            ("reason", text("Reason", "原因", "理由"), 350.0)
        ] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(identifier))
            column.title = title
            column.width = width
            column.minWidth = identifier == "path" ? 150 : 65
            table.addTableColumn(column)
        }
        table.setAccessibilityLabel(text("Read-only routing inventory", "只读处理清单", "読み取り専用の処理一覧"))
        let tableScroll = NSScrollView()
        tableScroll.documentView = table
        tableScroll.hasVerticalScroller = true
        tableScroll.hasHorizontalScroller = true
        tableScroll.borderType = .bezelBorder
        detail.isEditable = false
        detail.isSelectable = true
        detail.font = .systemFont(ofSize: 12)
        detail.textColor = .labelColor
        detail.backgroundColor = .textBackgroundColor
        detail.textContainerInset = NSSize(width: 8, height: 6)
        detail.isVerticallyResizable = true
        detail.isHorizontallyResizable = false
        detail.textContainer?.widthTracksTextView = true
        detail.autoresizingMask = [.width]
        detail.setAccessibilityLabel(text("File and filename candidate details", "文件与文件名候选详情", "ファイルと名前候補の詳細"))
        let detailScroll = NSScrollView()
        detailScroll.documentView = detail
        detailScroll.hasVerticalScroller = true
        detailScroll.borderType = .bezelBorder
        progress.style = .spinning
        progress.controlSize = .small
        progress.isDisplayedWhenStopped = false
        let footer = NSStackView(views: [progress, status])
        footer.spacing = 8
        status.font = .systemFont(ofSize: 11)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingTail
        for view in [input, summary, workers, search, tableScroll, detailScroll, footer] {
            stack.addArrangedSubview(view)
            view.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -12),
            tableScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 180),
            detailScroll.heightAnchor.constraint(equalToConstant: 110),
            progress.widthAnchor.constraint(equalToConstant: 16),
            progress.heightAnchor.constraint(equalToConstant: 16)
        ])
        tableScroll.setContentHuggingPriority(.defaultLow, for: .vertical)
    }

    func showLoading(input: String) {
        plan = nil
        rows = []
        table.deselectAll(nil)
        table.reloadData()
        self.input.stringValue = input
        self.input.toolTip = input
        self.input.setAccessibilityLabel(input)
        summary.stringValue = text("Reading selected input…", "正在检查选定路径…", "選択したパスを確認中…")
        workers.stringValue = ""
        detail.string = ""
        search.stringValue = ""
        search.isEnabled = false
        status.stringValue = text("Read-only inspection · Processing has not started", "只读预检 · 尚未开始处理", "読み取り専用の確認 · 処理は未開始")
        progress.startAnimation(nil)
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    func showReport(data: Data, expectedInput: String) throws {
        let snapshot = try PreflightSnapshot.decode(data, expectedInput: expectedInput)
        let plan = snapshot.plans[0]
        self.plan = plan
        progress.stopAnimation(nil)
        input.stringValue = plan.input
        input.toolTip = plan.input
        input.setAccessibilityLabel(plan.input)
        let mode = plan.in_place_requested ? text("In place", "原位处理", "元の場所で処理") : text("Adjacent output", "相邻输出", "隣接フォルダに出力")
        let output = plan.output_requested ?? text("Not explicitly selected", "未显式指定", "指定なし")
        summary.stringValue = text("Files", "文件", "ファイル") + ": \(plan.files_discovered)  · IMG: \(plan.selected_images)  · VID: \(plan.selected_videos)  · "
            + text("Filtered", "未选中媒体", "対象外メディア") + ": \(plan.filtered_media)  · XMP: \(plan.sidecars)  · "
            + text("Other", "其他文件", "その他") + ": \(plan.other_files)  · "
            + text("Selected bytes", "选中媒体字节", "対象メディアのバイト") + ": \(plan.selected_media_bytes)\n\(mode)  · "
            + text("Output", "输出", "出力") + ": \(output)"
        summary.toolTip = summary.stringValue
        summary.setAccessibilityLabel(summary.stringValue)
        workers.stringValue = snapshot.workers.map { worker in
            "\(worker.name.uppercased()): \(workerText(worker.status))  · \(worker.path)"
        }.joined(separator: "\n")
        workers.toolTip = workers.stringValue
        workers.setAccessibilityLabel(workers.stringValue)
        search.isEnabled = true
        applyFilter()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    func showError(_ message: String) {
        progress.stopAnimation(nil)
        plan = nil
        rows = []
        table.deselectAll(nil)
        table.reloadData()
        search.isEnabled = false
        summary.stringValue = text("Inspection unavailable", "预检不可用", "確認できません")
        workers.stringValue = ""
        status.stringValue = text("No validated report · Processing has not started", "未获得有效报告 · 尚未开始处理", "有効なレポートなし · 処理は未開始")
        detail.string = message
    }

    private func workerText(_ value: String) -> String {
        switch value {
        case "available": return text("Readable file, execute bits set", "文件可读，执行位已设置", "読み取り可能・実行ビットあり")
        case "missing": return text("File missing", "文件不存在", "ファイルなし")
        case "unreadable": return text("File unreadable", "无法读取文件", "読み取り不可")
        case "not_regular_file": return text("Not a regular file", "不是普通文件", "通常ファイルではありません")
        case "not_executable": return text("Executable permission absent", "没有执行权限", "実行権限なし")
        default: return text("Executable permission unknown", "执行权限未知", "実行権限は不明")
        }
    }

    private func routeText(_ value: String) -> String {
        switch value {
        case "image": return text("Image", "图像", "画像")
        case "video": return text("Video", "视频", "動画")
        case "sidecar": return "XMP"
        default: return text("Other", "其他", "その他")
        }
    }

    private func actionText(_ value: String) -> String {
        switch value {
        case "submit_img": return text("Submit to IMG", "交给 IMG", "IMG に渡す")
        case "submit_vid": return text("Submit to VID", "交给 VID", "VID に渡す")
        case "preserve_sidecar": return text("Preserve sidecar", "保留附属元数据", "付属メタデータを保持")
        case "copy_after_success": return text("Copy after success", "成功后复制", "成功後にコピー")
        default: return text("Retain source", "保留源文件", "元ファイルを保持")
        }
    }

    private func reasonText(_ value: String) -> String {
        switch value {
        case "content_routed_to_image_worker": return text("Image content identified", "内容识别为图像", "内容を画像と識別")
        case "content_routed_to_video_worker": return text("Video content identified", "内容识别为视频", "内容を動画と識別")
        case "excluded_by_selected_media_filter": return text("Outside selected media filter", "不在选定媒体范围内", "選択したメディア範囲の対象外")
        case "metadata_sidecar_not_a_separate_media_task": return text("Metadata, not a separate media task", "附属元数据，不单独转码", "付属メタデータ・単独の変換対象ではありません")
        case "non_media_source_retained_in_place": return text("Other file retained in place", "其他文件原位保留", "その他のファイルを元の場所に保持")
        default: return text("Copy only after successful adjacent processing", "相邻处理成功后才复制", "隣接処理の成功後にのみコピー")
        }
    }

    func controlTextDidChange(_ notification: Notification) { applyFilter() }

    private func applyFilter() {
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        rows = (plan?.entries ?? []).filter { entry in
            query.isEmpty || [entry.relative_path, entry.route, routeText(entry.route), entry.action,
                actionText(entry.action), entry.reason, reasonText(entry.reason)]
                .contains { $0.localizedCaseInsensitiveContains(query) }
        }
        table.deselectAll(nil)
        table.reloadData()
        detail.string = ""
        status.stringValue = text("Showing", "显示", "表示") + " \(rows.count) / \(plan?.entries.count ?? 0)  · "
            + text("Routing snapshot only · No encoding or delivery proof", "仅为路由快照 · 不是编码或交付证明", "経路のスナップショットのみ · エンコード・引き渡しの証明ではありません")
    }

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    private static func displayPath(_ path: String) -> String {
        path.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\n", with: "\\n")
            .replacingOccurrences(of: "\r", with: "\\r")
            .replacingOccurrences(of: "\t", with: "\\t")
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard rows.indices.contains(row), let column = tableColumn else { return nil }
        let entry = rows[row]
        let value: String
        switch column.identifier.rawValue {
        case "path": value = Self.displayPath(entry.relative_path)
        case "route": value = routeText(entry.route)
        case "action": value = actionText(entry.action)
        default: value = reasonText(entry.reason)
        }
        let cell = tableView.makeView(withIdentifier: column.identifier, owner: nil) as? NSTableCellView
            ?? NSTableCellView()
        cell.identifier = column.identifier
        let label: NSTextField
        if let existing = cell.textField {
            label = existing
        } else {
            label = NSTextField(labelWithString: "")
            label.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(label)
            cell.textField = label
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
                label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
                label.centerYAnchor.constraint(equalTo: cell.centerYAnchor)
            ])
        }
        label.stringValue = value
        label.font = .systemFont(ofSize: 12)
        label.usesSingleLineMode = true
        label.lineBreakMode = .byTruncatingTail
        label.toolTip = value
        label.setAccessibilityLabel(value)
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard rows.indices.contains(table.selectedRow), let plan else { detail.string = ""; return }
        let entry = rows[table.selectedRow]
        let key = Data(entry.relative_path.utf8)
        let relationships = plan.relationships.filter { Data($0.source_relative_path.utf8) == key }
        var lines = [Self.displayPath(entry.relative_path), "\(routeText(entry.route))  · \(actionText(entry.action))  · \(entry.bytes) "
            + text("bytes", "字节", "バイト"), reasonText(entry.reason)]
        for relationship in relationships {
            let kind = relationship.kind == "xmp_filename" ? "XMP" : "Live Photo"
            let match = relationship.status == "unmatched" ? text("Unmatched", "没有候选", "候補なし")
                : relationship.status == "ambiguous" ? text("Ambiguous", "候选不唯一", "候補が複数")
                : text("Candidate", "候选", "候補")
            lines.append("\(kind): \(match)  · \(relationship.candidate_count)  · "
                + text("Filename only, unverified", "仅文件名线索，未经核验", "ファイル名の手掛かりのみ・未検証"))
            lines += relationship.candidate_relative_paths.map(Self.displayPath)
            if relationship.candidates_truncated {
                lines.append(text("Candidate list limited to 64; total shown above", "候选列表限显 64 条；完整数量见上方", "候補一覧は64件まで・総数は上記"))
            }
        }
        detail.string = lines.joined(separator: "\n")
    }

    func windowWillClose(_ notification: Notification) {
        let callback = onClose
        onClose = nil
        callback?()
    }

    private static func fixture() -> [String: Any] {
        let media = "\u{56fe}\u{50cf}\n\""
        func entry(_ path: String, _ route: String, _ action: String, _ reason: String) -> [String: Any] {
            ["relative_path": path, "route": route, "selected": route != "sidecar", "bytes": 4,
                "action": action, "reason": reason]
        }
        return ["schema_version": 2, "scope": "launcher_routing_snapshot", "read_only": true,
            "execution_started": false, "plans": [["input": "/synthetic/preflight", "output_requested": NSNull(),
                "in_place_requested": false, "files_discovered": 3, "selected_images": 1, "selected_videos": 1,
                "filtered_media": 0, "sidecars": 1, "other_files": 0, "selected_media_bytes": 8,
                "entries": [entry(media + ".jpg", "image", "submit_img", "content_routed_to_image_worker"),
                    entry(media + ".mov", "video", "submit_vid", "content_routed_to_video_worker"),
                    entry(media + ".jpg.xmp", "sidecar", "preserve_sidecar", "metadata_sidecar_not_a_separate_media_task")],
                "relationships": [["kind": "live_photo_filename", "source_relative_path": media + ".jpg",
                    "match_basis": "same_directory_stem", "status": "candidate",
                    "candidate_relative_paths": [media + ".mov"], "candidate_count": 1,
                    "candidates_truncated": false, "verified": false],
                    ["kind": "xmp_filename", "source_relative_path": media + ".jpg.xmp", "match_basis": "exact_filename",
                        "status": "candidate", "candidate_relative_paths": [media + ".jpg"], "candidate_count": 1,
                        "candidates_truncated": false, "verified": false]]]],
            "workers": ["img", "vid"].map { name in
                ["name": name, "path": "/synthetic/bin/\(name)", "status": "available", "bytes": 4,
                    "executable_check": "unix_mode_bits"] as [String: Any]
            }]
    }

    static func selfTest() -> [String] {
        var failures: [String] = []
        if displayPath("a\n\t\\n.jpg") != "a\\n\\t\\\\n.jpg" {
            failures.append("filename control characters were hidden or confused with literal escapes")
        }
        func check(_ name: String, _ object: [String: Any], accepted: Bool = false) {
            do {
                let data = try JSONSerialization.data(withJSONObject: object)
                _ = try PreflightSnapshot.decode(data, expectedInput: "/synthetic/preflight")
                if !accepted { failures.append("invalid preflight accepted: \(name)") }
            } catch {
                if accepted { failures.append("valid preflight rejected: \(name): \(error.localizedDescription)") }
            }
        }
        let original = fixture()
        check("synthetic routing", original, accepted: true)
        guard let plans = original["plans"] as? [[String: Any]], var plan = plans.first,
              let entries = plan["entries"] as? [[String: Any]],
              let relationships = plan["relationships"] as? [[String: Any]] else {
            return failures + ["invalid preflight test fixture"]
        }
        for (name, value) in [("schema_version", 1), ("execution_started", true)] as [(String, Any)] {
            var object = original
            object[name] = value
            check(name, object)
        }
        var missing = original
        missing.removeValue(forKey: "workers")
        check("missing workers field", missing)
        missing["workers"] = []
        check("missing selected worker", missing)
        for (name, value) in [("selected_images", 2), ("selected_media_bytes", 9),
            ("input", "/different/input"), ("output_requested", "relative/output")] as [(String, Any)] {
            var changed = plan
            changed[name] = value
            var object = original
            object["plans"] = [changed]
            check(name, object)
        }
        for path in ["../outside.jpg", "/outside.jpg", "inside/../outside.jpg", "name\0.jpg",
            entries[1]["relative_path"] as? String ?? ""] {
            var changed = entries
            changed[0]["relative_path"] = path
            var changedPlan = plan
            changedPlan["entries"] = changed
            var object = original
            object["plans"] = [changedPlan]
            check("relative path \(path)", object)
        }
        for (name, value) in [("candidate_count", 2), ("verified", true), ("candidates_truncated", true),
            ("candidate_relative_paths", ["foreign.mov"])] as [(String, Any)] {
            var changed = relationships
            changed[0][name] = value
            var changedPlan = plan
            changedPlan["relationships"] = changed
            var object = original
            object["plans"] = [changedPlan]
            check(name, object)
        }
        var canonicalEntries = entries
        for path in ["\u{00e9}.txt", "e\u{0301}.txt"] {
            canonicalEntries.append(["relative_path": path, "route": "copy_only", "selected": false,
                "bytes": 1, "action": "copy_after_success",
                "reason": "non_media_copy_is_conditional_on_successful_adjacent_processing"])
        }
        var canonicalPlan = plan
        canonicalPlan["entries"] = canonicalEntries
        canonicalPlan["files_discovered"] = canonicalEntries.count
        canonicalPlan["other_files"] = 2
        var canonicalReport = original
        canonicalReport["plans"] = [canonicalPlan]
        check("byte-distinct Unicode paths", canonicalReport, accepted: true)
        var boundaryEntries = entries
        for index in 0..<1_997 {
            boundaryEntries.append(["relative_path": "other\(index).txt", "route": "copy_only",
                "selected": false, "bytes": 1, "action": "copy_after_success",
                "reason": "non_media_copy_is_conditional_on_successful_adjacent_processing"])
        }
        plan["entries"] = boundaryEntries
        plan["files_discovered"] = boundaryEntries.count
        plan["other_files"] = 1_997
        var boundaryReport = original
        boundaryReport["plans"] = [plan]
        check("2,000-entry limit", boundaryReport, accepted: true)
        boundaryEntries.append(boundaryEntries[0])
        plan["entries"] = boundaryEntries
        plan["files_discovered"] = boundaryEntries.count
        boundaryReport["plans"] = [plan]
        check("2,001 entries", boundaryReport)
        do {
            _ = try PreflightSnapshot.decode(Data(repeating: 32, count: 1_048_577), expectedInput: "/synthetic/preflight")
            failures.append("oversized preflight accepted")
        } catch { }
        return failures
    }

    func showTestFixture() {
        do {
            try showReport(data: JSONSerialization.data(withJSONObject: Self.fixture()), expectedInput: "/synthetic/preflight")
        } catch { showError(error.localizedDescription) }
    }
}
