import AppKit

@MainActor
final class ProcessingGuideController: NSWindowController, NSTableViewDataSource, NSTableViewDelegate {
    private static let sections: [(String, Int)] = [
        ("policy", 4), ("routes", 4), ("search", 5), ("safety", 4), ("results", 4), ("tools", 4),
    ]
    private let text: (String) -> String
    private let navigation = NSTableView()
    private let header = NSTextField(labelWithString: "")
    private let contextLabel = NSTextField(wrappingLabelWithString: "")
    private let footer = NSTextField(wrappingLabelWithString: "")
    private let textView = NSTextView()
    private let documentScroll = NSScrollView()
    private let findButton = NSButton()
    private var didPosition = false

    convenience init() {
        self.init(text: { localized($0) })
    }

    private init(text: @escaping (String) -> String) {
        self.text = text
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 650),
            styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        super.init(window: window)
        window.title = text("helpguide.title")
        window.isReleasedWhenClosed = false
        buildInterface()
        window.contentMinSize = NSSize(width: 760, height: 520)
        window.setContentSize(NSSize(width: 900, height: 650))
    }

    required init?(coder: NSCoder) { nil }

    func show(relativeTo parent: NSWindow?, context: String = "", topic: String? = nil) {
        window?.title = text("helpguide.title")
        header.stringValue = text("helpguide.title")
        footer.stringValue = text("helpguide.boundary")
        navigation.setAccessibilityLabel(text("helpguide.topics"))
        textView.setAccessibilityLabel(text("helpguide.content"))
        findButton.toolTip = text("helpguide.find")
        findButton.setAccessibilityLabel(text("helpguide.find"))
        navigation.reloadData()
        if let topic, let row = Self.sections.firstIndex(where: { $0.0 == topic }) {
            navigation.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
        displaySection(max(0, navigation.selectedRow))
        contextLabel.stringValue = context.isEmpty ? text("helpguide.context") : context
        window?.appearance = parent?.appearance
        if !didPosition {
            window?.center()
            didPosition = true
        }
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    private func buildInterface() {
        let content = NSView()
        window?.contentView = content
        header.stringValue = text("helpguide.title")
        header.font = .systemFont(ofSize: 19, weight: .semibold)
        contextLabel.stringValue = text("helpguide.context")
        contextLabel.font = .systemFont(ofSize: 12)
        contextLabel.textColor = .secondaryLabelColor
        contextLabel.isSelectable = true
        contextLabel.maximumNumberOfLines = 3
        contextLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        navigation.headerView = nil
        navigation.delegate = self
        navigation.dataSource = self
        navigation.rowHeight = 40
        navigation.style = .sourceList
        navigation.allowsEmptySelection = false
        navigation.focusRingType = .default
        navigation.addTableColumn(NSTableColumn(identifier: NSUserInterfaceItemIdentifier("topic")))
        navigation.setAccessibilityLabel(text("helpguide.topics"))
        let sidebar = NSScrollView()
        sidebar.documentView = navigation
        sidebar.hasVerticalScroller = true
        sidebar.borderType = .noBorder

        textView.isEditable = false
        textView.isSelectable = true
        textView.usesFindPanel = false
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.isRichText = false
        textView.drawsBackground = false
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.minSize = .zero
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        textView.textContainerInset = NSSize(width: 18, height: 12)
        textView.setAccessibilityLabel(text("helpguide.content"))
        documentScroll.documentView = textView
        documentScroll.hasVerticalScroller = true
        documentScroll.hasHorizontalScroller = false
        documentScroll.borderType = .noBorder

        findButton.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: text("helpguide.find"))
        findButton.bezelStyle = .texturedRounded
        findButton.imagePosition = .imageOnly
        findButton.title = ""
        findButton.toolTip = text("helpguide.find")
        findButton.setAccessibilityLabel(text("helpguide.find"))
        findButton.target = self
        findButton.action = #selector(findInTopic(_:))
        findButton.tag = Int(NSFindPanelAction.showFindPanel.rawValue)

        let divider = NSBox()
        divider.boxType = .separator
        footer.stringValue = text("helpguide.boundary")
        footer.font = .systemFont(ofSize: 11)
        footer.textColor = .secondaryLabelColor
        footer.isSelectable = true

        for view in [header, contextLabel, sidebar, divider, documentScroll, footer, findButton] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }
        NSLayoutConstraint.activate([
            content.widthAnchor.constraint(greaterThanOrEqualToConstant: 760),
            content.heightAnchor.constraint(greaterThanOrEqualToConstant: 520),
            header.topAnchor.constraint(equalTo: content.topAnchor, constant: 18),
            header.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 20),
            header.trailingAnchor.constraint(lessThanOrEqualTo: findButton.leadingAnchor, constant: -12),
            findButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            findButton.centerYAnchor.constraint(equalTo: header.centerYAnchor),
            findButton.widthAnchor.constraint(equalToConstant: 28),
            findButton.heightAnchor.constraint(equalToConstant: 28),
            contextLabel.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 7),
            contextLabel.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            contextLabel.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            sidebar.topAnchor.constraint(equalTo: contextLabel.bottomAnchor, constant: 14),
            sidebar.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 8),
            sidebar.widthAnchor.constraint(equalToConstant: 182),
            sidebar.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -12),
            divider.leadingAnchor.constraint(equalTo: sidebar.trailingAnchor, constant: 4),
            divider.topAnchor.constraint(equalTo: sidebar.topAnchor),
            divider.bottomAnchor.constraint(equalTo: sidebar.bottomAnchor),
            divider.widthAnchor.constraint(equalToConstant: 1),
            documentScroll.leadingAnchor.constraint(equalTo: divider.trailingAnchor, constant: 2),
            documentScroll.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -8),
            documentScroll.topAnchor.constraint(equalTo: sidebar.topAnchor),
            documentScroll.bottomAnchor.constraint(equalTo: sidebar.bottomAnchor),
            footer.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            footer.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            footer.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -14),
        ])
        navigation.reloadData()
        navigation.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        displaySection(0)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { Self.sections.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard Self.sections.indices.contains(row) else { return nil }
        let label = NSTextField(wrappingLabelWithString: text("helpguide.\(Self.sections[row].0).title"))
        label.font = .systemFont(ofSize: 12, weight: .medium)
        label.maximumNumberOfLines = 2
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return label
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        displaySection(navigation.selectedRow)
    }

    @objc private func findInTopic(_ sender: NSButton) {
        guard window?.makeFirstResponder(textView) == true else {
            NSSound.beep()
            return
        }
        textView.performFindPanelAction(sender)
    }

    private func displaySection(_ row: Int) {
        guard Self.sections.indices.contains(row) else { return }
        let (section, blocks) = Self.sections[row]
        let result = NSMutableAttributedString(string: "")
        func append(_ value: String, heading: Bool) {
            let paragraph = NSMutableParagraphStyle()
            paragraph.paragraphSpacing = heading ? 5 : 16
            paragraph.lineSpacing = 3
            result.append(NSAttributedString(string: value + "\n", attributes: [
                .font: NSFont.systemFont(ofSize: 13, weight: heading ? .semibold : .regular),
                .foregroundColor: NSColor.labelColor,
                .paragraphStyle: paragraph,
            ]))
        }
        append(text("helpguide.\(section).title"), heading: true)
        append(text("helpguide.\(section).intro"), heading: false)
        for index in 1...blocks {
            append(text("helpguide.\(section).\(index).title"), heading: true)
            append(text("helpguide.\(section).\(index).body"), heading: false)
        }
        textView.textStorage?.setAttributedString(result)
        textView.scrollRangeToVisible(NSRange(location: 0, length: 0))
    }

    static func selfTest() throws {
        func require(_ value: Bool, _ reason: String) throws {
            guard value else { throw NSError(domain: "MFBProcessingGuide", code: 1,
                userInfo: [NSLocalizedDescriptionKey: reason]) }
        }
        var keys = ["helpguide.title", "helpguide.context", "helpguide.topics", "helpguide.content", "helpguide.boundary", "helpguide.find"]
        keys += ["settings.imgFallback", "settings.imgFallback.note", "settings.imgToolPolicy",
                 "settings.imgToolPolicy.note", "settings.recovery.guide"]
        for (section, count) in sections {
            keys += ["helpguide.\(section).title", "helpguide.\(section).intro"]
            for index in 1...count {
                keys += ["helpguide.\(section).\(index).title", "helpguide.\(section).\(index).body"]
            }
        }
        for language in ["en", "zh-Hans", "ja"] {
            guard let path = Bundle.main.path(forResource: language, ofType: "lproj"),
                  let bundle = Bundle(path: path) else {
                try require(false, "Missing processing guide bundle: \(language)")
                continue
            }
            let translate: (String) -> String = { bundle.localizedString(forKey: $0, value: $0, table: nil) }
            for key in keys {
                let value = translate(key)
                try require(!value.isEmpty && value != key, "Missing processing guide text: \(language)/\(key)")
            }
            let controller = ProcessingGuideController(text: translate)
            guard let window = controller.window, let content = window.contentView else {
                try require(false, "Missing processing guide window")
                continue
            }
            window.setContentSize(NSSize(width: 760, height: 520))
            content.layoutSubtreeIfNeeded()
            guard let textContainer = controller.textView.textContainer else {
                try require(false, "Missing processing guide text container")
                continue
            }
            for row in sections.indices {
                controller.displaySection(row)
                controller.textView.layoutManager?.ensureLayout(for: textContainer)
                try require(controller.documentScroll.frame.width >= 480
                    && controller.documentScroll.frame.height >= 300,
                    "Processing guide content is clipped: \(controller.documentScroll.frame), window \(content.bounds)")
                try require(!controller.textView.string.isEmpty
                    && !controller.textView.string.contains("helpguide."), "Processing guide section is empty")
                try require(controller.textView.textContainer?.widthTracksTextView == true
                    && !controller.documentScroll.hasHorizontalScroller, "Processing guide does not wrap")
            }
            try require(controller.findButton.target === controller
                && controller.findButton.action == #selector(findInTopic(_:))
                && controller.textView.usesFindBar
                && controller.textView.isIncrementalSearchingEnabled,
                "Processing guide Find is not connected")
            try require(window.styleMask.contains(.closable)
                && window.standardWindowButton(.closeButton) != nil,
                "Processing guide standard close control is missing")
            controller.show(relativeTo: nil, topic: "tools")
            try require(controller.textView.string.hasPrefix(translate("helpguide.tools.title")),
                "Recovery guide shortcut did not select the tools topic")
            controller.close()
        }
    }
}
