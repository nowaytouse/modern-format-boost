import AppKit
import Foundation
import SQLite3

private enum HistoryLayout {
    static let margin: CGFloat = 16
    static let spacing: CGFloat = 10
    static let rowHeight: CGFloat = 28
    static let buttonSize: CGFloat = 28
    static let bodyFont = NSFont.systemFont(ofSize: NSFont.systemFontSize)
    static let smallFont = NSFont.systemFont(ofSize: NSFont.smallSystemFontSize)
    static let detailFont = NSFont.monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
}

private struct HistoryContext: Decodable, Equatable {
    let schema_version: Int
    let inputs: [String]
    let output: String?
    let backup: String?
    let mode: String
    let dry_run: Bool
    let in_place: Bool
    let shortest_path: Bool
}

private struct HistoryMedia: Decodable, Equatable {
    let active: Bool
    let succeeded: Int?
    let skipped: Int?
    let ignored: Int?
    let failed: Int?
    let unprocessed: Int?
    let exit_code: Int?

    var valid: Bool {
        if !active {
            return [succeeded, skipped, ignored, failed, unprocessed, exit_code].allSatisfy { $0 == nil }
        }
        let values = [succeeded, skipped, ignored, failed, unprocessed, exit_code].compactMap { $0 }
        guard values.allSatisfy({ $0 >= 0 }) else { return false }
        var total = 0
        for value in [succeeded, skipped, ignored, failed, unprocessed].compactMap({ $0 }) {
            let sum = total.addingReportingOverflow(value)
            guard !sum.overflow else { return false }
            total = sum.partialValue
        }
        return true
    }

    var complete: Bool { !active || [succeeded, skipped, ignored, failed, unprocessed, exit_code].allSatisfy { $0 != nil } }
}

private struct HistorySummary: Decodable, Equatable {
    struct Integrity: Decodable, Equatable {
        let state: String?
        let issue_count: Int?
    }
    let schema_version: Int
    let count_scope: String
    let img: HistoryMedia
    let vid: HistoryMedia
    let integrity: Integrity
    let failed_files: [String]
    let skipped_files: [String]

    var valid: Bool {
        guard schema_version == 1, count_scope == "processor_outcomes", img.valid, vid.valid,
              integrity.issue_count.map({ $0 >= 0 }) ?? true else { return false }
        for pair in [(img.succeeded, vid.succeeded), (img.skipped, vid.skipped),
                     (img.ignored, vid.ignored), (img.failed, vid.failed), (img.unprocessed, vid.unprocessed)] {
            if let left = pair.0, let right = pair.1, left.addingReportingOverflow(right).overflow { return false }
        }
        return true
    }
}

private struct HistoryFinished: Decodable, Equatable {
    let schema_version: Int
    let outcome: String
    let error: String?
}

private struct HistoryVerification: Decodable, Equatable {
    let schema_version: Int
    let has_warnings: Bool
    let issue_count: Int
    let source_count: Int
    let optimized_count: Int
    let skipped_count: Int
    let failed_count: Int
    let source_remaining_count: Int
    let verified_deleted_count: Int
    let count_status: String?
    let source_path: String?
    let output_path: String?

    var valid: Bool {
        schema_version == 1 && [issue_count, source_count, optimized_count, skipped_count, failed_count,
                               source_remaining_count, verified_deleted_count].allSatisfy { $0 >= 0 }
    }
}

private struct HistoryRecord: Decodable {
    let ts: String
    let event: String
}

private struct HistoryEntry {
    let id: String
    let stamp: String
    let audit: URL
    var timestamp: String
    var context: HistoryContext?
    var summary: HistorySummary?
    var verification: HistoryVerification?
    var finished: HistoryFinished?
    var legacy = false
    var legacyEnded = false
    var issues: [String] = []
    var logs: [URL] = []
    var contradictoryFields: Set<String> = []

    var hasProcessorError: Bool {
        [summary?.img, summary?.vid].compactMap { $0 }.contains { $0.active && ($0.exit_code ?? 0) != 0 }
    }
    var hasFailures: Bool {
        (summary?.img.failed ?? 0) > 0 || (summary?.vid.failed ?? 0) > 0
            || hasProcessorError
    }
    var hasVerificationWarnings: Bool {
        verification?.has_warnings == true || verification?.count_status?.hasPrefix("MISMATCH") == true
            || (summary?.integrity.issue_count ?? 0) > 0 || summary?.integrity.state == "WARNINGS"
    }
    var hasPending: Bool { (summary?.img.unprocessed ?? 0) > 0 || (summary?.vid.unprocessed ?? 0) > 0 }
    var statusKey: String {
        if finished?.outcome == "failed" { return "history.status.failed" }
        if finished?.outcome == "cancelled" { return "history.status.cancelled" }
        if hasProcessorError { return "history.status.failed" }
        if legacy { return "history.status.legacy" }
        if issues.isEmpty, context?.dry_run == true, finished?.outcome == "completed" {
            return "history.status.preview"
        }
        guard issues.isEmpty, context != nil, let summary, summary.img.complete, summary.vid.complete,
              finished?.outcome == "completed" else { return "history.status.incomplete" }
        if hasFailures { return "history.status.file_failures" }
        if hasPending { return "history.status.unfinished" }
        if hasVerificationWarnings { return "history.status.verification_warnings" }
        return "history.status.completed"
    }
    var needsAttention: Bool {
        !["history.status.completed", "history.status.preview"].contains(statusKey) || hasVerificationWarnings
    }
    var statusColor: NSColor {
        if finished?.outcome == "failed" || hasFailures { return .systemRed }
        return needsAttention ? .systemOrange : .labelColor
    }
    var searchable: String {
        ([stamp, timestamp, context?.mode ?? "", localized(statusKey)] + (context?.inputs ?? [])
            + [context?.output ?? "", context?.backup ?? "", finished?.error ?? ""])
            .joined(separator: " ")
    }
    var succeededLabel: String {
        guard let summary else { return localized("result.unknown") }
        let active = [summary.img, summary.vid].filter(\.active)
        guard !active.isEmpty, active.allSatisfy({ $0.succeeded != nil }) else { return localized("result.unknown") }
        var total = 0
        for media in active {
            let sum = total.addingReportingOverflow(media.succeeded!)
            guard !sum.overflow else { return localized("result.unknown") }
            total = sum.partialValue
        }
        return String(total)
    }
}

private struct HistoryAudit {
    let entries: [HistoryEntry]
    let hasEnd: Bool

    var evidenceWeight: Int {
        entries.reduce(0) { total, entry in
            total + (entry.context == nil ? 0 : 1) + (entry.summary == nil ? 0 : 1)
                + (entry.verification == nil ? 0 : 1) + (entry.finished == nil ? 0 : 1)
        }
    }
}

private enum HistoryStore {
    static let maximumAuditBytes = 8 * 1024 * 1024
    static let maximumSessions = 100
    static let maximumDirectoryItems = 20_000
    static let maximumAuditsPerSession = 8
    static let maximumEntriesPerSession = 1_000
    static let maximumRecordsPerSession = 100_000
    static let maximumScanBytes = 64 * 1024 * 1024

    struct Candidate {
        let stamp: String
        let url: URL
        let archived: Bool
        let modified: Date
    }
    struct Result {
        let entries: [HistoryEntry]
        let issues: [String]
        let limited: Bool
    }

