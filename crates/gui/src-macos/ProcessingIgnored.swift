import AppKit

struct HistoryIgnoredSource: Codable {
    let schema_version: Int
    let pipeline: String
    let run_id: String
    let invocation_id: String
    let source_path: String
    let absolute_path: String
    let reason_code: String
    let message: String
    let ordinal: Int
    let total: Int

    static let prefix = "MFB_FAST_IMG_IGNORED="

    static func decode(_ line: String) throws -> Self? {
        guard line.hasPrefix(prefix) else { return nil }
        let data = Data(line.dropFirst(prefix.utf8.count).utf8)
        guard data.count <= 64 * 1024 - prefix.utf8.count - 1,
              let fields = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(fields.keys) == Set(["schema_version", "pipeline", "run_id", "invocation_id", "source_path",
                                       "absolute_path", "reason_code", "message", "ordinal", "total"])
        else { throw invalid() }
        let value = try JSONDecoder().decode(Self.self, from: data)
        guard value.schema_version == 1, value.pipeline == "img",
              validHistoryIdentity(value.run_id), validHistoryIdentity(value.invocation_id),
              validHistoryIdentity(value.reason_code), !value.message.isEmpty, !value.message.contains("\0"),
              !value.source_path.isEmpty, !value.source_path.hasPrefix("/"), !value.source_path.contains("\0"),
              value.source_path.split(separator: "/", omittingEmptySubsequences: false).allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }),
              value.absolute_path.hasPrefix("/"), !value.absolute_path.contains("\0"),
              value.ordinal > 0, value.total >= value.ordinal else { throw invalid() }
        return value
    }

    static func invalid() -> HostError { HostError(message: localized("history.ignored.invalid")) }

    var userReason: String {
        let key = "history.ignored.reason." + reason_code
        let explanation = localized(key)
        return explanation == key ? message + " [\(reason_code)]" : explanation
    }

    var searchable: String {
        [source_path, absolute_path, reason_code, message, userReason, run_id, invocation_id].joined(separator: "\n")
    }

    func recordedJSON() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(self)
    }

    // Validate the complete recorded ordinal set, not a count of matching log lines.
    static func reconcile(_ observations: [Self], expected: Int?) throws -> [Self] {
        guard let expected, expected >= 0 else { throw invalid() }
        if observations.isEmpty {
            guard expected == 0 else { throw HostError(message: localized("history.ignored.not_recorded")) }
            return []
        }
        var result: [Self] = []
        for records in Dictionary(grouping: observations, by: \.invocation_id).values {
            var ordinals: [Int: Self] = [:]
            for record in records {
                if let previous = ordinals[record.ordinal] {
                    guard try previous.recordedJSON() == record.recordedJSON() else { throw invalid() }
                } else { ordinals[record.ordinal] = record }
            }
            let rows = ordinals.values.sorted { $0.ordinal < $1.ordinal }
            guard let total = rows.first?.total, total == rows.count,
                  rows.allSatisfy({ $0.total == total }),
                  rows.map(\.ordinal) == Array(1...total),
                  Set(rows.map { Data($0.source_path.utf8) }).count == rows.count else { throw invalid() }
            result += rows
        }
        guard result.count == expected else { throw invalid() }
        return result.sorted {
            if !$0.invocation_id.utf8.elementsEqual($1.invocation_id.utf8) { return $0.invocation_id < $1.invocation_id }
            return $0.ordinal < $1.ordinal
        }
    }
}