    static func regularFile(_ url: URL) -> Bool {
        guard let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey]) else { return false }
        return values.isRegularFile == true && values.isSymbolicLink != true
    }

    static func parse(data: Data, stamp: String, audit: URL) -> HistoryAudit {
        var current = HistoryEntry(id: stamp + ":0", stamp: stamp, audit: audit, timestamp: stamp)
        guard data.count <= maximumAuditBytes else {
            current.issues.append(localized("history.issue.too_large"))
            return HistoryAudit(entries: [current], hasEnd: false)
        }
        guard let text = String(data: data, encoding: .utf8) else {
            current.issues.append(localized("history.issue.encoding"))
            return HistoryAudit(entries: [current], hasEnd: false)
        }
        let decoder = JSONDecoder()
        return parse(records: text.split(separator: "\n", omittingEmptySubsequences: true).lazy.map {
            try? decoder.decode(HistoryRecord.self, from: Data($0.utf8))
        }, stamp: stamp, audit: audit)
    }

    private static func parse<S: Sequence>(records: S, stamp: String, audit: URL) -> HistoryAudit
        where S.Element == HistoryRecord? {
        var entries: [HistoryEntry] = []
        var current = HistoryEntry(id: stamp + ":0", stamp: stamp, audit: audit, timestamp: stamp)
        var hasContext = false
        var sawRecord = false
        let decoder = JSONDecoder()
        func decode<T: Decodable>(_ type: T.Type, from event: String, prefix: String) throws -> T {
            try decoder.decode(type, from: Data(event.dropFirst(prefix.count).utf8))
        }
        for (index, record) in records.enumerated() {
            guard index < maximumRecordsPerSession else {
                current.issues.append(localized("history.issue.entry_limit")); break
            }
            guard let record else {
                if current.issues.count < 32 { current.issues.append(localized("history.issue.record", index + 1)) }
                continue
            }
            if !sawRecord { current.timestamp = record.ts; sawRecord = true }
            let event = record.event
            do {
                if event.hasPrefix("MFB_HISTORY_CONTEXT=") {
                    if hasContext {
                        guard entries.count < maximumEntriesPerSession - 1 else {
                            current.issues.append(localized("history.issue.entry_limit"))
                            break
                        }
                        entries.append(current)
                        current = HistoryEntry(id: stamp + ":\(entries.count)", stamp: stamp, audit: audit, timestamp: record.ts)
                    }
                    hasContext = true
                    current.legacy = false
                    current.timestamp = record.ts
                    let context = try decode(HistoryContext.self, from: event, prefix: "MFB_HISTORY_CONTEXT=")
                    guard context.schema_version == 1, !context.mode.isEmpty else { throw HistoryParseError.invalid }
                    current.context = context
                    current.legacy = false
                } else if event.hasPrefix("MFB_HISTORY_SUMMARY=") {
                    let summary = try decode(HistorySummary.self, from: event, prefix: "MFB_HISTORY_SUMMARY=")
                    guard summary.valid, !current.contradictoryFields.contains("summary") else { throw HistoryParseError.invalid }
                    if let previous = current.summary, previous != summary {
                        current.summary = nil
                        current.contradictoryFields.insert("summary")
                        throw HistoryParseError.invalid
                    }
                    current.summary = summary
                } else if event.hasPrefix("MFB_HISTORY_VERIFICATION=") {
                    let verification = try decode(HistoryVerification.self, from: event, prefix: "MFB_HISTORY_VERIFICATION=")
                    guard verification.valid, !current.contradictoryFields.contains("verification") else { throw HistoryParseError.invalid }
                    if let previous = current.verification, previous != verification {
                        current.verification = nil
                        current.contradictoryFields.insert("verification")
                        throw HistoryParseError.invalid
                    }
                    current.verification = verification
                } else if event.hasPrefix("MFB_HISTORY_FINISHED=") {
                    let finished = try decode(HistoryFinished.self, from: event, prefix: "MFB_HISTORY_FINISHED=")
                    guard finished.schema_version == 1, ["completed", "failed", "cancelled"].contains(finished.outcome),
                          !current.contradictoryFields.contains("finished")
                    else { throw HistoryParseError.invalid }
                    if let previous = current.finished, previous != finished {
                        current.finished = nil
                        current.contradictoryFields.insert("finished")
                        throw HistoryParseError.invalid
                    }
                    current.finished = finished
                } else if event.hasPrefix("SESSION_COMPLETED") {
                    if !hasContext {
                        current.legacy = true
                        current.legacyEnded = true
                        current.summary = legacySummary(event)
                    }
                } else if event == "SESSION_STARTED", !hasContext {
                    current.legacy = true
                } else if event.hasPrefix("MFB_HISTORY_") {
                    throw HistoryParseError.invalid
                }
            } catch {
                if event.hasPrefix("MFB_HISTORY_SUMMARY=") {
                    current.summary = nil
                    current.contradictoryFields.insert("summary")
                } else if event.hasPrefix("MFB_HISTORY_VERIFICATION=") {
                    current.verification = nil
                    current.contradictoryFields.insert("verification")
                } else if event.hasPrefix("MFB_HISTORY_FINISHED=") {
                    current.finished = nil
                    current.contradictoryFields.insert("finished")
                }
                if current.issues.count < 32 { current.issues.append(localized("history.issue.payload", index + 1)) }
            }
        }
        if !sawRecord { current.issues.append(localized("history.issue.empty")) }
        entries.append(current)
        let hasEnd = entries.allSatisfy {
            $0.issues.isEmpty && ($0.context != nil && $0.finished != nil || $0.legacy && $0.legacyEnded)
        }
        return HistoryAudit(entries: entries, hasEnd: hasEnd)
    }

    private static func legacySummary(_ event: String) -> HistorySummary? {
        func value(_ keys: [String]) -> Int? {
            for key in keys {
                let pattern = "(?:^|\\s)" + NSRegularExpression.escapedPattern(for: key) + "=([^\\s]+)"
                guard let expression = try? NSRegularExpression(pattern: pattern),
                      let match = expression.firstMatch(in: event, range: NSRange(event.startIndex..., in: event)),
                      let range = Range(match.range(at: 1), in: event) else { continue }
                return Int(event[range]).flatMap { $0 >= 0 ? $0 : nil }
            }
            return nil
        }
        func media(_ names: [String]) -> HistoryMedia {
            func count(_ suffix: String) -> Int? { value(names.map { $0 + "_" + suffix }) }
            return HistoryMedia(active: true, succeeded: count("ok"), skipped: count("skip"),
                                ignored: count("ignore"), failed: count("fail"), unprocessed: count("unprocessed"), exit_code: nil)
        }
        let summary = HistorySummary(schema_version: 1, count_scope: "processor_outcomes", img: media(["images", "image"]),
                                     vid: media(["videos", "video"]),
                                     integrity: .init(state: nil, issue_count: value(["integrity_issues"])),
                                     failed_files: [], skipped_files: [])
        return summary.valid ? summary : nil
    }

    private static func databaseError(_ database: OpaquePointer?, _ detail: String? = nil) -> HostError {
        HostError(message: localized("history.issue.read") + " (history.sqlite3: "
                  + (detail ?? String(cString: sqlite3_errmsg(database))) + ")")
    }

    private static func statement(_ database: OpaquePointer, _ sql: String) throws -> OpaquePointer {
        var value: OpaquePointer?
        guard sqlite3_prepare_v2(database, sql, -1, &value, nil) == SQLITE_OK, let value else {
            if let value { sqlite3_finalize(value) }
            throw databaseError(database)
        }
        return value
    }

    private static func databaseText(_ statement: OpaquePointer, _ column: Int32, database: OpaquePointer) throws -> String {
        guard sqlite3_column_type(statement, column) == SQLITE_TEXT,
              let bytes = sqlite3_column_text(statement, column),
              let text = String(data: Data(bytes: bytes, count: Int(sqlite3_column_bytes(statement, column))), encoding: .utf8)
        else { throw databaseError(database, "invalid text record") }
        return text
    }

    private static func databaseAudit(_ database: OpaquePointer, stamp: String, url: URL,
                                      readBytes: inout Int, limited: inout Bool) throws -> HistoryAudit {
        let query = try statement(database, "SELECT ts, event FROM history_events WHERE session_id = ? ORDER BY sequence LIMIT \(maximumRecordsPerSession + 1)")
        defer { sqlite3_finalize(query) }
        guard stamp.withCString({ sqlite3_bind_text(query, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK
        else { throw databaseError(database) }
        var records: [HistoryRecord?] = []
        var bytes = 0
        var issue: String?
        while true {
            let result = sqlite3_step(query)
            if result == SQLITE_DONE { break }
            if result == SQLITE_TOOBIG {
                issue = localized("history.issue.too_large"); limited = true; break
            }
            guard result == SQLITE_ROW else { throw databaseError(database) }
            guard sqlite3_column_type(query, 0) == SQLITE_TEXT, sqlite3_column_type(query, 1) == SQLITE_TEXT else {
                issue = localized("history.issue.record", records.count + 1); break
            }
            let size = Int(sqlite3_column_bytes(query, 0)) + Int(sqlite3_column_bytes(query, 1))
            guard records.count < maximumRecordsPerSession else {
                issue = localized("history.issue.entry_limit"); limited = true; break
            }
            guard size <= maximumAuditBytes - bytes else {
                issue = localized("history.issue.too_large"); limited = true; break
            }
            guard size <= maximumScanBytes - readBytes else {
                issue = localized("history.issue.scan_limit"); limited = true; break
            }
            bytes += size
            readBytes += size
            do {
                let ts = try databaseText(query, 0, database: database)
                let event = try databaseText(query, 1, database: database)
                records.append(event.hasPrefix("MFB_HISTORY_") ? HistoryRecord(ts: ts, event: event) : nil)
            } catch {
                issue = localized("history.issue.encoding"); break
            }
        }
        let parsed = parse(records: records, stamp: stamp, audit: url)
        guard let issue else { return parsed }
        var entries = parsed.entries
        entries[entries.count - 1].issues.append(issue)
        return HistoryAudit(entries: entries, hasEnd: false)
    }

    static func load(directory: URL) throws -> Result {
        let root = directory.standardizedFileURL.resolvingSymlinksInPath()
        let manager = FileManager.default
        var candidates: [Candidate] = []
        var issues: [String] = []
        var inspected = 0
        var limited = false
        var readBytes = 0
        var folderLogs: [String: [URL]] = [:]
        let keys: [URLResourceKey] = [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey, .contentModificationDateKey]
        guard let rootValues = try? root.resourceValues(forKeys: [.isDirectoryKey]), rootValues.isDirectory == true else {
            if !manager.fileExists(atPath: root.path) { return Result(entries: [], issues: [], limited: false) }
            throw HostError(message: localized("history.issue.directory"))
        }
        func scan(_ folder: URL, archived: Bool) throws {
            guard let enumerator = manager.enumerator(at: folder, includingPropertiesForKeys: keys,
                                                       options: [.skipsHiddenFiles, .skipsSubdirectoryDescendants],
                                                       errorHandler: { _, _ in
                issues.append(localized("history.issue.directory")); return false
            }) else { throw HostError(message: localized("history.issue.directory")) }
            var bundles: [URL] = []
            for case let url as URL in enumerator {
                inspected += 1
                guard inspected <= maximumDirectoryItems else { limited = true; break }
                guard let values = try? url.resourceValues(forKeys: Set(keys)) else {
                    if issues.isEmpty { issues.append(localized("history.issue.directory")) }
                    continue
                }
                guard values.isSymbolicLink != true else { continue }
                let name = url.lastPathComponent
                if values.isRegularFile == true, name.hasPrefix("session_audit_"), name.hasSuffix(".jsonl") {
                    let stamp = String(name.dropFirst("session_audit_".count).dropLast(".jsonl".count))
                    guard !stamp.isEmpty else { continue }
                    candidates.append(Candidate(stamp: stamp, url: url, archived: archived,
                                                modified: values.contentModificationDate ?? .distantPast))
                } else if !archived, values.isDirectory == true, name.hasPrefix("Bundle_") {
                    bundles.append(url)
                }
            }
            for bundle in bundles.sorted(by: { $0.lastPathComponent > $1.lastPathComponent }) {
                guard inspected < maximumDirectoryItems else { limited = true; break }
                try scan(bundle, archived: true)
            }
        }
        try scan(root, archived: false)
        let groups = Dictionary(grouping: candidates, by: \.stamp)
        let databaseURL = root.appendingPathComponent("history.sqlite3")
        var database: OpaquePointer?
        defer { if let database { sqlite3_close(database) } }
        var databaseSessions: [String: Date] = [:]
        var legacyStamps = Set(groups.keys)
        if manager.fileExists(atPath: databaseURL.path)
            || (try? databaseURL.resourceValues(forKeys: [.isSymbolicLinkKey]))?.isSymbolicLink == true {
            guard regularFile(databaseURL) else { throw databaseError(nil, "not a regular file") }
            guard sqlite3_open_v2(databaseURL.path, &database, SQLITE_OPEN_READONLY, nil) == SQLITE_OK,
                  let database else { throw databaseError(database) }
            sqlite3_limit(database, SQLITE_LIMIT_LENGTH, Int32(maximumAuditBytes))
            guard sqlite3_exec(database, "BEGIN", nil, nil, nil) == SQLITE_OK else { throw databaseError(database) }
            let version = try statement(database, "PRAGMA user_version")
            defer { sqlite3_finalize(version) }
            guard sqlite3_step(version) == SQLITE_ROW else { throw databaseError(database) }
            let schema = sqlite3_column_int(version, 0)
            guard schema == 1 else { throw databaseError(database, "unsupported schema \(schema)") }
            let eventSchema = try statement(database, "SELECT sequence, session_id, ts, event FROM history_events LIMIT 0")
            sqlite3_finalize(eventSchema)
            let sessions = try statement(database, "SELECT session_id, updated_at FROM history_sessions ORDER BY updated_at DESC, session_id DESC LIMIT \(maximumSessions + 1)")
            defer { sqlite3_finalize(sessions) }
            let formatter = ISO8601DateFormatter()
            let fractionalFormatter = ISO8601DateFormatter()
            fractionalFormatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            while true {
                let result = sqlite3_step(sessions)
                if result == SQLITE_DONE { break }
                guard result == SQLITE_ROW else { throw databaseError(database) }
                if databaseSessions.count == maximumSessions { limited = true; break }
                guard sqlite3_column_type(sessions, 0) == SQLITE_TEXT, sqlite3_column_type(sessions, 1) == SQLITE_TEXT,
                      sqlite3_column_bytes(sessions, 0) <= 1_024, sqlite3_column_bytes(sessions, 1) <= 128 else {
                    throw databaseError(database, "invalid session record")
                }
                let stamp = try databaseText(sessions, 0, database: database)
                let updated = try databaseText(sessions, 1, database: database)
                guard !stamp.isEmpty, !stamp.contains("\0"), databaseSessions[stamp] == nil,
                      let date = fractionalFormatter.date(from: updated) ?? formatter.date(from: updated) else {
                    throw databaseError(database, "invalid session record")
                }
                databaseSessions[stamp] = date
            }
            let membership = try statement(database, "SELECT 1 FROM history_sessions WHERE session_id = ? LIMIT 1")
            defer { sqlite3_finalize(membership) }
            for stamp in groups.keys {
                sqlite3_reset(membership)
                guard stamp.withCString({ sqlite3_bind_text(membership, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK
                else { throw databaseError(database) }
                let result = sqlite3_step(membership)
                if result == SQLITE_ROW { legacyStamps.remove(stamp) }
                else if result != SQLITE_DONE { throw databaseError(database) }
            }
        }
        let stamps = legacyStamps.union(databaseSessions.keys).sorted { left, right in
            let leftDate = databaseSessions[left] ?? groups[left]!.map(\.modified).max() ?? .distantPast
            let rightDate = databaseSessions[right] ?? groups[right]!.map(\.modified).max() ?? .distantPast
            return leftDate == rightDate ? left > right : leftDate > rightDate
        }
        if stamps.count > maximumSessions { limited = true }
        var databaseAudits: [String: HistoryAudit] = [:]
        if let connection = database {
            for stamp in stamps.prefix(maximumSessions) where databaseSessions[stamp] != nil {
                guard readBytes < maximumScanBytes else { limited = true; break }
                databaseAudits[stamp] = try databaseAudit(connection, stamp: stamp, url: databaseURL,
                                                        readBytes: &readBytes, limited: &limited)
            }
            guard sqlite3_exec(connection, "COMMIT", nil, nil, nil) == SQLITE_OK else { throw databaseError(connection) }
            guard sqlite3_close(connection) == SQLITE_OK else { throw databaseError(connection) }
            database = nil
        }
        var entries: [HistoryEntry] = []
        for stamp in stamps.prefix(maximumSessions) {
            if databaseSessions[stamp] == nil, readBytes >= maximumScanBytes { limited = true; continue }
            let options = (groups[stamp] ?? []).sorted {
                if $0.archived != $1.archived { return $0.archived }
                return $0.url.path < $1.url.path
            }
            if options.count > maximumAuditsPerSession { limited = true }
            var selected: (Candidate, HistoryAudit)?
            var conflictingCopies = false
            if databaseSessions[stamp] != nil {
                guard let audit = databaseAudits[stamp] else { limited = true; continue }
                let candidate = options.first ?? Candidate(stamp: stamp, url: databaseURL, archived: false,
                                                            modified: databaseSessions[stamp]!)
                selected = (candidate, audit)
            }
            for candidate in options.prefix(databaseSessions[stamp] == nil ? maximumAuditsPerSession : 0) {
                guard readBytes < maximumScanBytes else { limited = true; break }
                let audit: HistoryAudit
                do {
                    let handle = try FileHandle(forReadingFrom: candidate.url)
                    defer { try? handle.close() }
                    let allowed = min(maximumAuditBytes + 1, maximumScanBytes - readBytes)
                    let data = try handle.read(upToCount: allowed) ?? Data()
                    readBytes += data.count
                    if allowed < maximumAuditBytes + 1 && data.count == allowed {
                        limited = true
                        var entry = HistoryEntry(id: stamp + ":0", stamp: stamp, audit: candidate.url, timestamp: stamp)
                        entry.issues = [localized("history.issue.scan_limit")]
                        audit = HistoryAudit(entries: [entry], hasEnd: false)
                    } else {
                        audit = parse(data: data, stamp: stamp, audit: candidate.url)
                    }
                } catch {
                    var entry = HistoryEntry(id: stamp + ":0", stamp: stamp, audit: candidate.url, timestamp: stamp)
                    entry.issues = [localized("history.issue.read")]
                    audit = HistoryAudit(entries: [entry], hasEnd: false)
                }
                if let previous = selected {
                    for (left, right) in zip(previous.1.entries, audit.entries) {
                        if let a = left.context, let b = right.context, a != b { conflictingCopies = true }
                        if let a = left.summary, let b = right.summary, a != b { conflictingCopies = true }
                        if let a = left.verification, let b = right.verification, a != b { conflictingCopies = true }
                        if let a = left.finished, let b = right.finished, a != b { conflictingCopies = true }
                    }
                    if audit.entries.count > previous.1.entries.count { selected = (candidate, audit) }
                    else if audit.entries.count == previous.1.entries.count, audit.hasEnd && !previous.1.hasEnd {
                        selected = (candidate, audit)
                    } else if audit.entries.count == previous.1.entries.count, audit.hasEnd == previous.1.hasEnd {
                        if audit.evidenceWeight > previous.1.evidenceWeight
                            || (audit.evidenceWeight == previous.1.evidenceWeight && candidate.archived && !previous.0.archived) {
                            selected = (candidate, audit)
                        }
                    }
                } else { selected = (candidate, audit) }
            }
            if let selected {
                let folder = selected.0.url.deletingLastPathComponent()
                let cacheKey = folder.path + ":" + stamp
                if folderLogs[cacheKey] == nil {
                    var logs: [URL] = []
                    if let enumerator = manager.enumerator(at: folder, includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey],
                                                           options: [.skipsHiddenFiles, .skipsSubdirectoryDescendants]) {
                        var count = 0
                        for case let url as URL in enumerator {
                            count += 1
                            if count > maximumDirectoryItems { limited = true; break }
                            if url.lastPathComponent.hasPrefix("MFB_"), url.lastPathComponent.hasSuffix("_\(stamp).log"), regularFile(url) {
                                logs.append(url)
                            }
                        }
                    }
                    logs.sort { $0.lastPathComponent < $1.lastPathComponent }
                    let verbose = folder.appendingPathComponent("verbose_\(stamp).log")
                    if regularFile(verbose) { logs.append(verbose) }
                    folderLogs[cacheKey] = logs
                }
                entries += selected.1.entries.reversed().map { original in
                    var entry = original
                    if conflictingCopies {
                        entry.context = nil
                        entry.summary = nil
                        entry.verification = nil
                        entry.finished = nil
                        entry.issues.append(localized("history.issue.conflicting_copies"))
                    }
                    entry.logs = folderLogs[cacheKey] ?? []
                    return entry
                }
            }
        }
        return Result(entries: entries, issues: issues, limited: limited)
    }

    private enum HistoryParseError: Error { case invalid }
}

final class ProcessingHistoryPanel: NSWindowController, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    private let table = NSTableView()
    private let search = NSSearchField()
    private let filter = NSSegmentedControl(labels: ["", ""], trackingMode: .selectOne, target: nil, action: nil)
    private let detail = NSTextView()
    private let status = NSTextField(labelWithString: "")
    private let progress = NSProgressIndicator()
    private let splitController = NSSplitViewController()
    private var split: NSSplitView { splitController.splitView }
    private var refreshButton = NSButton()
    private var logButton = NSButton()
    private var revealButton = NSButton()
    private var copyButton = NSButton()
    private var entries: [HistoryEntry] = []
    private var displayed: [HistoryEntry] = []
    private var directory: URL
    private var generation = UUID()
    private var loadIssues: [String] = []
    private var limited = false
    private var initialDividerConfigured = false

    init(directory: URL) {
        self.directory = directory
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1040, height: 620),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        super.init(window: window)
        window.minSize = NSSize(width: 780, height: 440)
        window.isReleasedWhenClosed = false
        window.setFrameAutosaveName("MFBProcessingHistory")
        buildInterface()
    }

    required init?(coder: NSCoder) { nil }

    func show() {
        updateLocalization()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        configureInitialDivider()
    }

    private func configureInitialDivider() {
        window?.contentView?.layoutSubtreeIfNeeded()
        guard !initialDividerConfigured, split.bounds.width > 0 else { return }
        split.setPosition(split.bounds.width * 0.57, ofDividerAt: 0)
        split.layoutSubtreeIfNeeded()
        table.sizeToFit()
        initialDividerConfigured = true
    }

    func refresh(directory: URL) {
        self.directory = directory
        let token = UUID()
        generation = token
        status.stringValue = localized("history.loading")
        progress.startAnimation(nil)
        refreshButton.isEnabled = false
        let source = directory
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Swift.Result { try HistoryStore.load(directory: source) }
            DispatchQueue.main.async {
                guard let self, self.generation == token else { return }
                self.progress.stopAnimation(nil)
                self.refreshButton.isEnabled = true
                switch result {
                case let .success(result):
                    self.entries = result.entries
                    self.loadIssues = result.issues
                    self.limited = result.limited
                    self.applyFilter()
                case let .failure(error):
                    self.entries = []
                    self.displayed = []
                    self.table.reloadData()
                    self.status.stringValue = error.localizedDescription
                    self.detail.string = error.localizedDescription
                    self.updateActions()
                }
            }
        }
    }

    private func buildInterface() {
        guard let content = window?.contentView else { return }
        let root = NSStackView()
        root.orientation = .vertical
        root.alignment = .leading
        root.spacing = HistoryLayout.spacing
        root.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: HistoryLayout.margin),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -HistoryLayout.margin),
            root.topAnchor.constraint(equalTo: content.topAnchor, constant: HistoryLayout.margin),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -HistoryLayout.margin)
        ])
        search.delegate = self
        search.sendsSearchStringImmediately = true
        search.setContentHuggingPriority(.defaultLow, for: .horizontal)
        filter.selectedSegment = 0
        filter.target = self
        filter.action = #selector(filterChanged)
        filter.setWidth(96, forSegment: 0)
        filter.setWidth(140, forSegment: 1)
        refreshButton = iconButton("arrow.clockwise", key: "history.refresh", action: #selector(refreshRequested))
        let toolbar = NSStackView(views: [search, filter, refreshButton])
        toolbar.spacing = HistoryLayout.spacing
        toolbar.orientation = .horizontal
        root.addArrangedSubview(toolbar)
        toolbar.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        search.widthAnchor.constraint(greaterThanOrEqualToConstant: 160).isActive = true

        table.delegate = self
        table.dataSource = self
        table.rowHeight = HistoryLayout.rowHeight
        table.usesAlternatingRowBackgroundColors = true
        table.style = .inset
        table.allowsMultipleSelection = false
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        for (identifier, width) in [("time", CGFloat(120)), ("source", CGFloat(130)), ("status", CGFloat(115)),
                                    ("verification", CGFloat(110)), ("succeeded", CGFloat(65))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(identifier))
            column.width = width
            column.minWidth = identifier == "succeeded" ? 55 : 80
            column.maxWidth = identifier == "succeeded" ? 110 : 360
            table.addTableColumn(column)
        }
        let listScroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 574, height: 540))
        listScroll.hasVerticalScroller = true
        listScroll.hasHorizontalScroller = true
        listScroll.autohidesScrollers = true
        listScroll.documentView = table
        listScroll.translatesAutoresizingMaskIntoConstraints = false
        detail.isEditable = false
        detail.isSelectable = true
        detail.isRichText = false
        detail.font = HistoryLayout.detailFont
        detail.textContainerInset = NSSize(width: HistoryLayout.spacing, height: HistoryLayout.spacing)
        detail.backgroundColor = .textBackgroundColor
        detail.autoresizingMask = [.width]
        detail.isHorizontallyResizable = false
        detail.isVerticallyResizable = true
        detail.textContainer?.widthTracksTextView = true
        detail.minSize = NSSize(width: 0, height: 0)
        detail.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        let detailScroll = NSScrollView(frame: NSRect(x: 575, y: 0, width: 433, height: 540))
        detailScroll.hasVerticalScroller = true
        detailScroll.autohidesScrollers = true
        detailScroll.documentView = detail
        detailScroll.translatesAutoresizingMaskIntoConstraints = false
        _ = splitController.view
        split.isVertical = true
        split.dividerStyle = .thin
        let listController = NSViewController()
        listController.view = listScroll
        let listItem = NSSplitViewItem(viewController: listController)
        listItem.minimumThickness = 430
        listItem.preferredThicknessFraction = 0.57
        listItem.holdingPriority = .defaultLow
        let detailController = NSViewController()
        detailController.view = detailScroll
        let detailItem = NSSplitViewItem(viewController: detailController)
        detailItem.minimumThickness = 280
        detailItem.preferredThicknessFraction = 0.43
        detailItem.holdingPriority = .defaultHigh
        splitController.addSplitViewItem(listItem)
        splitController.addSplitViewItem(detailItem)
        root.addArrangedSubview(split)
        split.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        split.heightAnchor.constraint(greaterThanOrEqualToConstant: 230).isActive = true

        logButton = iconButton("doc.text", key: "history.open_log", action: #selector(openLog))
        revealButton = iconButton("folder", key: "history.reveal", action: #selector(revealAudit))
        copyButton = iconButton("doc.on.doc", key: "history.copy_sources", action: #selector(copySources))
        progress.style = .spinning
        progress.controlSize = .small
        progress.isDisplayedWhenStopped = false
        status.font = HistoryLayout.smallFont
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingMiddle
        status.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let footer = NSStackView(views: [logButton, revealButton, copyButton, progress, status])
        footer.orientation = .horizontal
        footer.spacing = HistoryLayout.spacing
        root.addArrangedSubview(footer)
        footer.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        updateLocalization()
        updateActions()
        window?.center()
    }

    private func iconButton(_ symbol: String, key: String, action: Selector) -> NSButton {
        let button = NSButton(image: NSImage(systemSymbolName: symbol, accessibilityDescription: localized(key)) ?? NSImage(),
                              target: self, action: action)
        button.bezelStyle = .texturedRounded
        button.imagePosition = .imageOnly
        button.toolTip = localized(key)
        button.setAccessibilityLabel(localized(key))
        button.widthAnchor.constraint(equalToConstant: HistoryLayout.buttonSize).isActive = true
        button.heightAnchor.constraint(equalToConstant: HistoryLayout.buttonSize).isActive = true
        return button
    }

    func updateLocalization() {
        window?.title = localized("history.title")
        search.placeholderString = localized("history.search")
        search.setAccessibilityLabel(localized("history.search"))
        filter.setLabel(localized("history.all"), forSegment: 0)
        filter.setLabel(localized("history.attention"), forSegment: 1)
        filter.setAccessibilityLabel(localized("history.filter"))
        table.setAccessibilityLabel(localized("history.title"))
        detail.setAccessibilityLabel(localized("history.details"))
        for column in table.tableColumns { column.title = localized("history.column.\(column.identifier.rawValue)") }
        for (button, key) in [(refreshButton, "history.refresh"), (logButton, "history.open_log"),
                              (revealButton, "history.reveal"), (copyButton, "history.copy_sources")] {
            button.toolTip = localized(key)
            button.setAccessibilityLabel(localized(key))
        }
        if refreshButton.isEnabled { applyFilter() }
    }

    @objc private func refreshRequested() { refresh(directory: directory) }
    @objc private func filterChanged() { applyFilter() }
    func controlTextDidChange(_ notification: Notification) { applyFilter() }

    private var selected: HistoryEntry? {
        displayed.indices.contains(table.selectedRow) ? displayed[table.selectedRow] : nil
    }

    private func applyFilter() {
        let selectedID = selected?.id
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        displayed = entries.filter {
            (filter.selectedSegment != 1 || $0.needsAttention)
                && (query.isEmpty || $0.searchable.localizedCaseInsensitiveContains(query))
        }
        table.reloadData()
        if let index = displayed.firstIndex(where: { $0.id == selectedID }) {
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        } else if !displayed.isEmpty { table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false) }
        status.stringValue = entries.isEmpty ? localized("history.empty")
            : localized("history.shown", displayed.count, entries.count)
        if limited { status.stringValue += " · " + localized("history.limited") }
        if !loadIssues.isEmpty { status.stringValue += " · " + loadIssues.joined(separator: " · ") }
        status.toolTip = directory.path
        showSelected()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { displayed.count }
    func tableViewSelectionDidChange(_ notification: Notification) { showSelected() }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard displayed.indices.contains(row), let column = tableColumn else { return nil }
        let entry = displayed[row]
        let label = NSTextField(labelWithString: "")
        label.font = HistoryLayout.smallFont
        label.lineBreakMode = .byTruncatingMiddle
        switch column.identifier.rawValue {
        case "time": label.stringValue = Self.timestampLabel(entry.timestamp)
        case "source":
            if let paths = entry.context?.inputs, !paths.isEmpty {
                let first = URL(fileURLWithPath: paths[0]).lastPathComponent
                label.stringValue = paths.count == 1 ? first : localized("history.source_multiple", first, paths.count)
                label.toolTip = paths.joined(separator: "\n")
            } else { label.stringValue = localized("result.unknown") }
        case "status": label.stringValue = localized(entry.statusKey); label.textColor = entry.statusColor
        case "verification":
            label.stringValue = entry.verification?.count_status ?? localized("result.unknown")
            label.textColor = entry.verification?.count_status?.hasPrefix("MISMATCH") == true ? .systemRed : .secondaryLabelColor
        case "succeeded": label.stringValue = entry.succeededLabel; label.alignment = .right
        default: return nil
        }
        if label.toolTip == nil { label.toolTip = label.stringValue }
        let cell = NSTableCellView()
        cell.textField = label
        cell.addSubview(label)
        label.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
            label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor)
        ])
        return cell
    }

    private static func timestampLabel(_ value: String) -> String {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        var date = parser.date(from: value)
        if date == nil { parser.formatOptions = [.withInternetDateTime]; date = parser.date(from: value) }
        guard let date else { return value }
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }

    private func showSelected() {
        detail.string = selected.map(Self.render) ?? localized(displayed.isEmpty && !entries.isEmpty ? "history.no_match" : "history.select")
        detail.scrollToBeginningOfDocument(nil)
        updateActions()
    }

    private static func render(_ entry: HistoryEntry) -> String {
        let unknown = localized("result.unknown")
        func count(_ value: Int?) -> String { value.map(String.init) ?? unknown }
        func row(_ key: String, _ value: String) -> String { localized("history.detail.\(key)") + ": " + value }
        var lines = [localized(entry.statusKey), Self.timestampLabel(entry.timestamp), "",
                     row("mode", entry.context?.mode ?? unknown),
                     row("sources", entry.context?.inputs.isEmpty == false ? entry.context!.inputs.joined(separator: "\n") : unknown),
                     row("output", entry.verification?.output_path ?? entry.context?.output ?? unknown),
                     row("backup", entry.context?.backup ?? unknown)]
        if let context = entry.context {
            lines += [row("dry_run", localized(context.dry_run ? "settings.value.true" : "settings.value.false")),
                      row("in_place", localized(context.in_place ? "settings.value.true" : "settings.value.false")),
                      row("shortest_path", localized(context.shortest_path ? "settings.value.true" : "settings.value.false"))]
        }
        lines += ["", localized("history.processor_counts")]
        for (key, media) in [("img", entry.summary?.img), ("vid", entry.summary?.vid)] {
            lines.append(localized("history.media.\(key)"))
            if let media, !media.active { lines.append(localized("history.not_run")); continue }
            lines += ["  " + localized("result.counts", count(media?.succeeded), count(media?.skipped),
                                      count(media?.failed), count(media?.ignored)),
                      "  " + localized("result.unprocessed", count(media?.unprocessed)),
                      "  " + row("exit", count(media?.exit_code))]
        }
        lines += ["", localized("history.verification"), row("integrity", entry.summary?.integrity.state ?? unknown),
                  row("issues", count(entry.summary?.integrity.issue_count))]
        if let verification = entry.verification {
            lines += [row("verification_state", verification.has_warnings ? "WARNINGS" : "CLEAN"),
                      row("count_status", verification.count_status ?? unknown),
                      row("verified_source", verification.source_path ?? unknown),
                      row("issues", String(verification.issue_count)),
                      row("source_inventory", String(verification.source_count)),
                      row("optimized_inventory", String(verification.optimized_count)),
                      row("skipped_inventory", String(verification.skipped_count)),
                      row("failed_inventory", String(verification.failed_count)),
                      row("remaining_inventory", String(verification.source_remaining_count)),
                      row("deleted_inventory", String(verification.verified_deleted_count))]
        } else { lines += [row("verification_state", unknown), row("count_status", unknown)] }
        if let error = entry.finished?.error { lines += ["", row("error", error)] }
        if !entry.issues.isEmpty { lines += ["", localized("history.read_issues")] + entry.issues }
        if entry.legacy { lines += ["", localized("history.legacy_note")] }
        if let summary = entry.summary {
            if !summary.failed_files.isEmpty { lines += ["", localized("history.failed_files")] + summary.failed_files }
            if !summary.skipped_files.isEmpty { lines += ["", localized("history.skipped_files")] + summary.skipped_files }
        }
        lines += ["", row("audit", entry.audit.path)]
        return lines.joined(separator: "\n")
    }

    private func updateActions() {
        logButton.isEnabled = selected?.logs.isEmpty == false
        revealButton.isEnabled = selected != nil
        copyButton.isEnabled = selected?.context?.inputs.isEmpty == false
    }
    @objc private func openLog() {
        guard let url = selected?.logs.first, HistoryStore.regularFile(url), NSWorkspace.shared.open(url) else {
            status.stringValue = localized("history.issue.open_log"); return
        }
    }
    @objc private func revealAudit() {
        guard let url = selected?.audit, HistoryStore.regularFile(url) else {
            status.stringValue = localized("history.issue.read"); return
        }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }
    @objc private func copySources() {
        guard let paths = selected?.context?.inputs, !paths.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(paths.joined(separator: "\n"), forType: .string)
        status.stringValue = localized("history.copied")
    }

    func validateForSelfTest() throws {
        guard let content = window?.contentView else { throw HostError(message: "Missing history content view") }
        content.layoutSubtreeIfNeeded()
        configureInitialDivider()
        guard split.arrangedSubviews.count == 2 else {
            throw HostError(message: "History split controller did not load both columns")
        }
        guard split.arrangedSubviews[1].frame.width >= 350 else {
            throw HostError(message: "History initial widths: total=\(split.bounds.width) list=\(split.arrangedSubviews[0].frame.width) detail=\(split.arrangedSubviews[1].frame.width)")
        }
        guard let listScroll = table.enclosingScrollView,
              table.rect(ofColumn: 4).maxX <= listScroll.contentSize.width + 1 else {
            throw HostError(message: "History initial columns did not fit the list viewport")
        }
        generation = UUID()
        entries = []
        applyFilter()
        guard table.tableColumns.count == 5, search.delegate != nil, detail.isEditable == false,
              split.arrangedSubviews.count == 2, copyButton.isEnabled == false,
              logButton.isEnabled == false, revealButton.isEnabled == false, table.numberOfRows == 0,
              window?.styleMask.contains(.resizable) == true else {
            throw HostError(message: "History native controls or selection state regressed")
        }
        let synthetic = HistoryEntry(id: "synthetic:0", stamp: "synthetic", audit: URL(fileURLWithPath: "/synthetic/audit"),
                                     timestamp: "synthetic", legacy: true)
        entries = [synthetic]
        applyFilter()
        guard table.numberOfRows == 1, revealButton.isEnabled, !copyButton.isEnabled, !logButton.isEnabled else {
            throw HostError(message: "History selection actions invented missing paths")
        }
        search.stringValue = "absent synthetic search"
        applyFilter()
        guard table.numberOfRows == 0, !revealButton.isEnabled else {
            throw HostError(message: "History search retained a stale selected action")
        }
        search.stringValue = ""
        filter.selectedSegment = 1
        applyFilter()
        guard table.numberOfRows == 1 else { throw HostError(message: "Legacy history disappeared from attention filter") }
        let original = window!.frame
        window?.setContentSize(NSSize(width: 780, height: 440))
        content.layoutSubtreeIfNeeded()
        guard search.frame.width >= 150, split.frame.height >= 230,
              split.frame.maxX <= content.bounds.maxX else {
            throw HostError(message: "History controls clipped at minimum size")
        }
        window?.setFrame(original, display: false)
    }

    func validateDatabaseLoadForSelfTest(expectedStatuses: [String]) throws {
        refresh(directory: directory)
        let deadline = Date().addingTimeInterval(5)
        while !refreshButton.isEnabled, Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        guard refreshButton.isEnabled, entries.map(\.statusKey).sorted() == expectedStatuses.sorted(),
              table.numberOfRows == expectedStatuses.count, selected?.audit.lastPathComponent == "history.sqlite3",
              revealButton.isEnabled, detail.string.contains("history.sqlite3") else {
            throw HostError(message: "History panel did not load database records: \(status.stringValue)")
        }
        filter.selectedSegment = 1
        applyFilter()
        guard displayed.allSatisfy(\.needsAttention), displayed.count == entries.filter(\.needsAttention).count else {
            throw HostError(message: "Database history attention filter lost counted states")
        }
    }
}

func runProcessingHistorySelfTests() throws {
    let fixtureURL = URL(fileURLWithPath: "/synthetic/session_audit_test.jsonl")
    func records(_ events: [String]) throws -> Data {
        var data = Data()
        for event in events {
            data.append(try JSONSerialization.data(withJSONObject: ["ts": "2026-10-04T01:02:03Z", "event": event]))
            data.append(0x0a)
        }
        return data
    }
    func payload(_ event: String, _ value: [String: Any]) throws -> String {
        event + "=" + String(decoding: try JSONSerialization.data(withJSONObject: value), as: UTF8.self)
    }
    let context = try payload("MFB_HISTORY_CONTEXT", ["schema_version": 1, "inputs": ["/synthetic/input"],
        "output": NSNull(), "backup": NSNull(), "mode": "standard", "dry_run": false, "in_place": false, "shortest_path": false])
    let media: [String: Any] = ["active": true, "succeeded": 2, "skipped": 1, "ignored": 0, "failed": 0,
                               "unprocessed": 0, "exit_code": 0]
    let inactive: [String: Any] = ["active": false, "succeeded": NSNull(), "skipped": NSNull(),
        "ignored": NSNull(), "failed": NSNull(), "unprocessed": NSNull(), "exit_code": NSNull()]
    let summaryValue: [String: Any] = ["schema_version": 1, "count_scope": "processor_outcomes", "img": media, "vid": inactive,
        "integrity": ["state": NSNull(), "issue_count": NSNull()], "failed_files": [], "skipped_files": []]
    let summary = try payload("MFB_HISTORY_SUMMARY", summaryValue)
    let completed = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "completed", "error": NSNull()])
    let failed = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "failed", "error": "synthetic final failure"])
    func parsed(_ events: [String]) throws -> HistoryAudit { HistoryStore.parse(data: try records(events), stamp: "test", audit: fixtureURL) }
    func require(_ condition: Bool, _ message: String) throws { if !condition { throw HostError(message: message) } }
    let success = try parsed(["SESSION_STARTED", context, summary, completed])
    try require(success.entries.count == 1 && success.entries[0].statusKey == "history.status.completed"
                && success.entries[0].context?.output == nil && success.entries[0].succeededLabel == "2", "Typed history did not retain nullable paths or active counts")
    let failure = try parsed([context, summary, failed])
    try require(failure.entries[0].statusKey == "history.status.failed" && failure.entries[0].succeededLabel == "2",
                "Converted items hid a final session failure")
    let batches = try parsed([context, summary, completed, context, failed])
    try require(batches.entries.count == 2 && batches.entries[1].summary == nil && batches.entries[1].statusKey == "history.status.failed",
                "History merged independent batches or leaked counts")
    let partialBatches = try parsed([context, summary, completed, context])
    try require(!partialBatches.hasEnd, "An earlier run concealed the unfinished latest run")
    let duplicate = try parsed([context, summary, completed, failed])
    try require(duplicate.entries[0].finished == nil && duplicate.entries[0].needsAttention,
                "Conflicting terminal outcomes were silently overwritten")
    let repeated = try parsed([context, summary, summary, completed, completed])
    try require(repeated.entries[0].statusKey == "history.status.completed", "Repeated identical evidence was treated as conflicting")
    let previewValue: [String: Any] = ["schema_version": 1, "inputs": ["/synthetic/input"],
        "mode": "standard", "dry_run": true, "in_place": false, "shortest_path": false]
    let preview = try parsed([try payload("MFB_HISTORY_CONTEXT", previewValue), completed])
    try require(preview.entries[0].statusKey == "history.status.preview" && !preview.entries[0].needsAttention
                && preview.entries[0].succeededLabel == localized("result.unknown"), "Preview invented media counts or failure")
    var nullableMedia = media
    nullableMedia["unprocessed"] = NSNull()
    var nullableSummary = summaryValue
    nullableSummary["img"] = nullableMedia
    let nullable = try parsed([context, try payload("MFB_HISTORY_SUMMARY", nullableSummary), completed])
    try require(nullable.entries[0].statusKey == "history.status.incomplete" && nullable.entries[0].summary?.img.unprocessed == nil,
                "Missing history count became zero or completed")
    for invalidValue: Any in [-1, "4", true, NSNumber(value: UInt64.max)] {
        var invalidMedia = media
        invalidMedia["failed"] = invalidValue
        var invalidSummary = summaryValue
        invalidSummary["img"] = invalidMedia
        let invalid = try parsed([context, try payload("MFB_HISTORY_SUMMARY", invalidSummary), completed])
        try require(invalid.entries[0].summary == nil && invalid.entries[0].statusKey == "history.status.incomplete",
                    "Invalid or overflowing history count was accepted")
    }
    var futureSummary = summaryValue
    futureSummary["schema_version"] = 2
    let future = try parsed([context, try payload("MFB_HISTORY_SUMMARY", futureSummary), completed])
    try require(future.entries[0].summary == nil && !future.entries[0].issues.isEmpty, "Future history schema was silently accepted")
    let invalidReplacement = try parsed([context, summary, try payload("MFB_HISTORY_SUMMARY", futureSummary), summary, completed])
    try require(invalidReplacement.entries[0].summary == nil && invalidReplacement.entries[0].needsAttention,
                "Invalid replacement evidence retained stale counts")
    let verificationValue: [String: Any] = ["schema_version": 1, "has_warnings": false,
        "issue_count": 0, "source_count": 3, "optimized_count": 2, "skipped_count": 1,
        "failed_count": 0, "source_remaining_count": 1, "verified_deleted_count": 2,
        "count_status": "MATCH (1 expected handoff gap)", "source_path": "/synthetic/input", "output_path": "/synthetic/output"]
    let verification = try payload("MFB_HISTORY_VERIFICATION", verificationValue)
    let verified = try parsed([context, verification, summary, completed])
    try require(verified.entries[0].verification?.count_status == "MATCH (1 expected handoff gap)",
                "History reinterpreted the verifier's count status")
    var mismatchValue = verificationValue
    mismatchValue["count_status"] = "MISMATCH (1 invariant issue)"
    let mismatched = try parsed([context, try payload("MFB_HISTORY_VERIFICATION", mismatchValue), summary, completed])
    try require(mismatched.entries[0].needsAttention, "MATCH status ignored a verifier mismatch")
    let conflictVerification = try parsed([context, verification, try payload("MFB_HISTORY_VERIFICATION", mismatchValue), summary, completed])
    try require(conflictVerification.entries[0].verification == nil, "Conflicting verification replaced prior evidence")
    var overflowMedia = media
    overflowMedia["succeeded"] = Int.max
    var overflowSummary = summaryValue
    overflowSummary["img"] = overflowMedia
    let overflow = try parsed([context, try payload("MFB_HISTORY_SUMMARY", overflowSummary), completed])
    try require(overflow.entries[0].summary == nil, "Overflowing history inventory was accepted")
    let legacy = try parsed(["SESSION_STARTED", "SESSION_COMPLETED images_ok=2 images_skip=1 images_fail=0 videos_ok=0 integrity=Some(\"CLEAN\") integrity_issues=0"])
    try require(legacy.entries[0].legacy && legacy.entries[0].statusKey == "history.status.legacy"
                && legacy.entries[0].context == nil && legacy.entries[0].summary?.img.unprocessed == nil,
                "Legacy history invented paths, pending counts or completion")
    var truncated = try records([context, summary, completed])
    truncated.append(Data("{\"ts\":".utf8))
    let damaged = HistoryStore.parse(data: truncated, stamp: "test", audit: fixtureURL)
    try require(!damaged.hasEnd && damaged.entries[0].needsAttention, "Truncated history appeared complete")
    let invalidUTF8 = HistoryStore.parse(data: Data([0xff]), stamp: "test", audit: fixtureURL)
    try require(!invalidUTF8.entries[0].issues.isEmpty, "History silently replaced invalid UTF-8")

    let directory = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
        .appendingPathComponent("MFBHistorySelfTest-" + UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let rootAudit = directory.appendingPathComponent("session_audit_test.jsonl")
    try records([context]).write(to: rootAudit)
    let archive = directory.appendingPathComponent("Bundle_test")
    try FileManager.default.createDirectory(at: archive, withIntermediateDirectories: false)
    try records([context, summary, completed]).write(to: archive.appendingPathComponent("session_audit_test.jsonl"))
    try FileManager.default.createSymbolicLink(at: directory.appendingPathComponent("session_audit_link.jsonl"), withDestinationURL: rootAudit)
    let scanned = try HistoryStore.load(directory: directory)
    try require(scanned.entries.count == 1, "History duplicate or symlink filtering returned \(scanned.entries.count) records")
    try require(scanned.entries[0].audit.deletingLastPathComponent().resolvingSymlinksInPath().path == archive.resolvingSymlinksInPath().path,
                "History archive selection mismatch: \(scanned.entries[0].audit.path) expected \(archive.path)")
    try require(scanned.entries[0].statusKey == "history.status.completed",
                "History archive status was \(scanned.entries[0].statusKey): \(scanned.entries[0].issues)")
    let projectLog = archive.appendingPathComponent("MFB_synthetic_test.log")
    try Data("synthetic log".utf8).write(to: projectLog)
    try Data("synthetic verbose".utf8).write(to: archive.appendingPathComponent("verbose_test.log"))
    let withLogs = try HistoryStore.load(directory: directory)
    try require(withLogs.entries[0].logs.first?.resolvingSymlinksInPath().path == projectLog.resolvingSymlinksInPath().path,
                "Verbose log concealed the main session log")
    try records([context, completed]).write(to: archive.appendingPathComponent("session_audit_test.jsonl"))
    try records([context, summary, completed]).write(to: rootAudit)
    let richerActive = try HistoryStore.load(directory: directory)
    try require(richerActive.entries[0].summary != nil, "A sparse archived copy concealed richer result evidence")
    try records([context, summary, completed]).write(to: archive.appendingPathComponent("session_audit_test.jsonl"))
    try records([context, summary, completed, context]).write(to: rootAudit)
    let newerActive = try HistoryStore.load(directory: directory)
    try require(newerActive.entries.count == 2 && newerActive.entries[0].statusKey == "history.status.incomplete",
                "A completed archive concealed a newer unfinished active batch")
    try records([context, summary, completed, context]).write(to: archive.appendingPathComponent("session_audit_test.jsonl"))
    try records([context, summary, completed]).write(to: rootAudit)
    let newerArchive = try HistoryStore.load(directory: directory)
    try require(newerArchive.entries.count == 2 && newerArchive.entries[0].statusKey == "history.status.incomplete",
                "A completed active copy concealed a newer unfinished archived batch")
    try records([context, summary, completed]).write(to: archive.appendingPathComponent("session_audit_test.jsonl"))
    try records([context, summary, failed]).write(to: rootAudit)
    let conflict = try HistoryStore.load(directory: directory)
    try require(conflict.entries[0].summary == nil && conflict.entries[0].finished == nil
                && conflict.entries[0].needsAttention, "Conflicting archive copies concealed a terminal failure")

    let databaseURL = directory.appendingPathComponent("history.sqlite3")
    func writeDatabase(_ sessions: [(String, [String])], schema: Int = 1) throws {
        var database: OpaquePointer?
        guard sqlite3_open_v2(databaseURL.path, &database, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, nil) == SQLITE_OK,
              let database else { throw HostError(message: "Unable to create synthetic history database") }
        defer { sqlite3_close(database) }
        let setup = """
        DROP TABLE IF EXISTS history_events;
        DROP TABLE IF EXISTS history_sessions;
        CREATE TABLE history_sessions(session_id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
        CREATE TABLE history_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES history_sessions(session_id), ts TEXT NOT NULL, event TEXT NOT NULL);
        CREATE INDEX history_events_session_sequence ON history_events(session_id, sequence);
        CREATE INDEX history_sessions_updated_at ON history_sessions(updated_at);
        PRAGMA user_version = \(schema);
        BEGIN;
        """
        guard sqlite3_exec(database, setup, nil, nil, nil) == SQLITE_OK else {
            throw HostError(message: "Synthetic history schema failed")
        }
        var insertSession: OpaquePointer?
        var insertEvent: OpaquePointer?
        defer { sqlite3_finalize(insertSession); sqlite3_finalize(insertEvent) }
        guard sqlite3_prepare_v2(database, "INSERT INTO history_sessions VALUES (?, ?)", -1, &insertSession, nil) == SQLITE_OK,
              sqlite3_prepare_v2(database, "INSERT INTO history_events(session_id, ts, event) VALUES (?, ?, ?)", -1, &insertEvent, nil) == SQLITE_OK
        else { throw HostError(message: "Synthetic history insert prepare failed") }
        func insert(_ query: OpaquePointer?, _ values: [String]) throws {
            sqlite3_reset(query)
            for (index, value) in values.enumerated() {
                guard value.withCString({ sqlite3_bind_text(query, Int32(index + 1), $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK
                else { throw HostError(message: "Synthetic history binding failed") }
            }
            guard sqlite3_step(query) == SQLITE_DONE else { throw HostError(message: "Synthetic history insert failed") }
        }
        for (stamp, events) in sessions {
            let timestamp = "2026-10-07T01:02:03.123456Z"
            try insert(insertSession, [stamp, timestamp])
            for event in events { try insert(insertEvent, [stamp, timestamp, event]) }
        }
        guard sqlite3_exec(database, "COMMIT", nil, nil, nil) == SQLITE_OK else {
            throw HostError(message: "Synthetic history commit failed")
        }
    }
    let cancelled = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "cancelled", "error": NSNull()])
    var pendingMedia = media
    pendingMedia["unprocessed"] = 1
    var pendingSummary = summaryValue
    pendingSummary["img"] = pendingMedia
    var fileFailureMedia = media
    fileFailureMedia["failed"] = 1
    var fileFailureSummary = summaryValue
    fileFailureSummary["img"] = fileFailureMedia
    try writeDatabase([
        ("test", [context, verification, summary, completed]),
        ("failed", [context, summary, failed]),
        ("cancelled", [context, summary, cancelled]),
        ("preview", [try payload("MFB_HISTORY_CONTEXT", previewValue), completed]),
        ("pending", [context, try payload("MFB_HISTORY_SUMMARY", pendingSummary), completed]),
        ("file_failures", [context, try payload("MFB_HISTORY_SUMMARY", fileFailureSummary), completed]),
        ("warnings", [context, try payload("MFB_HISTORY_VERIFICATION", mismatchValue), summary, completed]),
        ("invalid", [context, try payload("MFB_HISTORY_SUMMARY", futureSummary), completed]),
        ("empty", [])
    ])
    let databaseBytes = try Data(contentsOf: databaseURL)
    try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: databaseURL.path)
    let databaseLoaded = try HistoryStore.load(directory: directory)
    let expectedStatuses = ["history.status.completed", "history.status.failed", "history.status.cancelled",
        "history.status.preview", "history.status.unfinished", "history.status.file_failures",
        "history.status.verification_warnings", "history.status.incomplete", "history.status.incomplete"]
    try require(databaseLoaded.entries.map(\.statusKey).sorted() == expectedStatuses.sorted(),
                "Database history changed counted terminal states")
    try require(databaseLoaded.entries.filter { $0.stamp == "test" }.count == 1
                && databaseLoaded.entries.first(where: { $0.stamp == "test" })?.succeededLabel == "2"
                && databaseLoaded.entries.first(where: { $0.stamp == "test" })?.logs.first?.resolvingSymlinksInPath().path == projectLog.resolvingSymlinksInPath().path,
                "Legacy copies overrode database evidence or lost associated logs")
    try MainActor.assumeIsolated {
        _ = NSApplication.shared
        try ProcessingHistoryPanel(directory: directory).validateDatabaseLoadForSelfTest(expectedStatuses: expectedStatuses)
    }
    try require(try Data(contentsOf: databaseURL) == databaseBytes
                && !FileManager.default.fileExists(atPath: databaseURL.path + "-wal")
                && !FileManager.default.fileExists(atPath: databaseURL.path + "-shm")
                && !FileManager.default.fileExists(atPath: databaseURL.path + "-journal"),
                "History read mutated the database or created sidecars")
    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: databaseURL.path)
    try records(["SESSION_STARTED", "SESSION_COMPLETED images_ok=1"]).write(to: directory.appendingPathComponent("session_audit_legacy.jsonl"))
    let mixed = try HistoryStore.load(directory: directory)
    try require(mixed.entries.count == expectedStatuses.count + 1 && mixed.entries.contains { $0.stamp == "legacy" && $0.legacy },
                "Database migration concealed unrelated legacy sessions")
    let many = (0...HistoryStore.maximumSessions).map { (String(format: "session_%03d", $0), [context, summary, completed]) }
    try writeDatabase(many)
    try records([context, summary, failed]).write(to: directory.appendingPathComponent("session_audit_session_000.jsonl"))
    let bounded = try HistoryStore.load(directory: directory)
    try require(bounded.limited && bounded.entries.count == HistoryStore.maximumSessions
                && !bounded.entries.contains { $0.stamp == "session_000" },
                "Database session bound leaked an older database-owned JSONL copy")
    try writeDatabase([("record_limit", Array(repeating: "MFB_HISTORY_FUTURE=", count: HistoryStore.maximumRecordsPerSession + 1))])
    let recordLimit = try HistoryStore.load(directory: directory)
    try require(recordLimit.limited && recordLimit.entries.first(where: { $0.stamp == "record_limit" })?.needsAttention == true,
                "Database record bound appeared complete")
    try writeDatabase([("entry_limit", Array(repeating: context, count: HistoryStore.maximumEntriesPerSession + 1))])
    let entryLimit = try HistoryStore.load(directory: directory)
    try require(entryLimit.entries.filter { $0.stamp == "entry_limit" }.count == HistoryStore.maximumEntriesPerSession
                && entryLimit.entries.first(where: { $0.stamp == "entry_limit" })?.needsAttention == true,
                "Database entries bypassed the shared parser bound")
    let largeEvent = "MFB_HISTORY_FUTURE=" + String(repeating: "x", count: HistoryStore.maximumAuditBytes / 2)
    try writeDatabase([("byte_limit", [largeEvent, largeEvent])])
    let byteLimit = try HistoryStore.load(directory: directory)
    try require(byteLimit.limited && byteLimit.entries.first(where: { $0.stamp == "byte_limit" })?.needsAttention == true,
                "Database byte bound appeared complete")
    let oversizedEvent = "MFB_HISTORY_FUTURE=" + String(repeating: "x", count: HistoryStore.maximumAuditBytes + 1)
    try writeDatabase([("oversized", [oversizedEvent]), ("test", [context, summary, completed])])
    let oversized = try HistoryStore.load(directory: directory)
    try require(oversized.limited && oversized.entries.first(where: { $0.stamp == "oversized" })?.needsAttention == true
                && oversized.entries.first(where: { $0.stamp == "test" })?.statusKey == "history.status.completed",
                "One oversized database session concealed other valid sessions")
    func requireReadFailure(_ message: String) throws {
        do { _ = try HistoryStore.load(directory: directory) }
        catch { return }
        throw HostError(message: message)
    }
    try writeDatabase([("test", [context, summary, completed])], schema: 2)
    let futureDatabase = try Data(contentsOf: databaseURL)
    try requireReadFailure("Unsupported database schema appeared successful")
    try require(try Data(contentsOf: databaseURL) == futureDatabase, "History reader migrated an unsupported schema")
    try Data("synthetic corrupt database".utf8).write(to: databaseURL)
    try requireReadFailure("Corrupt database fell back to successful legacy evidence")
    try FileManager.default.removeItem(at: databaseURL)
    _ = try HistoryStore.load(directory: directory)
    try require(!FileManager.default.fileExists(atPath: databaseURL.path), "History reader created an absent database")
}