final class HistoryIgnoredView: NSStackView, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    private let search = NSSearchField()
    private let category = NSPopUpButton(frame: .zero, pullsDown: false)
    private let status = NSTextField(wrappingLabelWithString: "")
    private let table = NSTableView()
    private let copyButton = NSButton()
    private let revealButton = NSButton()
    private let note = NSTextField(wrappingLabelWithString: "")
    private var records: [HistoryIgnoredSource] = []
    private var displayed: [HistoryIgnoredSource] = []
    private var codes: [String] = []
    private var classifiedCount = 0
    private var available = false
    private var enabled = true

    init() {
        super.init(frame: .zero)
        orientation = .vertical
        alignment = .leading
        spacing = 6
        search.delegate = self
        search.sendsSearchStringImmediately = true
        search.setContentHuggingPriority(.defaultLow, for: .horizontal)
        category.target = self
        category.action = #selector(categoryChanged)
        category.controlSize = .small
        category.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        category.cell?.lineBreakMode = .byTruncatingTail
        copyButton.image = NSImage(systemSymbolName: "doc.on.doc", accessibilityDescription: nil)
        copyButton.target = self
        copyButton.action = #selector(copyRecord)
        revealButton.image = NSImage(systemSymbolName: "folder", accessibilityDescription: nil)
        revealButton.target = self
        revealButton.action = #selector(revealRecord)
        for button in [copyButton, revealButton] { button.bezelStyle = .rounded; button.setButtonType(.momentaryPushIn) }
        let tools = NSStackView(views: [search, copyButton, revealButton])
        tools.spacing = 4
        addArrangedSubview(tools)
        addArrangedSubview(category)
        addArrangedSubview(status)
        table.delegate = self
        table.dataSource = self
        table.rowHeight = HistoryLayout.rowHeight
        table.usesAlternatingRowBackgroundColors = true
        table.style = .inset
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        for key in ["path", "reason"] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(key))
            column.width = 200
            column.minWidth = 100
            table.addTableColumn(column)
        }
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = table
        scroll.setContentHuggingPriority(.defaultLow, for: .vertical)
        addArrangedSubview(scroll)
        note.font = HistoryLayout.smallFont
        note.textColor = .secondaryLabelColor
        addArrangedSubview(note)
        for view in [tools, category, status, scroll, note] { view.widthAnchor.constraint(equalTo: widthAnchor).isActive = true }
        search.widthAnchor.constraint(greaterThanOrEqualToConstant: 100).isActive = true
        updateLocalization()
        reset(localized("history.ignored.loading"))
    }

    required init?(coder: NSCoder) { nil }

    func updateLocalization() {
        search.placeholderString = localized("history.ignored.search")
        search.setAccessibilityLabel(localized("history.ignored.search"))
        category.setAccessibilityLabel(localized("history.ignored.category"))
        table.setAccessibilityLabel(localized("history.ignored.tab"))
        table.tableColumns.first?.title = localized("history.failed_files.path")
        table.tableColumns.last?.title = localized("history.failed_files.reason")
        for (button, key) in [(copyButton, "history.ignored.copy"), (revealButton, "history.failed_files.reveal")] {
            button.toolTip = localized(key); button.setAccessibilityLabel(localized(key))
        }
        note.stringValue = localized("history.ignored.note")
        rebuildCategories()
        if available { applyFilter() }
    }

    func reset(_ message: String) {
        records = []; displayed = []; available = false
        search.stringValue = ""
        rebuildCategories()
        table.deselectAll(nil); table.reloadData()
        status.stringValue = message
        updateActions()
    }

    func show(_ values: [HistoryIgnoredSource]) {
        records = values; available = true
        rebuildCategories()
        applyFilter()
    }

    func setEnabled(_ value: Bool) { enabled = value; updateActions() }

    private func rebuildCategories() {
        let selected = codes.indices.contains(category.indexOfSelectedItem - 1) ? codes[category.indexOfSelectedItem - 1] : nil
        let groups = Dictionary(grouping: records, by: \.reason_code)
        classifiedCount = groups.values.reduce(0) { $0 + $1.count }
        codes = groups.keys.sorted()
        category.removeAllItems()
        category.addItem(withTitle: localized("history.ignored.all_reasons"))
        for code in codes {
            category.addItem(withTitle: (groups[code]?.first?.userReason ?? code) + " (\(groups[code]?.count ?? 0))")
        }
        if let selected, let index = codes.firstIndex(of: selected) { category.selectItem(at: index + 1) }
    }

    private func applyFilter() {
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let code = codes.indices.contains(category.indexOfSelectedItem - 1) ? codes[category.indexOfSelectedItem - 1] : nil
        let matches = records.filter { (code == nil || $0.reason_code == code) && (query.isEmpty || $0.searchable.localizedCaseInsensitiveContains(query)) }
        displayed = []
        var bytes = 0
        for record in matches {
            let size = record.searchable.utf8.count
            guard displayed.count < 1_000, size <= 2 * 1024 * 1024 - bytes else { continue }
            displayed.append(record); bytes += size
        }
        table.deselectAll(nil); table.reloadData()
        status.stringValue = localized("history.ignored.counts", records.count, classifiedCount, displayed.count, matches.count)
        updateActions()
    }

    private var selected: HistoryIgnoredSource? { displayed.indices.contains(table.selectedRow) ? displayed[table.selectedRow] : nil }
    private var selectedURL: URL? {
        guard let record = selected else { return nil }
        let url = URL(fileURLWithPath: record.absolute_path)
        guard let properties = try? url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]),
              properties.isRegularFile == true, properties.isSymbolicLink != true else { return nil }
        return url
    }
    private func updateActions() {
        search.isEnabled = enabled && available; category.isEnabled = enabled && available
        table.isEnabled = enabled && available
        copyButton.isEnabled = enabled && selected != nil
        revealButton.isEnabled = enabled && selectedURL != nil
    }
    func controlTextDidChange(_ notification: Notification) { applyFilter() }
    @objc private func categoryChanged() { applyFilter() }
    func numberOfRows(in tableView: NSTableView) -> Int { displayed.count }
    func tableViewSelectionDidChange(_ notification: Notification) { updateActions() }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard displayed.indices.contains(row) else { return nil }
        let record = displayed[row]
        let text = tableColumn?.identifier.rawValue == "path" ? record.source_path : record.userReason
        let field = NSTextField(labelWithString: text)
        field.lineBreakMode = .byTruncatingMiddle
        field.toolTip = [record.source_path, record.absolute_path, record.userReason, record.message, record.reason_code, record.invocation_id].joined(separator: "\n")
        field.setAccessibilityLabel(text)
        return field
    }
    @objc private func copyRecord() {
        guard enabled, let record = selected else { return }
        do {
            let data = try record.recordedJSON()
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(record.userReason + "\n" + String(decoding: data, as: UTF8.self), forType: .string)
            status.stringValue = localized("history.ignored.copied")
        } catch { status.stringValue = error.localizedDescription }
    }
    @objc private func revealRecord() {
        guard enabled, let url = selectedURL else { status.stringValue = localized("history.failed_files.unavailable"); return }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    func validateForSelfTest(_ values: [HistoryIgnoredSource]) throws {
        show(values)
        guard displayed.count == values.count, classifiedCount == records.count, codes.count == 2,
              table.enclosingScrollView?.hasHorizontalScroller == false else {
            throw HostError(message: "Ignored categories lost records or changed their total")
        }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        updateActions()
        guard copyButton.isEnabled, !revealButton.isEnabled else {
            throw HostError(message: "Ignored actions lost a recorded path or enabled an absent source")
        }
        search.stringValue = values[1].userReason
        applyFilter()
        guard displayed.count == 1, displayed[0].ordinal == values[1].ordinal,
              !copyButton.isEnabled, !revealButton.isEnabled else {
            throw HostError(message: "Localized ignored search failed or retained stale selection actions")
        }
        search.stringValue = ""
        category.selectItem(at: 1)
        applyFilter()
        guard displayed.allSatisfy({ $0.reason_code == codes[0] }), !displayed.isEmpty else {
            throw HostError(message: "Ignored category filtering mixed different reasons")
        }
        let large = (1...1_001).map { index in
            HistoryIgnoredSource(schema_version: 1, pipeline: "img", run_id: values[0].run_id,
                invocation_id: values[0].invocation_id, source_path: "large-\(index).png",
                absolute_path: "/synthetic/absent/large-\(index).png", reason_code: values[0].reason_code,
                message: values[0].message, ordinal: index, total: 1_001)
        }
        show(large)
        guard displayed.count == 1_000, records.count == 1_001 else {
            throw HostError(message: "Ignored table exceeded its display budget or changed its inventory")
        }
        search.stringValue = "large-1001.png"
        applyFilter()
        guard displayed.count == 1, displayed[0].ordinal == 1_001 else {
            throw HostError(message: "Ignored search only searched the currently visible rows")
        }
        reset(localized("history.ignored.not_recorded"))
        guard displayed.isEmpty, !copyButton.isEnabled, !revealButton.isEnabled, !search.isEnabled else {
            throw HostError(message: "Absent ignored evidence retained usable actions or old records")
        }
    }
}
