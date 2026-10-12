import AppKit
import Foundation
import SQLite3
import Darwin
import CoreFoundation
import UniformTypeIdentifiers

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
    let version: String?
    let retry: Bool?
    let resume: Bool?
    let no_resume: Bool?
    let output_sequence: Int64?
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
    struct Size: Decodable, Equatable {
        let scope: String
        let input_bytes: UInt64?
        let output_bytes: UInt64?

        var valid: Bool {
            (scope == "converted_this_run"
                || (scope == "fast_img_converted_this_run" && (input_bytes != nil || output_bytes != nil)))
                && [input_bytes, output_bytes].compactMap { $0 }.allSatisfy { $0 <= UInt64(Int64.max) }
        }

        var scopeKey: String {
            scope == "converted_this_run" ? "history.space.converted_scope" : "history.space.scope"
        }
    }
    let schema_version: Int
    let count_scope: String
    let img: HistoryMedia
    let vid: HistoryMedia
    let integrity: Integrity
    let failed_files: [String]
    let skipped_files: [String]
    var size: Size?
    var sizeInvalid: Bool

    private enum CodingKeys: String, CodingKey {
        case schema_version, count_scope, img, vid, integrity, failed_files, skipped_files, size, size_invalid
    }

    init(schema_version: Int, count_scope: String, img: HistoryMedia, vid: HistoryMedia,
         integrity: Integrity, failed_files: [String], skipped_files: [String]) {
        self.schema_version = schema_version
        self.count_scope = count_scope
        self.img = img
        self.vid = vid
        self.integrity = integrity
        self.failed_files = failed_files
        self.skipped_files = skipped_files
        size = nil
        sizeInvalid = false
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schema_version = try values.decode(Int.self, forKey: .schema_version)
        count_scope = try values.decode(String.self, forKey: .count_scope)
        img = try values.decode(HistoryMedia.self, forKey: .img)
        vid = try values.decode(HistoryMedia.self, forKey: .vid)
        integrity = try values.decode(Integrity.self, forKey: .integrity)
        failed_files = try values.decode([String].self, forKey: .failed_files)
        skipped_files = try values.decode([String].self, forKey: .skipped_files)
        size = nil
        sizeInvalid = values.contains(.size_invalid) ? ((try? values.decode(Bool.self, forKey: .size_invalid)) ?? true) : false
        guard !sizeInvalid else { return }
        if values.contains(.size), try !values.decodeNil(forKey: .size) {
            let candidate = try? values.decode(Size.self, forKey: .size)
            if let candidate, candidate.valid,
               (candidate.scope == "converted_this_run" ? (img.active || vid.active) : img.active) {
                size = candidate
            } else {
                sizeInvalid = true
            }
        }
    }

    var valid: Bool {
        guard schema_version == 1, count_scope == "processor_outcomes", img.valid, vid.valid,
              integrity.issue_count.map({ $0 >= 0 }) ?? true else { return false }
        for pair in [(img.succeeded, vid.succeeded), (img.skipped, vid.skipped),
                     (img.ignored, vid.ignored), (img.failed, vid.failed), (img.unprocessed, vid.unprocessed)] {
            if let left = pair.0, let right = pair.1, left.addingReportingOverflow(right).overflow { return false }
        }
        return true
    }

    func sameOutcomes(as other: HistorySummary) -> Bool {
        schema_version == other.schema_version && count_scope == other.count_scope
            && img == other.img && vid == other.vid && integrity == other.integrity
            && failed_files == other.failed_files && skipped_files == other.skipped_files
    }
}

private struct HistoryFinished: Decodable, Equatable {
    let schema_version: Int
    let outcome: String
    let error: String?
    let output_sequence: Int64?
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

private struct HistoryInputPlan: Decodable, Equatable {
    struct Input: Decodable, Equatable {
        let source: String
        let output: String?
        let in_place: Bool
        let requires_verification: Bool
    }
    let schema_version: Int
    let inputs: [Input]

    func valid(for context: HistoryContext) -> Bool {
        func absolute(_ path: String) -> Bool { path.hasPrefix("/") && !path.contains("\0") }
        return schema_version == 1 && context.inputs.count > 1
            && inputs.map(\.source) == context.inputs
            && Set(context.inputs).count == context.inputs.count
            && Set(inputs.compactMap(\.output)).count == inputs.compactMap(\.output).count
            && inputs.allSatisfy {
                absolute($0.source) && ($0.output.map(absolute) ?? true)
                    && $0.in_place == context.in_place
                    && (!$0.in_place || $0.output == nil)
                    && ($0.in_place || $0.output != nil)
                    && (!$0.requires_verification || (!$0.in_place && $0.output != nil))
            }
    }
}

private struct HistoryConfigSnapshot: Hashable {
    let pipeline: String
    let packageVersion: String
    let values: String
    let sources: String

    func comparisonFields() throws -> (values: [String: String], sources: [String: String]) {
        guard let object = try JSONSerialization.jsonObject(with: Data(values.utf8)) as? [String: Any],
              let origins = try JSONSerialization.jsonObject(with: Data(sources.utf8)) as? [String: String]
        else { throw HostError(message: localized("history.config.invalid")) }
        var fields: [String: String] = [:]
        func flatten(_ object: [String: Any], prefix: String = "") throws {
            for (key, value) in object {
                let path = prefix.isEmpty ? key : prefix + "." + key
                if let nested = value as? [String: Any], !nested.isEmpty {
                    try flatten(nested, prefix: path)
                } else {
                    guard fields[path] == nil else { throw HostError(message: localized("history.config.invalid")) }
                    let data = try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys])
                    fields[path] = String(decoding: data, as: UTF8.self)
                }
            }
        }
        try flatten(object)
        let sourceFields = try origins.mapValues { value in
            String(decoding: try JSONSerialization.data(withJSONObject: value, options: .fragmentsAllowed), as: UTF8.self)
        }
        return (fields, sourceFields)
    }
}

private struct HistoryConfigMetadata: Decodable {
    let schema_version: Int
    let pipeline: String
    let package_version: String
    let sources: [String: String]
    let invocation_id: String?
}

private struct HistoryInvocation: Codable {
    struct Binary: Codable {
        let scope: String
        let bytes: UInt64?
        let blake3: String?
        let diagnostic: String?

        var valid: Bool {
            guard scope == "observed_before_launch" else { return false }
            if let bytes, let blake3 {
                return bytes <= UInt64(Int64.max) && diagnostic == nil && blake3.utf8.count == 64
                    && blake3.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
            }
            return bytes == nil && blake3 == nil && diagnostic.map { !$0.isEmpty && $0.utf8.count <= 4096 } == true
        }
    }
    let schema_version: Int
    let invocation_id: String
    let pipeline: String
    let state: String
    let program_requested: String
    let program_path: String?
    let arguments: [String]
    let binary: Binary

    var valid: Bool {
        guard schema_version == 1, !invocation_id.isEmpty, invocation_id.utf8.count <= 256,
              invocation_id.rangeOfCharacter(from: .controlCharacters) == nil,
              ["img", "vid", "cmd"].contains(pipeline), state == "launch_requested",
              !program_requested.isEmpty, program_requested.utf8.count <= 32 * 1024, !program_requested.contains("\0"),
              program_path.map({ $0.hasPrefix("/") && !$0.contains("\0") }) ?? true,
              arguments.count <= 4096, binary.valid, program_path != nil || binary.blake3 == nil else { return false }
        var bytes = 0
        for argument in arguments {
            guard !argument.contains("\0"), argument.utf8.count <= 256 * 1024 - bytes else { return false }
            bytes += argument.utf8.count
        }
        return true
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
    var inputPlan: HistoryInputPlan?
    var inputVerifications: [String: HistoryVerification] = [:]
    var configSnapshots: [HistoryConfigSnapshot] = []
    var configSet: Set<HistoryConfigSnapshot> = []
    var configInvocationCount = 0
    var configMarkerSeen = false
    var configEvidenceInvalid = false
    var invocations: [HistoryInvocation] = []
    var invocationConfigs: [String: HistoryConfigSnapshot] = [:]
    var invocationEvidenceInvalid = false
    var finished: HistoryFinished?
    var legacy = false
    var legacyEnded = false
    var issues: [String] = []
    var logs: [URL] = []
    var contradictoryFields: Set<String> = []
    var outputUpperSequence: Int64?

    var hasStoredOutput: Bool { audit.lastPathComponent == "history.sqlite3" && context?.output_sequence != nil }
    var outputEndSource: String {
        if finished?.output_sequence != nil { return "terminal_boundary" }
        return outputUpperSequence != nil ? "next_batch_boundary" : "snapshot_only"
    }
    var isRestoreJpeg: Bool { context?.mode == "restore-jpeg" }
    var modeLabel: String? { isRestoreJpeg ? localized("operation.restore_jpeg") : context?.mode }

    var outputPaths: String? {
        if let inputPlan {
            let outputs = inputPlan.inputs.compactMap(\.output)
            return outputs.isEmpty ? nil : outputs.joined(separator: "\n")
        }
        return verification?.output_path ?? context?.output
    }

    mutating func aggregateInputVerification() {
        guard let inputPlan else { return }
        verification = nil
        guard context?.dry_run == false, issues.isEmpty, !contradictoryFields.contains("verification"),
              !contradictoryFields.contains("input_plan") else { return }
        let required = inputPlan.inputs.filter(\.requires_verification)
        guard !required.isEmpty else { return }
        let receipts = required.compactMap { inputVerifications[$0.source] }
        guard receipts.count == required.count else {
            issues.append(localized("history.status.incomplete")); return
        }
        // Single-file inputs have worker gates, not directory-inventory receipts.
        guard required.count == inputPlan.inputs.count else { return }
        var totals = Array(repeating: 0, count: 7)
        var warnings = false
        var mismatch = false
        var explained = false
        for receipt in receipts {
            let status = receipt.count_status ?? ""
            let matches = status == "MATCH" || (status.hasPrefix("MATCH (") && status.hasSuffix(")"))
            let mismatches = status == "MISMATCH" || (status.hasPrefix("MISMATCH (") && status.hasSuffix(")"))
            let explains = status == "EXPLAINED" || (status.hasPrefix("EXPLAINED (") && status.hasSuffix(")"))
            guard matches || mismatches || explains else {
                issues.append(localized("history.status.incomplete")); return
            }
            warnings = warnings || receipt.has_warnings || receipt.issue_count > 0 || mismatches
            mismatch = mismatch || mismatches
            explained = explained || explains
            for (index, value) in [receipt.issue_count, receipt.source_count, receipt.optimized_count,
                                   receipt.skipped_count, receipt.failed_count, receipt.source_remaining_count,
                                   receipt.verified_deleted_count].enumerated() {
                let sum = totals[index].addingReportingOverflow(value)
                guard !sum.overflow else {
                    contradictoryFields.insert("verification")
                    issues.append(localized("history.status.incomplete")); return
                }
                totals[index] = sum.partialValue
            }
        }
        if receipts.count == 1 {
            verification = receipts[0]
        } else {
            verification = HistoryVerification(schema_version: 1, has_warnings: warnings,
                issue_count: totals[0], source_count: totals[1], optimized_count: totals[2],
                skipped_count: totals[3], failed_count: totals[4], source_remaining_count: totals[5],
                verified_deleted_count: totals[6], count_status: mismatch ? "MISMATCH" : explained ? "EXPLAINED" : "MATCH",
                source_path: nil, output_path: nil)
        }
    }

    var hasProcessorError: Bool {
        [summary?.img, summary?.vid].compactMap { $0 }.contains { $0.active && ($0.exit_code ?? 0) != 0 }
    }
    var hasFailures: Bool {
        (summary?.img.failed ?? 0) > 0 || (summary?.vid.failed ?? 0) > 0
            || hasProcessorError
    }
    var hasVerificationWarnings: Bool {
        verification?.has_warnings == true || verification?.count_status?.hasPrefix("MISMATCH") == true
            || (verification?.issue_count ?? 0) > 0
            || inputVerifications.values.contains { $0.has_warnings || $0.issue_count > 0 || $0.count_status?.hasPrefix("MISMATCH") == true }
            || (summary?.integrity.issue_count ?? 0) > 0 || summary?.integrity.state == "WARNINGS"
    }
    var hasPending: Bool { (summary?.img.unprocessed ?? 0) > 0 || (summary?.vid.unprocessed ?? 0) > 0 }
    var hasPartialSuccess: Bool {
        guard !legacy, issues.isEmpty, contradictoryFields.isEmpty, context?.dry_run == false,
              let summary, summary.valid, summary.img.complete, summary.vid.complete, !hasPending,
              let outcome = finished?.outcome, ["completed", "failed"].contains(outcome) else { return false }
        let active = [summary.img, summary.vid].filter(\.active)
        return hasPartialProcessingSuccess(succeeded: active.map(\.succeeded), failed: active.map(\.failed),
                                           countsComplete: true)
    }
    var statusKey: String {
        if finished?.outcome == "cancelled" { return "history.status.cancelled" }
        if hasPartialSuccess { return "history.status.partial" }
        if finished?.outcome == "failed" { return "history.status.failed" }
        if hasProcessorError { return "history.status.failed" }
        if hasFailures { return "history.status.file_failures" }
        if legacy { return "history.status.legacy" }
        if issues.isEmpty, context?.dry_run == true, finished?.outcome == "completed" {
            return "history.status.preview"
        }
        guard issues.isEmpty, context != nil, let summary, summary.img.complete, summary.vid.complete,
              finished?.outcome == "completed" else { return "history.status.incomplete" }
        if hasPending { return "history.status.unfinished" }
        if hasVerificationWarnings { return "history.status.verification_warnings" }
        return "history.status.completed"
    }
    var needsAttention: Bool {
        !["history.status.completed", "history.status.preview"].contains(statusKey) || hasVerificationWarnings
    }
    var statusColor: NSColor {
        if hasPartialSuccess { return .systemOrange }
        if finished?.outcome == "failed" || hasFailures { return .systemRed }
        return needsAttention ? .systemOrange : .labelColor
    }
    var searchable: String {
        ([stamp, timestamp, context?.mode ?? "", modeLabel ?? "", localized(statusKey)] + (context?.inputs ?? [])
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
    var sizeOutcome: String {
        guard !isRestoreJpeg else { return localized("history.space.not_applicable_restore") }
        guard let input = summary?.size?.input_bytes, let output = summary?.size?.output_bytes else {
            return localized("result.unknown")
        }
        if input > output { return localized("history.space.saved", Self.sizeLabel(input - output)) }
        if output > input { return localized("history.space.increased", Self.sizeLabel(output - input)) }
        return localized("history.space.unchanged")
    }
    static func sizeLabel(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .binary) + " (\(bytes) B)"
    }
    func sizeMeasurementsComparable(to other: HistoryEntry) -> Bool {
        guard !isRestoreJpeg, !other.isRestoreJpeg,
               issues.isEmpty, other.issues.isEmpty, context?.dry_run == false, other.context?.dry_run == false,
              finished.map({ ["completed", "failed"].contains($0.outcome) }) == true,
              other.finished.map({ ["completed", "failed"].contains($0.outcome) }) == true,
              let left = summary, let right = other.summary, left.valid, right.valid,
              left.img.complete, left.vid.complete, right.img.complete, right.vid.complete,
              let leftSize = left.size, let rightSize = right.size, !left.sizeInvalid, !right.sizeInvalid,
              leftSize.input_bytes != nil, leftSize.output_bytes != nil,
              rightSize.input_bytes != nil, rightSize.output_bytes != nil else { return false }
        return leftSize.scope == rightSize.scope && left.count_scope == right.count_scope
            && left.img.active == right.img.active && left.vid.active == right.vid.active
    }
}

private struct HistoryAudit {
    let entries: [HistoryEntry]
    let hasEnd: Bool

    var evidenceWeight: Int {
        entries.reduce(0) { total, entry in
            total + (entry.context == nil ? 0 : 1) + (entry.summary == nil ? 0 : 1)
                + (entry.verification == nil ? 0 : 1) + (entry.configSnapshots.isEmpty ? 0 : 1)
                + (entry.inputPlan == nil ? 0 : 1) + entry.inputVerifications.count
                + (entry.finished == nil ? 0 : 1)
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
    static let maximumOutputLines = 1_000
    static let maximumOutputBytes = 2 * 1024 * 1024
    static let maximumConfigBytes = 32 * 1024
    static let maximumConfigSnapshots = 64

    struct Output {
        struct Row {
            let sequence: Int64
            let text: String
            let diagnostic: Bool
            let pipeline: String?
            let line: String?
        }
        struct DiagnosticGroup {
            let pipeline: String?
            let phase: String?
            let severity: String
            let message: String
            var count: Int
            let firstSequence: Int64
            var lastSequence: Int64
            let firstOriginal: String
            var lastOriginal: String
        }
        private struct DiagnosticKey: Hashable {
            let pipeline: String?
            let phase: String?
            let severity: String
            let message: String
        }
        let rows: [Row]
        let total: Int64
        let upper: Int64
        let firstOrdinal: Int64
        let issues: [String]
        var text: String { rows.map(\.text).joined(separator: "\n") }
        var shown: Int { rows.count }
        var hasOlder: Bool { shown > 0 && firstOrdinal > 1 }
        var hasNewer: Bool { shown > 0 && firstOrdinal + Int64(shown) - 1 < total }
        var diagnosticGroups: [DiagnosticGroup] {
            var accumulator = DiagnosticAccumulator()
            for row in rows { accumulator.ingest(row) }
            return accumulator.groups
        }

        struct DiagnosticAccumulator {
            private var phases: [String: PhaseLogPresentation] = [:]
            private var indices: [DiagnosticKey: Int] = [:]
            private(set) var groups: [DiagnosticGroup] = []
            private(set) var retainedBytes = 0
            private(set) var occurrences = 0
            private(set) var invalidPhaseSeen = false

            mutating func ingest(_ row: Row) {
                var invalidPhase = false
                if row.line == nil {
                    if let pipeline = row.pipeline { phases[pipeline] = PhaseLogPresentation(now: 0) }
                    else { phases.removeAll() }
                }
                if let pipeline = row.pipeline, let line = row.line {
                    var state = phases[pipeline] ?? PhaseLogPresentation(now: 0)
                    if let update = state.ingest(line, now: 0) {
                        if update == .invalid {
                            state = PhaseLogPresentation(now: 0)
                            invalidPhase = true
                            invalidPhaseSeen = true
                        }
                        phases[pipeline] = state
                        if !invalidPhase { return }
                    }
                }
                guard row.diagnostic || invalidPhase else { return }
                let phase = row.pipeline.flatMap { phases[$0]?.phase?.rawValue }
                let message = invalidPhase ? localized("log.phase.invalid") + "\n" + (row.line ?? row.text)
                    : row.line.map(LogTone.message) ?? row.text
                let severity = invalidPhase ? "warning" : row.line.map {
                    LogTone.classify($0) == .warning ? "warning" : "failure"
                } ?? "unreadable"
                let key = DiagnosticKey(pipeline: row.pipeline, phase: phase, severity: severity, message: message)
                if let index = indices[key] {
                    groups[index].count += 1
                    groups[index].lastSequence = row.sequence
                    retainedBytes += row.text.utf8.count - groups[index].lastOriginal.utf8.count
                    groups[index].lastOriginal = row.text
                } else {
                    indices[key] = groups.count
                    groups.append(DiagnosticGroup(pipeline: row.pipeline, phase: phase, severity: severity,
                        message: message, count: 1, firstSequence: row.sequence, lastSequence: row.sequence,
                        firstOriginal: row.text, lastOriginal: row.text))
                    retainedBytes += message.utf8.count + row.text.utf8.count * 2
                        + (row.pipeline?.utf8.count ?? 0) + (phase?.utf8.count ?? 0)
                }
                occurrences += 1
            }
        }
    }

    struct BatchIssues {
        let groups: [Output.DiagnosticGroup]
        let occurrences: Int
        let scannedRows: Int64
        let start: Int64
        let upper: Int64
        let endSource: String
    }

    final class IssueScanControl {
        private let lock = NSLock()
        private var cancelled = false
        private let deadline = ProcessInfo.processInfo.systemUptime + 15

        func cancel() { lock.lock(); cancelled = true; lock.unlock() }
        var isCancelled: Bool {
            lock.lock()
            defer { lock.unlock() }
            return cancelled
        }
        var shouldInterrupt: Bool { isCancelled || ProcessInfo.processInfo.systemUptime >= deadline }
        func check() throws {
            if isCancelled { throw HostError(message: localized("history.batch_issues.cancelled")) }
            if shouldInterrupt { throw HostError(message: localized("history.batch_issues.limit")) }
        }
    }

    enum OutputDirection { case older, newer }

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
        func decodeConfig(_ event: String) throws -> HistoryConfigSnapshot {
            let data = Data(event.dropFirst("MFB_HISTORY_CONFIG=".count).utf8)
            guard data.count <= maximumConfigBytes,
                  let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let values = object["config"] as? [String: Any] else { throw HistoryParseError.invalid }
            let metadata = try decoder.decode(HistoryConfigMetadata.self, from: data)
            func leafPaths(_ object: [String: Any], prefix: String = "") -> Set<String> {
                object.reduce(into: Set<String>()) { paths, field in
                    let path = prefix.isEmpty ? field.key : prefix + "." + field.key
                    if let nested = field.value as? [String: Any] { paths.formUnion(leafPaths(nested, prefix: path)) }
                    else { paths.insert(path) }
                }
            }
            guard metadata.schema_version == 1, ["img", "vid"].contains(metadata.pipeline),
                  !metadata.package_version.isEmpty, !metadata.sources.isEmpty,
                  metadata.sources.values.allSatisfy({ !$0.isEmpty }),
                  Set(metadata.sources.keys) == leafPaths(values),
                  let version = values["config_version"] as? NSNumber,
                  CFGetTypeID(version) != CFBooleanGetTypeID(), version == NSNumber(value: 1)
            else { throw HistoryParseError.invalid }
            let valuesData = try JSONSerialization.data(withJSONObject: values, options: [.prettyPrinted, .sortedKeys])
            let sourcesData = try JSONSerialization.data(withJSONObject: metadata.sources, options: [.prettyPrinted, .sortedKeys])
            guard let valuesJSON = String(data: valuesData, encoding: .utf8),
                  let sourcesJSON = String(data: sourcesData, encoding: .utf8) else { throw HistoryParseError.invalid }
            return HistoryConfigSnapshot(pipeline: metadata.pipeline, packageVersion: metadata.package_version,
                                         values: valuesJSON, sources: sourcesJSON)
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
                    let context = try decode(HistoryContext.self, from: event, prefix: "MFB_HISTORY_CONTEXT=")
                    guard context.schema_version == 1, !context.mode.isEmpty,
                          context.output_sequence.map({ $0 >= 0 }) ?? true else { throw HistoryParseError.invalid }
                    if hasContext {
                        guard entries.count < maximumEntriesPerSession - 1 else {
                            current.issues.append(localized("history.issue.entry_limit"))
                            break
                        }
                        current.outputUpperSequence = context.output_sequence
                        entries.append(current)
                        current = HistoryEntry(id: stamp + ":\(entries.count)", stamp: stamp, audit: audit, timestamp: record.ts)
                    }
                    hasContext = true
                    current.legacy = false
                    current.timestamp = record.ts
                    current.context = context
                    current.legacy = false
                } else if event.hasPrefix("MFB_HISTORY_SUMMARY=") {
                    var summary = try decode(HistorySummary.self, from: event, prefix: "MFB_HISTORY_SUMMARY=")
                    guard summary.valid, !current.contradictoryFields.contains("summary") else { throw HistoryParseError.invalid }
                    if current.isRestoreJpeg, summary.size != nil {
                        summary.size = nil
                        summary.sizeInvalid = true
                    }
                    if let previous = current.summary {
                        guard previous.sameOutcomes(as: summary) else {
                            current.summary = nil
                            current.contradictoryFields.insert("summary")
                            throw HistoryParseError.invalid
                        }
                        if previous.size != summary.size || previous.sizeInvalid != summary.sizeInvalid {
                            var merged = previous
                            merged.size = nil
                            merged.sizeInvalid = true
                            current.summary = merged
                        }
                    } else {
                        current.summary = summary
                    }
                    if current.summary?.sizeInvalid == true, current.issues.count < 32,
                       !current.issues.contains(localized("history.space.invalid")) {
                        current.issues.append(localized("history.space.invalid"))
                    }
                } else if event.hasPrefix("MFB_HISTORY_INPUT_PLAN=") {
                    let plan = try decode(HistoryInputPlan.self, from: event, prefix: "MFB_HISTORY_INPUT_PLAN=")
                    guard let context = current.context, plan.valid(for: context),
                          !current.contradictoryFields.contains("input_plan"),
                          current.inputPlan.map({ $0 == plan }) ?? true
                    else { throw HistoryParseError.invalid }
                    current.inputPlan = plan
                } else if event.hasPrefix("MFB_HISTORY_VERIFICATION=") {
                    let verification = try decode(HistoryVerification.self, from: event, prefix: "MFB_HISTORY_VERIFICATION=")
                    guard verification.valid, !current.contradictoryFields.contains("verification") else { throw HistoryParseError.invalid }
                    if current.isRestoreJpeg {
                        guard let context = current.context, context.inputs.count == 1,
                              let output = context.output, verification.source_path == context.inputs.first,
                              verification.output_path == output else { throw HistoryParseError.invalid }
                    }
                    if let plan = current.inputPlan {
                        guard let source = verification.source_path,
                              let input = plan.inputs.first(where: { $0.source == source }), input.requires_verification,
                              input.output == verification.output_path,
                              current.inputVerifications[source].map({ $0 == verification }) ?? true
                        else { throw HistoryParseError.invalid }
                        current.inputVerifications[source] = verification
                    } else {
                        guard current.context.map({ $0.inputs.count <= 1 }) ?? true,
                              current.verification.map({ $0 == verification }) ?? true
                        else { throw HistoryParseError.invalid }
                        current.verification = verification
                    }
                } else if event.hasPrefix("MFB_HISTORY_CONFIG=") {
                    current.configMarkerSeen = true
                    let snapshot = try decodeConfig(event)
                    let metadata = try decode(HistoryConfigMetadata.self, from: event, prefix: "MFB_HISTORY_CONFIG=")
                    if let id = metadata.invocation_id {
                        guard let invocation = current.invocations.first(where: { $0.invocation_id == id }),
                              invocation.pipeline == snapshot.pipeline, current.invocationConfigs[id] == nil,
                              !current.invocationEvidenceInvalid else { throw HistoryParseError.invalid }
                        current.invocationConfigs[id] = snapshot
                    } else {
                        let object = try JSONSerialization.jsonObject(with: Data(event.dropFirst("MFB_HISTORY_CONFIG=".count).utf8)) as? [String: Any]
                        guard object?["invocation_id"] == nil, current.invocations.isEmpty else { throw HistoryParseError.invalid }
                    }
                    current.configInvocationCount += 1
                    if !current.configSet.contains(snapshot) {
                        guard current.configSnapshots.count < maximumConfigSnapshots else { throw HistoryParseError.invalid }
                        current.configSet.insert(snapshot)
                        current.configSnapshots.append(snapshot)
                    }
                } else if event.hasPrefix("MFB_HISTORY_INVOCATION=") {
                    let invocation = try decode(HistoryInvocation.self, from: event, prefix: "MFB_HISTORY_INVOCATION=")
                    guard current.context != nil, current.finished == nil, invocation.valid,
                          current.invocations.count < 1000, !current.invocationEvidenceInvalid,
                          !current.invocations.contains(where: { $0.invocation_id == invocation.invocation_id }) else {
                        throw HistoryParseError.invalid
                    }
                    current.invocations.append(invocation)
                } else if event.hasPrefix("MFB_HISTORY_FINISHED=") {
                    let finished = try decode(HistoryFinished.self, from: event, prefix: "MFB_HISTORY_FINISHED=")
                    guard finished.schema_version == 1, ["completed", "failed", "cancelled"].contains(finished.outcome),
                          current.context?.output_sequence == nil || finished.output_sequence != nil,
                          finished.output_sequence.map({ $0 >= (current.context?.output_sequence ?? 0) }) ?? true,
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
                if event.hasPrefix("MFB_HISTORY_CONFIG=") { current.configEvidenceInvalid = true }
                if event.hasPrefix("MFB_HISTORY_INVOCATION=") {
                    current.invocations.removeAll()
                    current.invocationConfigs.removeAll()
                    current.invocationEvidenceInvalid = true
                    current.contradictoryFields.insert("invocation")
                }
                if event.hasPrefix("MFB_HISTORY_SUMMARY=") {
                    current.summary = nil
                    current.contradictoryFields.insert("summary")
                } else if event.hasPrefix("MFB_HISTORY_VERIFICATION=") {
                    current.verification = nil
                    current.contradictoryFields.insert("verification")
                } else if event.hasPrefix("MFB_HISTORY_INPUT_PLAN=") {
                    current.inputPlan = nil
                    current.verification = nil
                    current.contradictoryFields.insert("input_plan")
                } else if event.hasPrefix("MFB_HISTORY_FINISHED=") {
                    current.finished = nil
                    current.contradictoryFields.insert("finished")
                }
                if current.issues.count < 32 { current.issues.append(localized("history.issue.payload", index + 1)) }
            }
        }
        if !sawRecord { current.issues.append(localized("history.issue.empty")) }
        entries.append(current)
        for index in entries.indices { entries[index].aggregateInputVerification() }
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

    fileprivate static func databasePath(_ url: URL) throws -> String {
        // Foundation preserves the /var alias; SQLite NOFOLLOW needs the real parent.
        let parent = url.deletingLastPathComponent().withUnsafeFileSystemRepresentation { path -> String? in
            guard let path, let resolved = realpath(path, nil) else { return nil }
            defer { free(resolved) }
            return String(cString: resolved)
        }
        guard let parent else { throw databaseError(nil, "resolve database parent directory") }
        return parent + "/" + url.lastPathComponent
    }

    static func output(for entry: HistoryEntry, cursor: Int64? = nil,
                       direction: OutputDirection = .older, upperSnapshot: Int64? = nil) throws -> Output {
        guard entry.hasStoredOutput, let start = entry.context?.output_sequence, start >= 0,
              regularFile(entry.audit) else { throw databaseError(nil, "processing output is unavailable") }
        var end = entry.finished?.output_sequence ?? entry.outputUpperSequence ?? Int64.max
        guard end >= start else { throw databaseError(nil, "invalid processing output range") }
        let path = try databasePath(entry.audit)
        var handle: OpaquePointer?
        guard sqlite3_open_v2(path, &handle, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK,
              let database = handle else {
            let error = databaseError(handle, "open \(path): \(String(cString: sqlite3_errmsg(handle))) (\(sqlite3_extended_errcode(handle)))")
            if let handle { sqlite3_close(handle) }
            throw error
        }
        defer { sqlite3_close(database) }
        sqlite3_limit(database, SQLITE_LIMIT_LENGTH, Int32(maximumAuditBytes))
        guard sqlite3_busy_timeout(database, 1_000) == SQLITE_OK,
              sqlite3_exec(database, "PRAGMA query_only=ON; BEGIN", nil, nil, nil) == SQLITE_OK else { throw databaseError(database) }
        let version = try statement(database, "PRAGMA user_version")
        defer { sqlite3_finalize(version) }
        guard sqlite3_step(version) == SQLITE_ROW, sqlite3_column_int(version, 0) == 2 else {
            throw databaseError(database, "unsupported output schema")
        }
        if let upperSnapshot { end = min(end, upperSnapshot) }
        else {
            let maximum = try statement(database, "SELECT COALESCE(MAX(sequence), 0) FROM history_output WHERE session_id=?")
            defer { sqlite3_finalize(maximum) }
            guard entry.stamp.withCString({ sqlite3_bind_text(maximum, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
                  sqlite3_step(maximum) == SQLITE_ROW else { throw databaseError(database) }
            end = min(end, max(start, sqlite3_column_int64(maximum, 0)))
        }
        func bind(_ query: OpaquePointer) throws {
            guard entry.stamp.withCString({ sqlite3_bind_text(query, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
                  sqlite3_bind_int64(query, 2, start) == SQLITE_OK, sqlite3_bind_int64(query, 3, end) == SQLITE_OK else {
                throw databaseError(database)
            }
        }
        let count = try statement(database, "SELECT COUNT(*) FROM history_output WHERE session_id=? AND sequence>? AND sequence<=?")
        defer { sqlite3_finalize(count) }
        try bind(count)
        guard sqlite3_step(count) == SQLITE_ROW, sqlite3_column_type(count, 0) == SQLITE_INTEGER,
              sqlite3_column_int64(count, 0) >= 0 else { throw databaseError(database, "invalid output count") }
        let total = sqlite3_column_int64(count, 0)
        let ascending = direction == .newer
        // Inspect lengths before fetching payloads so an oversized row can become an explicit, navigable placeholder.
        let columns = ["ts", "pipeline", "line"].map {
            "CASE WHEN typeof(\($0))='text' AND length(CAST(\($0) AS BLOB))<=\(maximumOutputBytes) THEN \($0) ELSE NULL END"
        }.joined(separator: ", ")
        let cursorClause = cursor == nil ? "" : " AND sequence\(ascending ? ">" : "<")?"
        let query = try statement(database, "SELECT sequence, \(columns) FROM history_output WHERE session_id=? AND sequence>? AND sequence<=?\(cursorClause) ORDER BY sequence \(ascending ? "ASC" : "DESC") LIMIT \(maximumOutputLines)")
        defer { sqlite3_finalize(query) }
        try bind(query)
        if let cursor, sqlite3_bind_int64(query, 4, cursor) != SQLITE_OK { throw databaseError(database) }
        var rows: [Output.Row] = []
        var issues: [String] = []
        var bytes = 0
        while true {
            let result = sqlite3_step(query)
            if result == SQLITE_DONE { break }
            guard result == SQLITE_ROW else { throw databaseError(database) }
            let sequence = sqlite3_column_int64(query, 0)
            var text: String
            var diagnostic = false
            var pipeline: String?
            var line: String?
            do {
                let ts = try databaseText(query, 1, database: database)
                let rawPipeline = try databaseText(query, 2, database: database)
                pipeline = rawPipeline
                let rawLine = try databaseText(query, 3, database: database)
                text = "[\(ts)] \(rawPipeline): \(rawLine)"
                let tone = LogTone.classify(rawLine)
                diagnostic = tone == .warning || tone == .failure
                if text.utf8.count + 1 > maximumOutputBytes { throw HistoryParseError.invalid }
                line = rawLine
            } catch {
                text = localized("history.output.row_issue", sequence)
                issues.append(text)
                diagnostic = true
            }
            let size = text.utf8.count + 1
            guard size <= maximumOutputBytes - bytes else { break }
            bytes += size
            rows.append(Output.Row(sequence: sequence, text: text, diagnostic: diagnostic, pipeline: pipeline, line: line))
        }
        if !ascending { rows.reverse() }
        var ordinal: Int64 = 0
        if let first = rows.first {
            let rank = try statement(database, "SELECT COUNT(*) FROM history_output WHERE session_id=? AND sequence>? AND sequence<=?")
            defer { sqlite3_finalize(rank) }
            try bind(rank)
            guard sqlite3_bind_int64(rank, 3, first.sequence) == SQLITE_OK, sqlite3_step(rank) == SQLITE_ROW else { throw databaseError(database) }
            ordinal = sqlite3_column_int64(rank, 0)
        }
        guard sqlite3_exec(database, "COMMIT", nil, nil, nil) == SQLITE_OK else { throw databaseError(database) }
        return Output(rows: rows, total: total, upper: end, firstOrdinal: ordinal, issues: issues)
    }

    static func batchIssues(for selected: HistoryEntry, control: IssueScanControl = IssueScanControl()) throws -> BatchIssues {
        try control.check()
        guard selected.hasStoredOutput, let selectedContext = selected.context,
              let start = selectedContext.output_sequence, start >= 0, regularFile(selected.audit) else {
            throw HostError(message: localized("history.batch_issues.invalid"))
        }
        let path = try databasePath(selected.audit)
        var handle: OpaquePointer?
        guard sqlite3_open_v2(path, &handle, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK,
              let database = handle else {
            let error = databaseError(handle)
            if let handle { sqlite3_close(handle) }
            throw error
        }
        defer { sqlite3_close(database) }
        sqlite3_limit(database, SQLITE_LIMIT_LENGTH, Int32(maximumAuditBytes))
        sqlite3_progress_handler(database, 1_000, { pointer in
            guard let pointer else { return 1 }
            return Unmanaged<IssueScanControl>.fromOpaque(pointer).takeUnretainedValue().shouldInterrupt ? 1 : 0
        }, Unmanaged.passUnretained(control).toOpaque())
        defer { sqlite3_progress_handler(database, 0, nil, nil) }
        guard sqlite3_busy_timeout(database, 1_000) == SQLITE_OK,
              sqlite3_exec(database, "PRAGMA query_only=ON; BEGIN", nil, nil, nil) == SQLITE_OK else {
            try control.check()
            throw databaseError(database)
        }
        let version = try statement(database, "PRAGMA user_version")
        defer { sqlite3_finalize(version) }
        guard sqlite3_step(version) == SQLITE_ROW, sqlite3_column_int(version, 0) == 2 else {
            try control.check()
            throw databaseError(database, "unsupported output schema")
        }
        var bytes = 0
        var limited = false
        let audit = try databaseAudit(database, stamp: selected.stamp, url: selected.audit, readBytes: &bytes, limited: &limited)
        try control.check()
        guard !limited else { throw HostError(message: localized("history.batch_issues.limit")) }
        guard let entry = audit.entries.first(where: { $0.id == selected.id && $0.context == selectedContext }),
              entry.issues.isEmpty, entry.contradictoryFields.isEmpty, !entry.legacy else {
            throw HostError(message: localized("history.batch_issues.invalid"))
        }
        let maximum = try statement(database, "SELECT COALESCE(MAX(sequence), 0) FROM history_output WHERE session_id=?")
        defer { sqlite3_finalize(maximum) }
        guard selected.stamp.withCString({ sqlite3_bind_text(maximum, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
              sqlite3_step(maximum) == SQLITE_ROW, sqlite3_column_type(maximum, 0) == SQLITE_INTEGER else {
            try control.check()
            throw databaseError(database)
        }
        let snapshotEnd = sqlite3_column_int64(maximum, 0)
        let knownEnd = entry.finished?.output_sequence ?? entry.outputUpperSequence
        let end = knownEnd ?? max(start, snapshotEnd)
        guard end >= start, knownEnd.map({ $0 == start || snapshotEnd >= $0 }) ?? true,
              entry.outputUpperSequence.map({ $0 >= start && end <= $0 }) ?? true else {
            throw HostError(message: localized("history.batch_issues.invalid"))
        }
        func bind(_ query: OpaquePointer) throws {
            guard selected.stamp.withCString({ sqlite3_bind_text(query, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
                  sqlite3_bind_int64(query, 2, start) == SQLITE_OK, sqlite3_bind_int64(query, 3, end) == SQLITE_OK else {
                throw databaseError(database)
            }
        }
        let count = try statement(database, "SELECT COUNT(*) FROM history_output WHERE session_id=? AND sequence>? AND sequence<=?")
        defer { sqlite3_finalize(count) }
        try bind(count)
        guard sqlite3_step(count) == SQLITE_ROW, sqlite3_column_type(count, 0) == SQLITE_INTEGER else {
            try control.check()
            throw databaseError(database)
        }
        let total = sqlite3_column_int64(count, 0)
        guard total >= 0, total <= Int64(maximumRecordsPerSession) else { throw HostError(message: localized("history.batch_issues.limit")) }
        let columns = ["ts", "pipeline", "line"].map {
            "CASE WHEN typeof(\($0))='text' AND length(CAST(\($0) AS BLOB))<=\(maximumOutputBytes) THEN \($0) ELSE NULL END"
        }.joined(separator: ", ")
        let query = try statement(database, "SELECT sequence, \(columns) FROM history_output WHERE session_id=? AND sequence>? AND sequence<=? ORDER BY sequence")
        defer { sqlite3_finalize(query) }
        try bind(query)
        var accumulator = Output.DiagnosticAccumulator()
        var scanned: Int64 = 0
        var previous = start
        while true {
            try control.check()
            let result = sqlite3_step(query)
            if result == SQLITE_DONE { break }
            guard result == SQLITE_ROW else { try control.check(); throw databaseError(database) }
            let sequence = sqlite3_column_int64(query, 0)
            guard sqlite3_column_type(query, 0) == SQLITE_INTEGER, sequence > previous, sequence <= end else {
                throw HostError(message: localized("history.batch_issues.invalid"))
            }
            let length = (1...3).reduce(0) { $0 + Int(sqlite3_column_bytes(query, Int32($1))) }
            guard length <= maximumOutputBytes, length <= maximumScanBytes - bytes else {
                throw HostError(message: localized("history.batch_issues.limit"))
            }
            let timestamp = try databaseText(query, 1, database: database)
            let pipeline = try databaseText(query, 2, database: database)
            let line = try databaseText(query, 3, database: database)
            guard !timestamp.isEmpty, !pipeline.isEmpty else { throw HostError(message: localized("history.batch_issues.invalid")) }
            let tone = LogTone.classify(line)
            accumulator.ingest(Output.Row(sequence: sequence, text: "[\(timestamp)] \(pipeline): \(line)",
                diagnostic: tone == .warning || tone == .failure, pipeline: pipeline, line: line))
            guard !accumulator.invalidPhaseSeen else { throw HostError(message: localized("history.batch_issues.invalid")) }
            guard accumulator.groups.count <= 4_096, accumulator.retainedBytes <= maximumOutputBytes else {
                throw HostError(message: localized("history.batch_issues.limit"))
            }
            bytes += length
            scanned += 1
            previous = sequence
        }
        try control.check()
        guard scanned == total else { throw HostError(message: localized("history.batch_issues.invalid")) }
        guard sqlite3_exec(database, "COMMIT", nil, nil, nil) == SQLITE_OK else {
            try control.check()
            throw databaseError(database)
        }
        return BatchIssues(groups: accumulator.groups, occurrences: accumulator.occurrences,
            scannedRows: scanned, start: start, upper: end, endSource: entry.outputEndSource)
    }

    static func diagnosticArtifact(for selected: HistoryEntry) throws -> Data {
        guard selected.hasStoredOutput, let selectedContext = selected.context,
              let start = selectedContext.output_sequence, regularFile(selected.audit) else {
            throw HostError(message: localized("history.export.invalid"))
        }
        let path = try databasePath(selected.audit)
        var handle: OpaquePointer?
        guard sqlite3_open_v2(path, &handle, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK,
              let database = handle else {
            let error = databaseError(handle)
            if let handle { sqlite3_close(handle) }
            throw error
        }
        defer { sqlite3_close(database) }
        sqlite3_limit(database, SQLITE_LIMIT_LENGTH, Int32(maximumAuditBytes))
        guard sqlite3_busy_timeout(database, 1_000) == SQLITE_OK,
              sqlite3_exec(database, "PRAGMA query_only=ON; BEGIN", nil, nil, nil) == SQLITE_OK else { throw databaseError(database) }
        let version = try statement(database, "PRAGMA user_version")
        defer { sqlite3_finalize(version) }
        guard sqlite3_step(version) == SQLITE_ROW, sqlite3_column_int(version, 0) == 2 else { throw databaseError(database, "unsupported output schema") }
        var bytes = 0
        var limited = false
        let audit = try databaseAudit(database, stamp: selected.stamp, url: selected.audit, readBytes: &bytes, limited: &limited)
        guard !limited else { throw HostError(message: localized("history.export.limit")) }
        guard let entry = audit.entries.first(where: { $0.id == selected.id && $0.context == selectedContext }),
               entry.issues.isEmpty, entry.contradictoryFields.isEmpty, !entry.configEvidenceInvalid, !entry.invocationEvidenceInvalid,
              !entry.legacy, let context = entry.context else { throw HostError(message: localized("history.export.invalid")) }
        let maximum = try statement(database, "SELECT COALESCE(MAX(sequence), 0) FROM history_output WHERE session_id=?")
        defer { sqlite3_finalize(maximum) }
        guard selected.stamp.withCString({ sqlite3_bind_text(maximum, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
              sqlite3_step(maximum) == SQLITE_ROW, sqlite3_column_type(maximum, 0) == SQLITE_INTEGER else { throw databaseError(database) }
        let snapshotEnd = sqlite3_column_int64(maximum, 0)
        let knownEnd = entry.finished?.output_sequence ?? entry.outputUpperSequence
        let end = knownEnd ?? snapshotEnd
        guard start >= 0, end >= start, snapshotEnd >= end,
              entry.outputUpperSequence.map({ $0 >= start && end <= $0 }) ?? true else {
            throw HostError(message: localized("history.export.invalid"))
        }
        let query = try statement(database, "SELECT sequence, ts, pipeline, line FROM history_output WHERE session_id=? AND sequence>? AND sequence<=? ORDER BY sequence LIMIT \(maximumRecordsPerSession + 1)")
        defer { sqlite3_finalize(query) }
        guard selected.stamp.withCString({ sqlite3_bind_text(query, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
              sqlite3_bind_int64(query, 2, start) == SQLITE_OK, sqlite3_bind_int64(query, 3, end) == SQLITE_OK else { throw databaseError(database) }
        var retained: [[String: Any]] = []
        var total = 0
        var diagnosticCount = 0
        var phaseCount = 0
        var activityCount = 0
        var phases: [String: PhaseLogPresentation] = [:]
        var activities: [String: GateCheckActivity] = [:]
        while true {
            let result = sqlite3_step(query)
            if result == SQLITE_DONE { break }
            guard result == SQLITE_ROW else { throw databaseError(database) }
            guard total < maximumRecordsPerSession else { throw HostError(message: localized("history.export.limit")) }
            let sequence = sqlite3_column_int64(query, 0)
            guard sqlite3_column_type(query, 0) == SQLITE_INTEGER, sequence > start, sequence <= end else { throw HostError(message: localized("history.export.invalid")) }
            let length = (1...3).reduce(0) { $0 + Int(sqlite3_column_bytes(query, Int32($1))) }
            guard length <= maximumOutputBytes, length <= maximumScanBytes - bytes else { throw HostError(message: localized("history.export.limit")) }
            bytes += length
            total += 1
            let timestamp = try databaseText(query, 1, database: database)
            let pipeline = try databaseText(query, 2, database: database)
            let line = try databaseText(query, 3, database: database)
            guard !timestamp.isEmpty, !pipeline.isEmpty else { throw HostError(message: localized("history.export.invalid")) }
            let normalized = LogTone.message(line)
            var phase = phases[pipeline] ?? PhaseLogPresentation(now: 0)
            if let update = phase.ingest(normalized, now: 0) {
                guard update != .invalid else { throw HostError(message: localized("history.export.invalid")) }
                phases[pipeline] = phase
                if phase.phase == .processing { activities[pipeline] = nil }
                phaseCount += 1
                retained.append(["sequence": sequence, "timestamp": timestamp, "pipeline": pipeline, "line": line,
                    "normalized_message": normalized, "kind": "phase", "phase": phase.phase?.rawValue as Any? ?? NSNull()])
            } else {
                var activity = activities[pipeline] ?? GateCheckActivity()
                if activity.ingest(normalized, now: 0) {
                    guard !activity.invalid else { throw HostError(message: localized("history.export.invalid")) }
                    activities[pipeline] = activity
                    activityCount += 1
                    retained.append(["sequence": sequence, "timestamp": timestamp, "pipeline": pipeline, "line": line,
                        "normalized_message": normalized, "kind": "verification_activity",
                        "phase": phases[pipeline]?.phase?.rawValue as Any? ?? NSNull()])
                    continue
                }
                let tone = LogTone.classify(line)
                guard tone == .warning || tone == .failure else { continue }
                diagnosticCount += 1
                retained.append(["sequence": sequence, "timestamp": timestamp, "pipeline": pipeline, "line": line,
                    "normalized_message": normalized, "kind": tone == .warning ? "warning" : "error",
                    "phase": phases[pipeline]?.phase?.rawValue as Any? ?? NSNull()])
            }
        }
        func nullable<T>(_ value: T?) -> Any { value.map { $0 as Any } ?? NSNull() }
        func media(_ value: HistoryMedia) -> [String: Any] {
            ["active": value.active, "succeeded": nullable(value.succeeded), "skipped": nullable(value.skipped),
             "ignored": nullable(value.ignored), "failed": nullable(value.failed), "unprocessed": nullable(value.unprocessed),
             "exit_code": nullable(value.exit_code)]
        }
        func verification(_ value: HistoryVerification) -> [String: Any] {
            ["schema_version": value.schema_version, "has_warnings": value.has_warnings, "issue_count": value.issue_count,
             "source_count": value.source_count, "optimized_count": value.optimized_count, "skipped_count": value.skipped_count,
             "failed_count": value.failed_count, "source_remaining_count": value.source_remaining_count,
             "verified_deleted_count": value.verified_deleted_count, "count_status": nullable(value.count_status),
             "source_path": nullable(value.source_path), "output_path": nullable(value.output_path)]
        }
        let contextObject: [String: Any] = ["schema_version": context.schema_version, "inputs": context.inputs,
            "output": nullable(context.output), "backup": nullable(context.backup), "mode": context.mode,
            "dry_run": context.dry_run, "in_place": context.in_place, "shortest_path": context.shortest_path,
            "version": nullable(context.version), "retry": nullable(context.retry), "resume": nullable(context.resume),
            "no_resume": nullable(context.no_resume), "output_sequence": nullable(context.output_sequence)]
        let summaryObject: [String: Any]? = entry.summary.map { value in
            ["schema_version": value.schema_version, "count_scope": value.count_scope, "img": media(value.img), "vid": media(value.vid),
             "integrity": ["state": nullable(value.integrity.state), "issue_count": nullable(value.integrity.issue_count)],
             "failed_files": value.failed_files, "skipped_files": value.skipped_files, "size_invalid": value.sizeInvalid,
             "size": nullable(value.size.map { ["scope": $0.scope, "input_bytes": nullable($0.input_bytes), "output_bytes": nullable($0.output_bytes)] })]
        }
        let finishedObject: [String: Any]? = entry.finished.map {
            ["schema_version": $0.schema_version, "outcome": $0.outcome, "error": nullable($0.error), "output_sequence": nullable($0.output_sequence)]
        }
        let configurations: [[String: Any]] = try entry.configSnapshots.map {
            ["pipeline": $0.pipeline, "package_version": $0.packageVersion,
             "config": try JSONSerialization.jsonObject(with: Data($0.values.utf8)),
             "sources": try JSONSerialization.jsonObject(with: Data($0.sources.utf8))]
        }
        let launches: [[String: Any]] = try entry.invocations.map { invocation in
            guard var object = try JSONSerialization.jsonObject(with: JSONEncoder().encode(invocation)) as? [String: Any] else {
                throw HostError(message: localized("history.export.invalid"))
            }
            object["program_path"] = nullable(invocation.program_path)
            object["binary"] = ["scope": invocation.binary.scope, "bytes": nullable(invocation.binary.bytes),
                "blake3": nullable(invocation.binary.blake3), "diagnostic": nullable(invocation.binary.diagnostic)]
            if let config = entry.invocationConfigs[invocation.invocation_id] {
                object["configuration"] = ["pipeline": config.pipeline, "package_version": config.packageVersion,
                    "config": try JSONSerialization.jsonObject(with: Data(config.values.utf8)),
                    "sources": try JSONSerialization.jsonObject(with: Data(config.sources.utf8))]
            } else { object["configuration"] = NSNull() }
            return object
        }
        let plan: [String: Any]? = entry.inputPlan.map {
            ["schema_version": $0.schema_version, "inputs": $0.inputs.map {
                ["source": $0.source, "output": nullable($0.output), "in_place": $0.in_place, "requires_verification": $0.requires_verification]
            }]
        }
        let artifact: [String: Any] = ["schema_version": 1, "artifact_kind": "mfb_selected_run_diagnostic",
            "authoritative_state": false, "full_run_manifest": false,
            "exported_at": ISO8601DateFormatter().string(from: Date()), "session_id": entry.stamp, "run_id": entry.id,
            "scope": ["output_start_exclusive": start, "output_end_inclusive": end, "known_end_sequence": nullable(knownEnd),
                "end_source": entry.outputEndSource,
                "total_output_rows": total, "retained_output_rows": retained.count, "diagnostic_log_rows": diagnosticCount,
                "phase_control_rows": phaseCount, "verification_activity_rows": activityCount,
                "omitted_non_diagnostic_output_rows": total - retained.count,
                "scan_complete": true, "count_scope": "log_records_not_files", "contains_local_paths_and_diagnostics": true],
            "context": contextObject, "summary": nullable(summaryObject), "finished": nullable(finishedObject),
            "configuration": ["marker_seen": entry.configMarkerSeen, "invocation_count": entry.configInvocationCount, "snapshots": configurations],
            "worker_launches": launches.isEmpty ? NSNull() : launches,
            "input_plan": nullable(plan), "verification": nullable(entry.verification.map(verification)),
            "input_verifications": entry.inputVerifications.mapValues(verification), "records": retained]
        let data = try JSONSerialization.data(withJSONObject: artifact, options: [.prettyPrinted, .sortedKeys])
        guard data.count <= maximumScanBytes else { throw HostError(message: localized("history.export.limit")) }
        guard sqlite3_exec(database, "COMMIT", nil, nil, nil) == SQLITE_OK else { throw databaseError(database) }
        return data
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
                } else if !archived, values.isDirectory == true,
                          name.hasPrefix("Bundle_") || (name.contains("_Bundle_") && !name.hasPrefix("_Bundle_")) {
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
            let path = try databasePath(databaseURL)
            guard sqlite3_open_v2(path, &database, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK,
                  let database else { throw databaseError(database, "open \(path): \(String(cString: sqlite3_errmsg(database))) (\(sqlite3_extended_errcode(database)))") }
            sqlite3_limit(database, SQLITE_LIMIT_LENGTH, Int32(maximumAuditBytes))
            guard sqlite3_exec(database, "BEGIN", nil, nil, nil) == SQLITE_OK else { throw databaseError(database) }
            let version = try statement(database, "PRAGMA user_version")
            defer { sqlite3_finalize(version) }
            guard sqlite3_step(version) == SQLITE_ROW else { throw databaseError(database) }
            let schema = sqlite3_column_int(version, 0)
            guard schema == 1 || schema == 2 else { throw databaseError(database, "unsupported schema \(schema)") }
            if schema == 2 {
                let outputSchema = try statement(database, "SELECT sequence, session_id, ts, pipeline, line FROM history_output LIMIT 0")
                sqlite3_finalize(outputSchema)
            }
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
                        if let a = left.inputPlan, let b = right.inputPlan, a != b { conflictingCopies = true }
                        for (source, receipt) in left.inputVerifications {
                            if let other = right.inputVerifications[source], receipt != other { conflictingCopies = true }
                        }
                        if !left.configSet.isEmpty && !right.configSet.isEmpty,
                           !left.configSet.isSubset(of: right.configSet), !right.configSet.isSubset(of: left.configSet) {
                            conflictingCopies = true
                        }
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
                        entry.inputPlan = nil
                        entry.inputVerifications = [:]
                        entry.configSnapshots = []
                        entry.configSet = []
                        entry.configInvocationCount = 0
                        entry.configMarkerSeen = true
                        entry.invocations = []
                        entry.invocationConfigs = [:]
                        entry.invocationEvidenceInvalid = true
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

private final class HistoryDiagnosticStaging {
    private let directory: Int32
    private let parent: URL
    private let temporaryName: String
    private let destinationName: String
    private var pending = true

    private init(directory: Int32, parent: URL, temporaryName: String, destinationName: String) {
        self.directory = directory
        self.parent = parent
        self.temporaryName = temporaryName
        self.destinationName = destinationName
    }

    private static func ioError(_ operation: String) -> NSError {
        let code = errno
        return NSError(domain: NSPOSIXErrorDomain, code: Int(code),
            userInfo: [NSLocalizedDescriptionKey: operation + ": " + String(cString: strerror(code))])
    }

    private static func rejectGitWorkspace(_ parent: URL) throws {
        var ancestor = parent
        while true {
            var info = stat()
            if lstat(ancestor.appendingPathComponent(".git").path, &info) == 0 {
                throw HostError(message: localized("history.export.workspace"))
            }
            guard errno == ENOENT else { throw ioError("inspect diagnostic export workspace") }
            if ancestor.path == "/" { return }
            ancestor.deleteLastPathComponent()
        }
    }

    static func prepare(_ data: Data, destination: URL) throws -> HistoryDiagnosticStaging {
        guard destination.isFileURL, destination.path.hasPrefix("/"), !destination.path.contains("\0"),
              !["", ".", ".."].contains(destination.lastPathComponent), !data.isEmpty else {
            throw HostError(message: localized("history.export.invalid"))
        }
        let parent = URL(fileURLWithPath: try HistoryStore.databasePath(destination)).deletingLastPathComponent()
        try rejectGitWorkspace(parent)
        let directory = Darwin.open(parent.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        guard directory >= 0 else { throw ioError("open export directory") }
        let name = destination.lastPathComponent
        var existing = stat()
        if fstatat(directory, name, &existing, AT_SYMLINK_NOFOLLOW) == 0 {
            Darwin.close(directory)
            throw HostError(message: localized("history.export.exists"))
        }
        guard errno == ENOENT else {
            let error = ioError("inspect export destination")
            Darwin.close(directory)
            throw error
        }
        let temporaryName = ".mfb-diagnostic-" + UUID().uuidString + ".tmp"
        var file = openat(directory, temporaryName, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, S_IRUSR | S_IWUSR)
        guard file >= 0 else {
            let error = ioError("create export staging file")
            Darwin.close(directory)
            throw error
        }
        let staging = HistoryDiagnosticStaging(directory: directory, parent: parent, temporaryName: temporaryName, destinationName: name)
        defer {
            if file >= 0, Darwin.close(file) != 0 { fputs("MFB: failed to close diagnostic staging file\n", stderr) }
        }
        try data.withUnsafeBytes { buffer in
            guard let base = buffer.baseAddress else { throw HostError(message: localized("history.export.invalid")) }
            var written = 0
            while written < buffer.count {
                let count = Darwin.write(file, base.advanced(by: written), buffer.count - written)
                if count < 0, errno == EINTR { continue }
                guard count > 0 else { throw ioError("write diagnostic staging file") }
                written += count
            }
        }
        guard fsync(file) == 0 else { throw ioError("flush diagnostic staging file") }
        let descriptor = file
        file = -1
        guard Darwin.close(descriptor) == 0 else { throw ioError("close diagnostic staging file") }
        return staging
    }

    func publish() throws {
        guard pending else { throw HostError(message: localized("history.export.invalid")) }
        try Self.rejectGitWorkspace(parent)
        var opened = stat()
        var current = stat()
        guard fstat(directory, &opened) == 0, lstat(parent.path, &current) == 0,
              current.st_mode & S_IFMT == S_IFDIR, opened.st_dev == current.st_dev, opened.st_ino == current.st_ino else {
            throw HostError(message: localized("history.export.invalid"))
        }
        guard renameatx_np(directory, temporaryName, directory, destinationName, UInt32(RENAME_EXCL)) == 0 else {
            if errno == EEXIST { throw HostError(message: localized("history.export.exists")) }
            throw Self.ioError("publish diagnostic export")
        }
        pending = false
    }

    func discard() throws {
        guard pending else { return }
        guard unlinkat(directory, temporaryName, 0) == 0 || errno == ENOENT else { throw Self.ioError("remove diagnostic staging file") }
        pending = false
    }

    deinit {
        do { try discard() }
        catch { fputs("MFB: diagnostic staging cleanup failed: \(error.localizedDescription)\n", stderr) }
        if Darwin.close(directory) != 0 { fputs("MFB: failed to close diagnostic export directory\n", stderr) }
    }
}

private final class HistoryResultCell: NSTableCellView {
    var resultColor: NSColor = .labelColor {
        didSet { updateTextColor() }
    }
    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { updateTextColor() }
    }
    private func updateTextColor() {
        textField?.textColor = backgroundStyle == .emphasized ? .labelColor : resultColor
    }
}

final class ProcessingHistoryPanel: NSWindowController, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate, NSWindowDelegate {
    private let table = NSTableView()
    private let search = NSSearchField()
    private let filter = NSSegmentedControl(labels: ["", ""], trackingMode: .selectOne, target: nil, action: nil)
    private let detail = NSTextView()
    private let detailMode = NSSegmentedControl(labels: ["", "", "", ""], trackingMode: .selectOne, target: nil, action: nil)
    private let outputFilter = NSSegmentedControl(labels: ["", ""], trackingMode: .selectOne, target: nil, action: nil)
    private let failedTable = NSTableView()
    private let failedSearch = NSSearchField()
    private let failedStatus = NSTextField(wrappingLabelWithString: "")
    private let failedContent = NSStackView()
    private var failedCopyButton = NSButton()
    private var failedRevealButton = NSButton()
    private var failedPaths: [String] = []
    private var detailScroll = NSScrollView()
    private var outputNavigation = NSStackView()
    private var olderButton = NSButton()
    private var newerButton = NSButton()
    private var outputPage: HistoryStore.Output?
    private var outputLoading = false
    private var batchIssueResult: HistoryStore.BatchIssues?
    private var issueScanControl: HistoryStore.IssueScanControl?
    private var issueGeneration = UUID()
    private let status = NSTextField(labelWithString: "")
    private let progress = NSProgressIndicator()
    private let splitController = NSSplitViewController()
    private var split: NSSplitView { splitController.splitView }
    private var refreshButton = NSButton()
    private var logButton = NSButton()
    private var revealButton = NSButton()
    private var copyButton = NSButton()
    private var exportButton = NSButton()
    private var entries: [HistoryEntry] = []
    private var displayed: [HistoryEntry] = []
    private var directory: URL
    private var generation = UUID()
    private var outputGeneration = UUID()
    private var exportGeneration = UUID()
    private var exportBusy = false
    private var loadIssues: [String] = []
    private var limited = false
    private var initialDividerConfigured = false
    private var selectionInitialized = false

    init(directory: URL) {
        self.directory = directory
        outputGeneration = UUID()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1040, height: 620),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        super.init(window: window)
        window.minSize = NSSize(width: 780, height: 440)
        window.isReleasedWhenClosed = false
        window.delegate = self
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
        exportGeneration = UUID()
        exportBusy = false
        resetOutput()
        self.directory = directory
        let token = UUID()
        generation = token
        status.stringValue = localized("history.loading")
        progress.startAnimation(nil)
        refreshButton.isEnabled = false
        updateActions()
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
                    self.table.deselectAll(nil)
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
        table.allowsMultipleSelection = true
        table.allowsEmptySelection = true
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
        detailScroll = NSScrollView(frame: NSRect(x: 575, y: 0, width: 433, height: 540))
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
        detailMode.selectedSegment = 0
        detailMode.controlSize = .small
        detailMode.font = HistoryLayout.smallFont
        detailMode.target = self
        detailMode.action = #selector(detailModeChanged)
        outputFilter.selectedSegment = 0
        outputFilter.target = self
        outputFilter.action = #selector(outputFilterChanged)
        olderButton = iconButton("chevron.left", key: "history.output.older", action: #selector(olderOutput))
        newerButton = iconButton("chevron.right", key: "history.output.newer", action: #selector(newerOutput))
        outputFilter.controlSize = .small
        outputFilter.font = HistoryLayout.smallFont
        outputNavigation = NSStackView(views: [outputFilter, olderButton, newerButton])
        outputNavigation.spacing = 4
        configureFailedFiles()
        let detailRoot = NSStackView(views: [detailMode, outputNavigation, detailScroll, failedContent])
        detailRoot.orientation = .vertical
        detailRoot.alignment = .leading
        detailRoot.spacing = 6
        detailRoot.translatesAutoresizingMaskIntoConstraints = false
        detailScroll.setContentHuggingPriority(.defaultLow, for: .vertical)
        detailScroll.widthAnchor.constraint(equalTo: detailRoot.widthAnchor).isActive = true
        failedContent.widthAnchor.constraint(equalTo: detailRoot.widthAnchor).isActive = true
        // Keep toolbar intrinsic widths out of the split item's sizing negotiation.
        let detailContainer = NSView(frame: NSRect(x: 0, y: 0, width: 433, height: 540))
        detailContainer.addSubview(detailRoot)
        NSLayoutConstraint.activate([
            detailRoot.leadingAnchor.constraint(equalTo: detailContainer.leadingAnchor),
            detailRoot.trailingAnchor.constraint(equalTo: detailContainer.trailingAnchor),
            detailRoot.topAnchor.constraint(equalTo: detailContainer.topAnchor),
            detailRoot.bottomAnchor.constraint(equalTo: detailContainer.bottomAnchor)
        ])
        detailController.view = detailContainer
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
        exportButton = iconButton("arrow.up.doc", key: "history.export.title", action: #selector(exportSelectedRun))
        progress.style = .spinning
        progress.controlSize = .small
        progress.isDisplayedWhenStopped = false
        status.font = HistoryLayout.smallFont
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingMiddle
        status.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let footer = NSStackView(views: [logButton, revealButton, copyButton, exportButton, progress, status])
        footer.orientation = .horizontal
        footer.spacing = HistoryLayout.spacing
        root.addArrangedSubview(footer)
        footer.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        updateLocalization()
        updateActions()
        window?.center()
    }

    private func configureFailedFiles() {
        failedTable.delegate = self
        failedTable.dataSource = self
        failedTable.rowHeight = HistoryLayout.rowHeight
        failedTable.usesAlternatingRowBackgroundColors = true
        failedTable.style = .inset
        failedTable.allowsEmptySelection = true
        let path = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("failedPath"))
        path.minWidth = 100
        failedTable.addTableColumn(path)
        failedTable.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = failedTable
        scroll.setContentHuggingPriority(.defaultLow, for: .vertical)
        failedSearch.delegate = self
        failedSearch.sendsSearchStringImmediately = true
        failedSearch.setContentHuggingPriority(.defaultLow, for: .horizontal)
        failedCopyButton = iconButton("doc.on.doc", key: "history.failed_files.copy", action: #selector(copyFailedPath))
        failedRevealButton = iconButton("folder", key: "history.failed_files.reveal", action: #selector(revealFailedPath))
        let toolbar = NSStackView(views: [failedSearch, failedCopyButton, failedRevealButton])
        toolbar.spacing = 4
        failedStatus.font = HistoryLayout.smallFont
        failedStatus.textColor = .secondaryLabelColor
        failedContent.orientation = .vertical
        failedContent.alignment = .leading
        failedContent.spacing = 6
        for view in [toolbar, failedStatus, scroll] as [NSView] {
            failedContent.addArrangedSubview(view)
            view.widthAnchor.constraint(equalTo: failedContent.widthAnchor).isActive = true
        }
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
        detailMode.setLabel(localized("history.summary"), forSegment: 0)
        detailMode.setLabel(localized("history.output.title"), forSegment: 1)
        detailMode.setLabel(localized("history.failed_files"), forSegment: 2)
        detailMode.setLabel(localized("history.batch_issues.tab"), forSegment: 3)
        detailMode.setAccessibilityLabel(localized("history.details"))
        failedSearch.placeholderString = localized("history.failed_files.search")
        failedSearch.setAccessibilityLabel(localized("history.failed_files.search"))
        failedTable.setAccessibilityLabel(localized("history.failed_files"))
        failedTable.tableColumns.first?.title = localized("history.failed_files.path")
        outputFilter.setLabel(localized("history.output.all_page"), forSegment: 0)
        outputFilter.setLabel(localized("history.output.diagnostics_page"), forSegment: 1)
        outputFilter.setAccessibilityLabel(localized("history.output.filter_page"))
        for column in table.tableColumns { column.title = localized("history.column.\(column.identifier.rawValue)") }
        for (button, key) in [(refreshButton, "history.refresh"), (logButton, "history.open_log"),
                              (revealButton, "history.reveal"), (copyButton, "history.copy_sources"),
                              (exportButton, "history.export.title"),
                              (failedCopyButton, "history.failed_files.copy"), (failedRevealButton, "history.failed_files.reveal"),
                              (olderButton, "history.output.older"), (newerButton, "history.output.newer")] {
            button.toolTip = localized(key)
            button.setAccessibilityLabel(localized(key))
        }
        if refreshButton.isEnabled { applyFilter() }
    }

    @objc private func refreshRequested() { refresh(directory: directory) }
    @objc private func filterChanged() { applyFilter() }
    func controlTextDidChange(_ notification: Notification) {
        if (notification.object as? NSSearchField) === failedSearch { filterFailedFiles() }
        else { applyFilter() }
    }

    private var selectedEntries: [HistoryEntry] {
        table.selectedRowIndexes.compactMap { displayed.indices.contains($0) ? displayed[$0] : nil }
    }
    private var selected: HistoryEntry? {
        let entries = selectedEntries
        return entries.count == 1 ? entries[0] : nil
    }

    private func applyFilter() {
        let selectedIDs = Set(selectedEntries.map(\.id))
        let query = search.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        displayed = entries.filter {
            (filter.selectedSegment != 1 || $0.needsAttention)
                && (query.isEmpty || $0.searchable.localizedCaseInsensitiveContains(query))
        }
        table.reloadData()
        let indexes = IndexSet(displayed.indices.filter { selectedIDs.contains(displayed[$0].id) })
        if !indexes.isEmpty {
            table.selectRowIndexes(indexes, byExtendingSelection: false)
        } else if !selectionInitialized, !displayed.isEmpty {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        } else { table.deselectAll(nil) }
        if !entries.isEmpty { selectionInitialized = true }
        showSelected()
    }

    private func updateHistoryStatus() {
        status.stringValue = entries.isEmpty ? localized("history.empty")
            : localized("history.shown", displayed.count, entries.count)
        if limited { status.stringValue += " · " + localized("history.limited") }
        if !loadIssues.isEmpty { status.stringValue += " · " + loadIssues.joined(separator: " · ") }
        status.toolTip = directory.path
    }

    func numberOfRows(in tableView: NSTableView) -> Int { tableView === failedTable ? failedPaths.count : displayed.count }
    func tableViewSelectionDidChange(_ notification: Notification) {
        if (notification.object as? NSTableView) === failedTable { updateActions() }
        else { showSelected() }
    }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if tableView === failedTable {
            guard failedPaths.indices.contains(row) else { return nil }
            let label = NSTextField(labelWithString: failedPaths[row])
            label.font = HistoryLayout.smallFont
            label.lineBreakMode = .byTruncatingMiddle
            label.toolTip = failedPaths[row]
            label.setAccessibilityLabel(failedPaths[row])
            let cell = HistoryResultCell()
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
        guard displayed.indices.contains(row), let column = tableColumn else { return nil }
        let entry = displayed[row]
        let label = NSTextField(labelWithString: "")
        var resultColor = NSColor.labelColor
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
        case "status": label.stringValue = localized(entry.statusKey); resultColor = entry.statusColor
        case "verification":
            label.stringValue = entry.verification?.count_status ?? localized("result.unknown")
            resultColor = entry.verification?.count_status?.hasPrefix("MISMATCH") == true ? .systemRed : .secondaryLabelColor
        case "succeeded": label.stringValue = entry.succeededLabel; label.alignment = .right
        default: return nil
        }
        if label.toolTip == nil { label.toolTip = label.stringValue }
        let cell = HistoryResultCell()
        cell.textField = label
        cell.resultColor = resultColor
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
        resetOutput()
        failedSearch.stringValue = ""
        filterFailedFiles()
        updateHistoryStatus()
        let selection = selectedEntries
        detailMode.setLabel(localized(selection.count == 2 ? "history.compare" : "history.summary"), forSegment: 0)
        if selection.count == 2 { detail.string = Self.renderComparison(selection) }
        else if selection.count > 2 { detail.string = localized("history.compare.select_two") }
        else { detail.string = selected.map { Self.render($0) } ?? localized(displayed.isEmpty && !entries.isEmpty ? "history.no_match" : "history.select") }
        detail.scrollToBeginningOfDocument(nil)
        updateActions()
    }

    fileprivate static func renderComparison(_ entries: [HistoryEntry]) -> String {
        guard entries.count == 2 else { return localized("history.compare.select_two") }
        var sections = [localized("history.compare")]
        if !entries[0].sizeMeasurementsComparable(to: entries[1]) {
            sections.append(localized("history.compare.not_comparable"))
        }
        let unknown = localized("result.unknown")
        func count(_ value: Int?) -> String { value.map(String.init) ?? unknown }
        func row(_ key: String, _ value: String) -> String { localized("history.detail.\(key)") + ": " + value }
        for (index, entry) in entries.enumerated() {
            var lines = [localized("history.compare.batch", index + 1) + ": " + entry.stamp,
                         localized(entry.statusKey) + " · " + timestampLabel(entry.timestamp),
                         row("mode", entry.modeLabel ?? unknown), row("version", entry.context?.version ?? unknown)]
            for (key, media) in [("img", entry.summary?.img), ("vid", entry.summary?.vid)] {
                let title = localized("history.media.\(key)") + ": "
                if media?.active == false { lines.append(title + localized("history.not_run")); continue }
                lines += [title + localized("result.counts", count(media?.succeeded), count(media?.skipped),
                                             count(media?.failed), count(media?.ignored)),
                          row("exit", count(media?.exit_code)) + " · " + localized("result.unprocessed", count(media?.unprocessed))]
            }
            lines += [row("count_status", entry.verification?.count_status ?? unknown),
                      row("integrity", entry.summary?.integrity.state ?? unknown)
                        + " · " + row("issues", count(entry.summary?.integrity.issue_count))]
            if let verification = entry.verification {
                lines.append(row("verification_state", verification.has_warnings || verification.issue_count > 0 || verification.count_status?.hasPrefix("MISMATCH") == true ? "WARNINGS" : "CLEAN")
                    + " · " + row("issues", String(verification.issue_count)))
            } else { lines.append(row("verification_state", unknown)) }
            if entry.isRestoreJpeg {
                lines.append(entry.sizeOutcome)
                if entry.summary?.sizeInvalid == true { lines.append(localized("history.space.invalid")) }
            } else if let size = entry.summary?.size {
                lines += [entry.sizeOutcome,
                          row("input_bytes", size.input_bytes.map(HistoryEntry.sizeLabel) ?? unknown),
                          row("output_bytes", size.output_bytes.map(HistoryEntry.sizeLabel) ?? unknown), localized(size.scopeKey)]
            } else { lines.append(localized(entry.summary?.sizeInvalid == true ? "history.space.invalid" : "history.space.not_recorded")) }
            let sources = entry.context?.inputs ?? []
            lines += [row("sources", sources.isEmpty ? unknown : sources.joined(separator: "\n")),
                      row("output", entry.outputPaths ?? unknown)]
            for version in Set(entry.configSnapshots.map { $0.pipeline + ": " + $0.packageVersion }).sorted() {
                lines.append(row("config_package_version", version))
            }
            if let error = entry.finished?.error { lines.append(row("error", error)) }
            if !entry.issues.isEmpty { lines += entry.issues }
            sections.append(lines.joined(separator: "\n"))
        }
        sections.append(renderConfigurationComparison(entries))
        return sections.joined(separator: "\n\n")
    }

    private static func renderConfigurationComparison(_ entries: [HistoryEntry]) -> String {
        var lines = [localized("history.compare.config")]
        let pipelines = ["img", "vid"].filter { pipeline in
            entries.contains { entry in
                entry.configSnapshots.contains { $0.pipeline == pipeline }
                    || (pipeline == "img" ? entry.summary?.img.active : entry.summary?.vid.active) == true
            }
        }
        if pipelines.isEmpty { return (lines + [localized("history.config.not_recorded")]).joined(separator: "\n") }
        let missing = localized("history.compare.config.missing")
        for pipeline in pipelines {
            let snapshots = entries.map { $0.configSnapshots.filter { $0.pipeline == pipeline } }
            lines += ["", localized("history.media.\(pipeline)")]
            guard entries.allSatisfy({ !$0.configEvidenceInvalid && $0.issues.isEmpty }),
                  snapshots.allSatisfy({ $0.count == 1 }),
                  let left = try? snapshots[0][0].comparisonFields(),
                  let right = try? snapshots[1][0].comparisonFields()
            else {
                lines += [localized("history.compare.config.unavailable"),
                          localized("history.compare.config.snapshots", snapshots[0].count, snapshots[1].count)]
                continue
            }
            for (key, first, second) in [("values", left.values, right.values), ("sources", left.sources, right.sources)] {
                let changed = Set(first.keys).union(second.keys).filter { first[$0] != second[$0] }.sorted()
                if changed.isEmpty {
                    lines.append(localized("history.compare.config.\(key).same"))
                } else {
                    lines.append(localized("history.compare.config.\(key)"))
                    for field in changed {
                        lines.append("  " + localized("history.compare.config.field", field, first[field] ?? missing, second[field] ?? missing))
                    }
                }
            }
        }
        return lines.joined(separator: "\n")
    }

    fileprivate static func render(_ entry: HistoryEntry) -> String {
        let unknown = localized("result.unknown")
        func count(_ value: Int?) -> String { value.map(String.init) ?? unknown }
        func row(_ key: String, _ value: String) -> String { localized("history.detail.\(key)") + ": " + value }
        func verificationRows(_ verification: HistoryVerification) -> [String] {
            [row("verification_state", verification.has_warnings || verification.issue_count > 0 || verification.count_status?.hasPrefix("MISMATCH") == true ? "WARNINGS" : "CLEAN"),
             row("count_status", verification.count_status ?? unknown),
             row("verified_source", verification.source_path ?? unknown),
             row("issues", String(verification.issue_count)),
             row("source_inventory", String(verification.source_count)),
             row(entry.isRestoreJpeg ? "restored_inventory" : "optimized_inventory", String(verification.optimized_count)),
             row("skipped_inventory", String(verification.skipped_count)),
             row("failed_inventory", String(verification.failed_count)),
             row("remaining_inventory", String(verification.source_remaining_count)),
             row("deleted_inventory", String(verification.verified_deleted_count))]
        }
        var lines = [localized(entry.statusKey), Self.timestampLabel(entry.timestamp), ""]
        if entry.isRestoreJpeg {
            lines.append(entry.sizeOutcome)
            if entry.summary?.sizeInvalid == true { lines.append(localized("history.space.invalid")) }
        } else if let size = entry.summary?.size {
            lines.append(localized("history.space.title"))
            lines += [entry.sizeOutcome,
                      row("input_bytes", size.input_bytes.map(HistoryEntry.sizeLabel) ?? unknown),
                      row("output_bytes", size.output_bytes.map(HistoryEntry.sizeLabel) ?? unknown),
                      localized(size.scopeKey), localized("history.space.note")]
        } else if entry.summary?.sizeInvalid == true {
            lines.append(localized("history.space.title"))
            lines.append(localized("history.space.invalid"))
        } else {
            lines.append(localized("history.space.title"))
            lines.append(localized("history.space.not_recorded"))
        }
        lines += ["",
                     row("mode", entry.modeLabel ?? unknown),
                     row("sources", entry.context?.inputs.isEmpty == false ? entry.context!.inputs.joined(separator: "\n") : unknown),
                     row("output", entry.outputPaths ?? unknown),
                     row("backup", entry.context?.backup ?? unknown)]
        if let context = entry.context {
            lines += [row("dry_run", localized(context.dry_run ? "settings.value.true" : "settings.value.false")),
                      row("in_place", localized(context.in_place ? "settings.value.true" : "settings.value.false")),
                      row("shortest_path", localized(context.shortest_path ? "settings.value.true" : "settings.value.false"))]
            lines.append(row("version", context.version ?? unknown))
            for (key, value) in [("retry", context.retry), ("resume", context.resume), ("no_resume", context.no_resume)] {
                lines.append(row(key, value.map { localized($0 ? "settings.value.true" : "settings.value.false") } ?? unknown))
            }
        }
        lines += ["", localized("history.effective_config")]
        if entry.configSnapshots.isEmpty {
            lines.append(localized(entry.configMarkerSeen ? "history.config.invalid" : "history.config.not_recorded"))
        } else {
            lines.append(row("config_invocations", String(entry.configInvocationCount)))
            for (index, snapshot) in entry.configSnapshots.enumerated() {
                lines += ["", row("config_snapshot", String(index + 1)),
                          row("config_pipeline", snapshot.pipeline),
                          row("config_package_version", snapshot.packageVersion),
                          row("config_values", snapshot.values),
                          row("config_sources", snapshot.sources)]
            }
        }
        lines += ["", localized("history.worker.title"), localized("history.worker.note")]
        if entry.invocationEvidenceInvalid {
            lines.append(localized("history.worker.invalid"))
        } else if entry.invocations.isEmpty {
            lines.append(localized("history.worker.not_recorded"))
        } else {
            for invocation in entry.invocations {
                lines += ["", row("invocation_id", invocation.invocation_id), row("config_pipeline", invocation.pipeline),
                    row("worker_program", invocation.program_requested), row("worker_path", invocation.program_path ?? unknown)]
                let arguments = try? JSONSerialization.data(withJSONObject: invocation.arguments, options: [.prettyPrinted, .sortedKeys])
                lines.append(row("worker_arguments", arguments.map { String(decoding: $0, as: UTF8.self) } ?? unknown))
                if let bytes = invocation.binary.bytes, let hash = invocation.binary.blake3 {
                    lines.append(row("worker_binary", "\(bytes) B · \(hash)"))
                } else { lines.append(row("worker_observation_error", invocation.binary.diagnostic ?? unknown)) }
                lines.append(row("config_package_version", entry.invocationConfigs[invocation.invocation_id]?.packageVersion ?? unknown))
            }
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
            lines += verificationRows(verification)
        } else { lines += [row("verification_state", unknown), row("count_status", unknown)] }
        if let inputPlan = entry.inputPlan {
            for input in inputPlan.inputs {
                lines += ["", row("output", input.output ?? unknown)]
                if let receipt = entry.inputVerifications[input.source] {
                    lines += verificationRows(receipt)
                } else {
                    lines += [row("verified_source", input.source), row("count_status", unknown)]
                }
            }
        }
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
        let ready = refreshButton.isEnabled && !exportBusy
        let single = ready && selected != nil
        table.isEnabled = ready
        search.isEnabled = ready
        filter.isEnabled = ready
        let failuresVisible = single && detailMode.selectedSegment == 2
        failedContent.isHidden = !failuresVisible
        detailScroll.isHidden = failuresVisible
        outputNavigation.isHidden = detailMode.selectedSegment != 1
        failedSearch.isEnabled = failuresVisible
        failedTable.isEnabled = failuresVisible
        failedCopyButton.isEnabled = failuresVisible && selectedFailedPath != nil
        failedRevealButton.isEnabled = failuresVisible && selectedFailedURL != nil
        logButton.isEnabled = single && !outputLoading && (selected?.hasStoredOutput == true || selected?.logs.isEmpty == false)
        detailMode.setEnabled(single && selected?.hasStoredOutput == true, forSegment: 1)
        detailMode.setEnabled(single, forSegment: 2)
        detailMode.setEnabled(single && selected?.hasStoredOutput == true, forSegment: 3)
        olderButton.isEnabled = single && detailMode.selectedSegment == 1 && !outputLoading && outputPage?.hasOlder == true
        newerButton.isEnabled = single && detailMode.selectedSegment == 1 && !outputLoading && outputPage?.hasNewer == true
        outputFilter.isEnabled = single && detailMode.selectedSegment == 1 && !outputLoading && outputPage != nil
        revealButton.isEnabled = single
        copyButton.isEnabled = ready && selectedEntries.contains { $0.context?.inputs.isEmpty == false }
        exportButton.isEnabled = single && !outputLoading && issueScanControl == nil && selected?.hasStoredOutput == true
    }

    func windowWillClose(_ notification: Notification) {
        cancelIssueScan()
        exportGeneration = UUID()
        if exportBusy { finishExport(localized("history.export.cancelled")) }
    }

    private func finishExport(_ message: String) {
        exportBusy = false
        refreshButton.isEnabled = true
        progress.stopAnimation(nil)
        status.stringValue = message
        updateActions()
    }

    @objc private func exportSelectedRun() {
        guard exportButton.isEnabled, let entry = selected, let window, window.isVisible else { return }
        let token = UUID()
        exportGeneration = token
        exportBusy = true
        refreshButton.isEnabled = false
        updateActions()
        let chooser = NSSavePanel()
        chooser.title = localized("history.export.title")
        chooser.message = localized("history.export.scope")
        chooser.allowedContentTypes = [.json]
        chooser.isExtensionHidden = false
        chooser.nameFieldStringValue = "MFB-diagnostics-" + UUID().uuidString + ".json"
        chooser.beginSheetModal(for: window) { [weak self] response in
            guard let self, self.exportGeneration == token else { return }
            guard response == .OK, let destination = chooser.url else {
                self.finishExport(localized("history.export.cancelled")); return
            }
            guard self.window?.isVisible == true, self.selected?.id == entry.id else {
                self.finishExport(localized("history.export.cancelled")); return
            }
            self.status.stringValue = localized("history.export.running")
            self.progress.startAnimation(nil)
            DispatchQueue.global(qos: .utility).async { [weak self] in
                let result = Swift.Result {
                    try HistoryDiagnosticStaging.prepare(HistoryStore.diagnosticArtifact(for: entry), destination: destination)
                }
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.exportGeneration == token, self.window?.isVisible == true,
                          self.selected?.id == entry.id else {
                        if case let .success(staging) = result {
                            do { try staging.discard() }
                            catch { fputs("MFB: cancelled diagnostic export cleanup failed: \(error.localizedDescription)\n", stderr) }
                        }
                        if let self, self.exportGeneration == token { self.finishExport(localized("history.export.cancelled")) }
                        return
                    }
                    do {
                        // Publication stays on the main queue so a processed close/selection change cannot race it.
                        try result.get().publish()
                        self.finishExport(localized("history.export.completed", destination.path))
                    } catch { self.finishExport(error.localizedDescription) }
                }
            }
        }
    }
    @objc private func openLog() {
        if let entry = selected, entry.hasStoredOutput {
            cancelIssueScan()
            detailMode.selectedSegment = 1
            updateActions()
            loadOutput(entry: entry)
            return
        }
        guard let url = selected?.logs.first, HistoryStore.regularFile(url), NSWorkspace.shared.open(url) else {
            status.stringValue = localized("history.issue.open_log"); return
        }
    }

    private func resetOutput() {
        cancelIssueScan()
        batchIssueResult = nil
        outputGeneration = UUID()
        outputLoading = false
        outputPage = nil
        outputFilter.selectedSegment = 0
        detailMode.selectedSegment = 0
    }

    @objc private func detailModeChanged() {
        guard selected != nil else { showSelected(); return }
        outputGeneration = UUID()
        outputLoading = false
        if detailMode.selectedSegment != 3 { cancelIssueScan() }
        if detailMode.selectedSegment == 0 {
            detail.string = selected.map { Self.render($0) } ?? localized("history.select")
            updateHistoryStatus()
        }
        else if detailMode.selectedSegment == 2 { filterFailedFiles(); updateHistoryStatus() }
        else if detailMode.selectedSegment == 3 {
            if batchIssueResult != nil { renderBatchIssues() }
            else if let entry = selected { loadBatchIssues(entry: entry) }
        }
        else if outputPage != nil { renderOutput() }
        else if let entry = selected { loadOutput(entry: entry) }
        updateActions()
    }

    private var selectedFailedPath: String? {
        guard selected != nil, failedPaths.indices.contains(failedTable.selectedRow) else { return nil }
        return failedPaths[failedTable.selectedRow]
    }

    private var selectedFailedURL: URL? {
        guard let path = selectedFailedPath, path.hasPrefix("/"), !path.utf8.contains(0) else { return nil }
        let url = URL(fileURLWithPath: path)
        return HistoryStore.regularFile(url) ? url : nil
    }

    private static func failureRows(_ paths: [String], query: String) -> (paths: [String], matches: Int) {
        var shown: [String] = []
        var matches = 0
        var bytes = 0
        for path in paths where query.isEmpty || path.localizedCaseInsensitiveContains(query) {
            matches += 1
            let length = path.utf8.count
            if shown.count < HistoryStore.maximumOutputLines, length <= HistoryStore.maximumOutputBytes - bytes {
                shown.append(path)
                bytes += length
            }
        }
        return (shown, matches)
    }

    private func filterFailedFiles() {
        let paths = selected?.summary?.failed_files ?? []
        let query = failedSearch.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let rows = Self.failureRows(paths, query: query)
        failedPaths = rows.paths
        failedTable.deselectAll(nil)
        failedTable.reloadData()
        failedStatus.stringValue = (paths.isEmpty ? localized("history.failed_files.empty")
            : localized("history.failed_files.range", failedPaths.count, rows.matches, paths.count))
            + "\n" + localized("history.failed_files.note")
        updateActions()
    }

    @objc private func copyFailedPath() {
        guard detailMode.selectedSegment == 2, refreshButton.isEnabled, let path = selectedFailedPath else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(path, forType: .string)
        status.stringValue = localized("history.copied")
    }

    @objc private func revealFailedPath() {
        guard detailMode.selectedSegment == 2, refreshButton.isEnabled, let url = selectedFailedURL else {
            status.stringValue = localized("history.failed_files.unavailable"); return
        }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }
    @objc private func outputFilterChanged() {
        outputGeneration = UUID()
        outputLoading = false
        renderOutput()
        updateActions()
    }
    @objc private func olderOutput() {
        guard let entry = selected, let page = outputPage, page.hasOlder else { return }
        loadOutput(entry: entry, cursor: page.rows.first?.sequence, direction: .older, upper: page.upper)
    }
    @objc private func newerOutput() {
        guard let entry = selected, let page = outputPage, page.hasNewer else { return }
        loadOutput(entry: entry, cursor: page.rows.last?.sequence, direction: .newer, upper: page.upper)
    }
    private func renderOutput() {
        guard let page = outputPage else { return }
        let end = page.shown == 0 ? 0 : page.firstOrdinal + Int64(page.shown) - 1
        let label = localized("history.output.range", page.firstOrdinal, end, page.total)
        if outputFilter.selectedSegment == 1 {
            let groups = page.diagnosticGroups
            let count = groups.reduce(0) { $0 + $1.count }
            let sections = groups.map { group in
                localized("history.output.diagnostic_group", group.pipeline ?? localized("result.unknown"),
                    group.phase.map { localized("log.phase.\($0)") } ?? localized("history.output.phase_unknown"),
                    localized("history.output.severity.\(group.severity)"), group.count, group.count - 1,
                    group.firstSequence, group.lastSequence) + "\n" + group.message
            }
            detail.string = label + "\n" + localized("history.output.diagnostic_groups", count, groups.count)
                + "\n" + localized("history.output.diagnostics_note") + "\n\n" + sections.joined(separator: "\n\n")
        } else {
            detail.string = label + "\n" + localized("history.output.visible_page", page.shown, page.shown)
                + "\n\n" + page.rows.map(\.text).joined(separator: "\n")
        }
        detail.scrollToBeginningOfDocument(nil)
        status.stringValue = label
    }
    private func loadOutput(entry: HistoryEntry, cursor: Int64? = nil,
                            direction: HistoryStore.OutputDirection = .older, upper: Int64? = nil) {
        let token = UUID()
        outputGeneration = token
        outputLoading = true
        status.stringValue = localized("history.output.loading")
        updateActions()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Swift.Result { try HistoryStore.output(for: entry, cursor: cursor, direction: direction, upperSnapshot: upper) }
            DispatchQueue.main.async {
                guard let self, self.outputGeneration == token, self.selected?.id == entry.id else { return }
                self.outputLoading = false
                switch result {
                case let .success(output):
                    self.outputPage = output
                    self.renderOutput()
                case let .failure(error):
                    self.outputPage = nil
                    self.outputFilter.selectedSegment = 0
                    self.status.stringValue = error.localizedDescription
                    self.detail.string = error.localizedDescription
                }
                self.updateActions()
            }
        }
    }

    private func cancelIssueScan() {
        issueGeneration = UUID()
        issueScanControl?.cancel()
        issueScanControl = nil
    }

    private func renderBatchIssues() {
        guard let result = batchIssueResult else { return }
        let scope = localized(result.endSource == "snapshot_only" ? "history.batch_issues.scope_snapshot"
            : "history.batch_issues.scope_closed", result.scannedRows, result.start, result.upper)
        let totals = localized("history.batch_issues.totals", result.occurrences, result.groups.count)
        let sections = result.groups.map { group -> String in
            var section = localized("history.output.diagnostic_group", group.pipeline ?? localized("result.unknown"),
                group.phase.map { localized("log.phase.\($0)") } ?? localized("history.batch_issues.phase_unknown"),
                localized("history.output.severity.\(group.severity)"), group.count, group.count - 1,
                group.firstSequence, group.lastSequence) + "\n" + group.message
            section += "\n" + localized("history.batch_issues.original_first") + "\n" + group.firstOriginal
            if group.lastSequence != group.firstSequence {
                section += "\n" + localized("history.batch_issues.original_last") + "\n" + group.lastOriginal
            }
            return section
        }
        detail.string = [localized("history.batch_issues.title"), scope, totals,
            localized("history.batch_issues.note"), "", sections.isEmpty ? localized("history.batch_issues.empty")
                : sections.joined(separator: "\n\n")].joined(separator: "\n")
        detail.scrollToBeginningOfDocument(nil)
        status.stringValue = totals
    }

    private func loadBatchIssues(entry: HistoryEntry) {
        cancelIssueScan()
        let token = UUID()
        issueGeneration = token
        let control = HistoryStore.IssueScanControl()
        issueScanControl = control
        batchIssueResult = nil
        detail.string = localized("history.batch_issues.loading")
        status.stringValue = detail.string
        updateActions()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Swift.Result { try HistoryStore.batchIssues(for: entry, control: control) }
            DispatchQueue.main.async {
                guard let self, self.issueGeneration == token, self.selected?.id == entry.id,
                      self.detailMode.selectedSegment == 3, !control.isCancelled else { return }
                self.issueScanControl = nil
                switch result {
                case let .success(issues):
                    self.batchIssueResult = issues
                    self.renderBatchIssues()
                case let .failure(error):
                    self.batchIssueResult = nil
                    self.detail.string = error.localizedDescription
                    self.status.stringValue = error.localizedDescription
                }
                self.updateActions()
            }
        }
    }
    @objc private func revealAudit() {
        guard let url = selected?.audit, HistoryStore.regularFile(url) else {
            status.stringValue = localized("history.issue.read"); return
        }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }
    @objc private func copySources() {
        var seen = Set<String>()
        let paths = selectedEntries.flatMap { $0.context?.inputs ?? [] }.filter { seen.insert($0).inserted }
        guard !paths.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(paths.joined(separator: "\n"), forType: .string)
        status.stringValue = localized("history.copied")
    }

    func validateForSelfTest() throws {
        let resultCell = HistoryResultCell()
        let resultLabel = NSTextField(labelWithString: "synthetic partial success")
        resultCell.addSubview(resultLabel)
        resultCell.textField = resultLabel
        resultCell.resultColor = .systemOrange
        resultCell.backgroundStyle = .emphasized
        guard resultCell.textField?.textColor == .labelColor else {
            throw HostError(message: "Selected history result retained a low-contrast custom color")
        }
        resultCell.backgroundStyle = .normal
        guard resultCell.textField?.textColor == .systemOrange else {
            throw HostError(message: "Unselected history result lost its semantic color")
        }
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
              detailMode.segmentCount == 4, !detailMode.isEnabled(forSegment: 2), !detailMode.isEnabled(forSegment: 3),
               failedContent.isHidden, !failedCopyButton.isEnabled, !failedRevealButton.isEnabled,
               table.allowsMultipleSelection, table.allowsEmptySelection,
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
        detailMode.selectedSegment = 2
        detailModeChanged()
        guard failedPaths.isEmpty, failedStatus.stringValue.contains(localized("history.failed_files.empty")),
              selected?.summary == nil, !failedCopyButton.isEnabled, !failedRevealButton.isEnabled else {
            throw HostError(message: "Missing failure-path evidence became actionable or proved zero failures")
        }
        detailMode.selectedSegment = 0
        detailModeChanged()
        search.stringValue = "absent synthetic search"
        applyFilter()
        guard table.numberOfRows == 0, !revealButton.isEnabled else {
            throw HostError(message: "History search retained a stale selected action")
        }
        search.stringValue = ""
        filter.selectedSegment = 1
        applyFilter()
        guard table.numberOfRows == 1 else { throw HostError(message: "Legacy history disappeared from attention filter") }
        let other = HistoryEntry(id: "synthetic:1", stamp: "second synthetic", audit: synthetic.audit,
                                 timestamp: "synthetic", legacy: true)
        let third = HistoryEntry(id: "synthetic:2", stamp: "third synthetic", audit: synthetic.audit,
                                 timestamp: "synthetic", legacy: true)
        entries = [synthetic, other, third]
        applyFilter()
        table.selectRowIndexes(IndexSet(integersIn: 0...1), byExtendingSelection: false)
        showSelected()
        guard selected == nil, selectedEntries.count == 2, !logButton.isEnabled, !revealButton.isEnabled,
              !detailMode.isEnabled(forSegment: 1), !detailMode.isEnabled(forSegment: 2), !detailMode.isEnabled(forSegment: 3),
              !olderButton.isEnabled, !newerButton.isEnabled,
              !outputFilter.isEnabled, detail.string.contains(localized("history.compare")),
              detail.string.contains(other.stamp) else { throw HostError(message: "History comparison retained single-run actions") }
        let selectedIDs = Set(selectedEntries.map(\.id))
        entries.reverse()
        applyFilter()
        guard Set(selectedEntries.map(\.id)) == selectedIDs else { throw HostError(message: "History filtering compared different run identities") }
        search.stringValue = other.stamp
        applyFilter()
        guard selected?.id == other.id else { throw HostError(message: "History filtering lost the remaining selected run") }
        search.stringValue = ""
        applyFilter()
        table.selectRowIndexes(IndexSet(integersIn: 0...2), byExtendingSelection: false)
        showSelected()
        guard detail.string == localized("history.compare.select_two"), selected == nil else {
            throw HostError(message: "History compared an ambiguous selection")
        }
        table.deselectAll(nil)
        showSelected()
        applyFilter()
        guard selectedEntries.isEmpty, !logButton.isEnabled, !copyButton.isEnabled, !revealButton.isEnabled else {
            throw HostError(message: "Cleared history selection silently selected another batch")
        }
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent("mfb-history-files-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let existing = temporary.appendingPathComponent("synthetic retained.txt")
        let missing = temporary.appendingPathComponent("synthetic missing.txt")
        try Data().write(to: existing)
        var fileEntry = synthetic
        fileEntry.summary = HistorySummary(schema_version: 1, count_scope: "processor_outcomes",
            img: HistoryMedia(active: true, succeeded: 1, skipped: 0, ignored: 0, failed: 2, unprocessed: 0, exit_code: 1),
            vid: HistoryMedia(active: false, succeeded: nil, skipped: nil, ignored: nil, failed: nil, unprocessed: nil, exit_code: nil),
            integrity: HistorySummary.Integrity(state: nil, issue_count: nil),
            failed_files: [existing.path, missing.path, "relative/synthetic.txt"], skipped_files: [])
        entries = [fileEntry]
        applyFilter()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        detailMode.selectedSegment = 2
        detailModeChanged()
        guard !failedContent.isHidden, detailScroll.isHidden, failedTable.numberOfRows == 3,
              selected?.summary?.img.failed == 2, !failedCopyButton.isEnabled, !failedRevealButton.isEnabled else {
            throw HostError(message: "Recorded failure paths invented counts or selected actions")
        }
        failedTable.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        updateActions()
        guard selectedFailedPath == existing.path, selectedFailedURL?.path == existing.path,
              failedCopyButton.isEnabled, failedRevealButton.isEnabled else {
            throw HostError(message: "Recorded existing path was not exactly selectable")
        }
        for row in [1, 2] {
            failedTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            updateActions()
            guard failedCopyButton.isEnabled, !failedRevealButton.isEnabled, selectedFailedURL == nil else {
                throw HostError(message: "Missing or relative failure path enabled Finder navigation")
            }
        }
        failedSearch.stringValue = "RETAINED"
        filterFailedFiles()
        guard failedPaths == [existing.path], !failedCopyButton.isEnabled,
              failedStatus.stringValue.contains(localized("history.failed_files.range", 1, 1, 3)) else {
            throw HostError(message: "Failure path search changed totals or retained stale actions")
        }
        let bounded = Self.failureRows(Array(repeating: existing.path, count: HistoryStore.maximumOutputLines + 1), query: "")
        let oversized = Self.failureRows([String(repeating: "x", count: HistoryStore.maximumOutputBytes + 1), existing.path], query: "")
        guard bounded.matches == HistoryStore.maximumOutputLines + 1, bounded.paths.count == HistoryStore.maximumOutputLines,
              oversized.matches == 2, oversized.paths == [existing.path] else {
            throw HostError(message: "Failure path bounds lost matching totals or exact paths")
        }
        entries = [fileEntry, other]
        applyFilter()
        table.selectRowIndexes(IndexSet(integersIn: 0...1), byExtendingSelection: false)
        showSelected()
        guard failedContent.isHidden, failedPaths.isEmpty, !failedCopyButton.isEnabled, !failedRevealButton.isEnabled else {
            throw HostError(message: "Comparison retained prior failure-file navigation")
        }
        let original = window!.frame
        window?.setContentSize(NSSize(width: 780, height: 440))
        content.layoutSubtreeIfNeeded()
        guard search.frame.width >= 150, split.frame.height >= 230,
              split.frame.maxX <= content.bounds.maxX else {
            throw HostError(message: "History controls clipped at minimum size")
        }
        let detailBounds = split.arrangedSubviews[1].bounds
        for control in [detailMode, outputFilter, olderButton, newerButton] as [NSView] {
            let rect = control.convert(control.bounds, to: split.arrangedSubviews[1])
            guard rect.minX >= -1, rect.maxX <= detailBounds.maxX + 1,
                  rect.minY >= -1, rect.maxY <= detailBounds.maxY + 1 else {
                throw HostError(message: "History output toolbar clipped at minimum size")
            }
        }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        detailMode.selectedSegment = 2
        detailModeChanged()
        content.layoutSubtreeIfNeeded()
        for control in [detailMode, failedSearch, failedCopyButton, failedRevealButton, failedStatus, failedContent] as [NSView] {
            let rect = control.convert(control.alignmentRect(forFrame: control.bounds), to: split.arrangedSubviews[1])
            let bounds = split.arrangedSubviews[1].bounds
            guard rect.minX >= -1, rect.maxX <= bounds.maxX + 1,
                  rect.minY >= -1, rect.maxY <= bounds.maxY + 1 else {
                throw HostError(message: "History failure-file controls clipped at minimum size: \(type(of: control)) \(rect), pane \(split.arrangedSubviews[1].bounds)")
            }
        }
        table.deselectAll(nil)
        showSelected()
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
        if displayed.count >= 2 {
            table.selectRowIndexes(IndexSet(integersIn: 0...1), byExtendingSelection: false)
            showSelected()
            let selectedIDs = Set(selectedEntries.map(\.id))
            refresh(directory: directory)
            let refreshDeadline = Date().addingTimeInterval(5)
            while !refreshButton.isEnabled, Date() < refreshDeadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01))
            }
            guard refreshButton.isEnabled, Set(selectedEntries.map(\.id)) == selectedIDs, selected == nil,
                  !logButton.isEnabled, detail.string.contains(localized("history.compare")) else {
                throw HostError(message: "History refresh lost or replaced the compared batches")
            }
        }
        filter.selectedSegment = 1
        applyFilter()
        guard displayed.allSatisfy(\.needsAttention), displayed.count == entries.filter(\.needsAttention).count else {
            throw HostError(message: "Database history attention filter lost counted states")
        }
    }

    func validateOutputForSelfTest() throws {
        entries = try HistoryStore.load(directory: directory).entries.filter { $0.stamp == "output" }
            .sorted { ($0.context?.output_sequence ?? -1) < ($1.context?.output_sequence ?? -1) }
        applyFilter()
        guard entries.count == 2, logButton.isEnabled, exportButton.isEnabled else { throw HostError(message: "Stored output or diagnostic export action is unavailable") }
        let oldExportGeneration = exportGeneration
        exportBusy = true
        refreshButton.isEnabled = false
        updateActions()
        guard !exportButton.isEnabled, !table.isEnabled, !copyButton.isEnabled else {
            throw HostError(message: "Diagnostic export did not disable conflicting actions")
        }
        windowWillClose(Notification(name: NSWindow.willCloseNotification, object: window))
        guard !exportBusy, exportGeneration != oldExportGeneration, exportButton.isEnabled else {
            throw HostError(message: "Closing history did not cancel diagnostic publication or restore its controls")
        }
        openLog()
        let deadline = Date().addingTimeInterval(5)
        while !logButton.isEnabled, Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        guard logButton.isEnabled, detail.string.contains("synthetic first failure"), !detail.string.contains("synthetic next run") else {
            throw HostError(message: "History output action did not display the selected batch")
        }
        openLog()
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        showSelected()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        guard !detail.string.contains("synthetic first failure") else { throw HostError(message: "Stale output replaced a new selection") }
        openLog()
        let nextDeadline = Date().addingTimeInterval(5)
        while !logButton.isEnabled, Date() < nextDeadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        guard logButton.isEnabled, detail.string.contains("synthetic next run 1002"), !detail.string.contains("synthetic first failure") else {
            throw HostError(message: "History output action lost its bounded latest records")
        }
        detailMode.selectedSegment = 0
        detailModeChanged()
        guard status.stringValue == localized("history.shown", displayed.count, entries.count) else {
            throw HostError(message: "Summary retained output-page status")
        }
        detailMode.selectedSegment = 1
        detailModeChanged()
        let invalid = HistoryEntry(id: selected!.id, stamp: "output", audit: directory.appendingPathComponent("absent/history.sqlite3"),
                                   timestamp: "synthetic", context: selected!.context)
        loadOutput(entry: invalid)
        let errorDeadline = Date().addingTimeInterval(5)
        while outputLoading, Date() < errorDeadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        guard outputPage == nil, !olderButton.isEnabled, !newerButton.isEnabled, logButton.isEnabled else {
            throw HostError(message: "Output failure retained stale navigation or disabled recovery")
        }
        openLog()
        let retryDeadline = Date().addingTimeInterval(5)
        while outputLoading, Date() < retryDeadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        guard outputPage != nil else { throw HostError(message: "Output failure could not be retried") }
        outputFilter.selectedSegment = 1
        outputFilterChanged()
        guard !detail.string.contains("synthetic next run"), outputPage?.shown == HistoryStore.maximumOutputLines else {
            throw HostError(message: "Page diagnostics changed pagination or exposed routine records")
        }
        let previousPage = outputPage
        let phase = "MFB_LOG_PHASE={\"schema_version\":1,\"phase\":\"processing\"}"
        let repeated = "[WARN] /synthetic/file.jpg: repeated warning 1"
        let groupedRows = [
            HistoryStore.Output.Row(sequence: 1, text: phase, diagnostic: false, pipeline: "IMG", line: phase),
            HistoryStore.Output.Row(sequence: 2, text: repeated, diagnostic: true, pipeline: "IMG", line: repeated),
            HistoryStore.Output.Row(sequence: 3, text: "ERR: " + repeated, diagnostic: true, pipeline: "IMG", line: "ERR: " + repeated),
        ]
        outputPage = HistoryStore.Output(rows: groupedRows, total: 100, upper: 100, firstOrdinal: 1, issues: [])
        outputFilterChanged()
        guard detail.string.contains(localized("history.output.diagnostic_groups", 2, 1)),
              detail.string.contains(localized("history.output.diagnostics_note")),
              detail.string.contains(localized("log.phase.processing")),
              !detail.string.contains("ERR: " + repeated), outputPage?.total == 100 else {
            throw HostError(message: "Diagnostic group presentation repeated logs or claimed whole-batch failure counts")
        }
        outputFilter.selectedSegment = 0
        outputFilterChanged()
        guard detail.string.contains(phase), detail.string.contains("ERR: " + repeated), outputPage?.shown == 3 else {
            throw HostError(message: "All output did not retain the original diagnostic and phase records")
        }
        outputPage = previousPage
        outputFilter.selectedSegment = 1
        outputFilterChanged()
        olderOutput()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        showSelected()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        guard outputFilter.selectedSegment == 0, detailMode.selectedSegment == 0, outputPage == nil,
              status.stringValue == localized("history.shown", displayed.count, entries.count),
              !detail.string.contains("synthetic next run") else {
            throw HostError(message: "Selection did not reset page filtering or reject stale navigation")
        }
        openLog()
        applyFilter()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        guard outputPage == nil, detailMode.selectedSegment == 0 else {
            throw HostError(message: "History filtering accepted stale output")
        }
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        showSelected()
        detailMode.selectedSegment = 3
        detailModeChanged()
        let issueDeadline = Date().addingTimeInterval(5)
        while issueScanControl != nil, Date() < issueDeadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        guard batchIssueResult?.occurrences == 999, batchIssueResult?.scannedRows == 1_002,
              detail.string.contains(localized("history.batch_issues.totals", 999, 3)),
              detail.string.contains(localized("history.batch_issues.original_first")),
              detail.string.contains("ERR: \u{001B}[33m"), outputNavigation.isHidden,
              !olderButton.isEnabled, !newerButton.isEnabled, exportButton.isEnabled else {
            throw HostError(message: "Whole-batch issue view guessed file counts, lost raw evidence, or retained page navigation")
        }
        let batchEntry = selected!
        loadBatchIssues(entry: batchEntry)
        let selectionControl = issueScanControl!
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        showSelected()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        guard selectionControl.isCancelled, issueScanControl == nil, batchIssueResult == nil,
              detailMode.selectedSegment == 0, !detail.string.contains(localized("history.batch_issues.title")) else {
            throw HostError(message: "Changing selection did not cancel the issue reader or reject stale publication")
        }
        detailMode.selectedSegment = 3
        loadBatchIssues(entry: selected!)
        let closedControl = issueScanControl!
        windowWillClose(Notification(name: NSWindow.willCloseNotification, object: window))
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        guard closedControl.isCancelled, issueScanControl == nil, batchIssueResult == nil else {
            throw HostError(message: "Closing History left an issue reader or published cancelled counts")
        }
        resetOutput()
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
    let cancelled = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "cancelled", "error": NSNull()])
    func parsed(_ events: [String]) throws -> HistoryAudit { HistoryStore.parse(data: try records(events), stamp: "test", audit: fixtureURL) }
    func require(_ condition: Bool, _ message: String) throws { if !condition { throw HostError(message: message) } }
    func diagnosticRow(_ sequence: Int64, _ pipeline: String, _ line: String) -> HistoryStore.Output.Row {
        let tone = LogTone.classify(line)
        return HistoryStore.Output.Row(sequence: sequence, text: "[synthetic] \(pipeline): \(line)",
            diagnostic: tone == .warning || tone == .failure, pipeline: pipeline, line: line)
    }
    let processingPhase = try payload("MFB_LOG_PHASE", ["schema_version": 1, "phase": "processing"])
    let verificationPhase = try payload("MFB_LOG_PHASE", ["schema_version": 1, "phase": "verification"])
    let diagnosticRows = [
        diagnosticRow(1, "IMG", "[WARN] /synthetic/first.jpg: warning 1"),
        diagnosticRow(2, "IMG", processingPhase),
        diagnosticRow(3, "IMG", "[FAIL] /synthetic/one.jpg: error 1"),
        diagnosticRow(4, "IMG", "ERR: \u{001B}[31m[FAIL] /synthetic/one.jpg: error 1\u{001B}[0m"),
        diagnosticRow(5, "IMG", "[FAIL] /synthetic/two.jpg: error 1"),
        diagnosticRow(6, "VID", "[FAIL] /synthetic/one.jpg: error 1"),
        diagnosticRow(7, "IMG", "[FAIL] /synthetic/one.jpg: error 2"),
        diagnosticRow(8, "IMG", verificationPhase),
        diagnosticRow(9, "IMG", "[FAIL] /synthetic/one.jpg: error 1"),
        diagnosticRow(10, "IMG", "MFB_LOG_PHASE={invalid}"),
        diagnosticRow(11, "IMG", "[FAIL] /synthetic/one.jpg: error 1"),
        HistoryStore.Output.Row(sequence: 12, text: "synthetic unreadable record", diagnostic: true, pipeline: nil, line: nil),
    ]
    let diagnosticPage = HistoryStore.Output(rows: diagnosticRows, total: 100, upper: 100, firstOrdinal: 1, issues: [])
    let groups = diagnosticPage.diagnosticGroups
    try require(groups.count == 9 && groups.reduce(0, { $0 + $1.count }) == 10
                && groups[1].pipeline == "IMG" && groups[1].phase == "processing" && groups[1].count == 2
                && groups[1].firstSequence == 3 && groups[1].lastSequence == 4
                && groups[2].message.contains("two.jpg") && groups[3].pipeline == "VID" && groups[3].phase == nil
                && groups[4].message.contains("error 2") && groups[5].phase == "verification"
                && groups[6].severity == "warning" && groups[6].phase == nil && groups[6].message.contains("{invalid}")
                && groups[7].phase == nil && groups[8].severity == "unreadable",
                "Page diagnostic grouping merged paths, numbers, pipelines, phases, or hid invalid evidence")
    try require(diagnosticPage.total == 100 && diagnosticPage.shown == 12
                && diagnosticPage.text == diagnosticRows.map(\.text).joined(separator: "\n"),
                "Diagnostic grouping changed authoritative pagination or discarded original output")
    let laterPage = HistoryStore.Output(rows: [diagnosticRows[8]], total: 100, upper: 100, firstOrdinal: 9, issues: [])
    try require(laterPage.diagnosticGroups.first?.phase == nil && laterPage.diagnosticGroups.first?.count == 1,
                "A new page guessed its phase or carried diagnostic counts from another page")
    let success = try parsed(["SESSION_STARTED", context, summary, completed])
    try require(success.entries.count == 1 && success.entries[0].statusKey == "history.status.completed"
                && success.entries[0].context?.output == nil && success.entries[0].succeededLabel == "2", "Typed history did not retain nullable paths or active counts")
    try require(success.entries[0].configSnapshots.isEmpty && !success.entries[0].configMarkerSeen,
                "Old history invented an effective configuration")
    try require(success.entries[0].summary?.size == nil && success.entries[0].sizeOutcome == localized("result.unknown"),
                "Old history invented a size comparison")
    var sizedSummary = summaryValue
    sizedSummary["size"] = ["scope": "fast_img_converted_this_run", "input_bytes": 1_024, "output_bytes": 512]
    let reduced = try parsed([context, try payload("MFB_HISTORY_SUMMARY", sizedSummary), completed]).entries[0]
    try require(reduced.summary?.size?.input_bytes == 1_024 && reduced.summary?.size?.output_bytes == 512
                && reduced.sizeOutcome == localized("history.space.saved", HistoryEntry.sizeLabel(512)),
                "Exact session bytes or reduction disappeared from history")
    sizedSummary["size"] = ["scope": "fast_img_converted_this_run", "input_bytes": 512, "output_bytes": 1_024]
    let expanded = try parsed([context, try payload("MFB_HISTORY_SUMMARY", sizedSummary), failed]).entries[0]
    try require(expanded.statusKey == "history.status.failed"
                && expanded.sizeOutcome == localized("history.space.increased", HistoryEntry.sizeLabel(512)),
                "Expansion was shown as savings or hid the batch failure")
    sizedSummary["size"] = ["scope": "fast_img_converted_this_run", "input_bytes": 512, "output_bytes": NSNull()]
    let partialSize = try parsed([context, try payload("MFB_HISTORY_SUMMARY", sizedSummary), completed]).entries[0]
    try require(partialSize.summary?.size?.input_bytes == 512 && partialSize.sizeOutcome == localized("result.unknown"),
                "Partial size evidence became a claimed saving")
    for invalidSize in [
        ["scope": "directory_total", "input_bytes": 512, "output_bytes": 256],
        ["scope": "fast_img_converted_this_run", "input_bytes": NSNull(), "output_bytes": NSNull()],
        ["scope": "fast_img_converted_this_run", "input_bytes": -1, "output_bytes": 256],
        ["scope": "fast_img_converted_this_run", "input_bytes": UInt64.max, "output_bytes": 256]
    ] as [[String: Any]] {
        sizedSummary["size"] = invalidSize
        let invalidEvent = try payload("MFB_HISTORY_SUMMARY", sizedSummary)
        let invalid = try parsed([context, invalidEvent, completed]).entries[0]
        try require(invalid.summary?.img.succeeded == 2 && invalid.succeededLabel == "2"
                    && invalid.summary?.size == nil && invalid.summary?.sizeInvalid == true
                    && invalid.sizeOutcome == localized("result.unknown")
                    && invalid.issues.contains(localized("history.space.invalid"))
                    && invalid.statusKey == "history.status.incomplete",
                    "Invalid size evidence erased counts or appeared as a saving")
        let failedInvalid = try parsed([context, invalidEvent, failed]).entries[0]
        try require(failedInvalid.statusKey == "history.status.failed" && failedInvalid.succeededLabel == "2",
                    "Invalid size evidence erased a terminal failure or valid counts")
    }
    sizedSummary["size"] = "malformed"
    let malformedSize = try parsed([context, try payload("MFB_HISTORY_SUMMARY", sizedSummary), failed]).entries[0]
    try require(malformedSize.summary?.sizeInvalid == true && malformedSize.statusKey == "history.status.failed"
                && malformedSize.succeededLabel == "2", "Malformed size erased the summary or final failure")
    var fileFailureWithBadSize = sizedSummary
    var failedImage = media
    failedImage["failed"] = 1
    fileFailureWithBadSize["img"] = failedImage
    let fileFailureSize = try parsed([context, try payload("MFB_HISTORY_SUMMARY", fileFailureWithBadSize), completed]).entries[0]
    try require(fileFailureSize.statusKey == "history.status.file_failures" && fileFailureSize.summary?.img.failed == 1,
                "An auxiliary size issue hid a recorded file failure")
    sizedSummary["size"] = ["scope": "fast_img_converted_this_run", "input_bytes": 1_024, "output_bytes": 512]
    var otherSizeSummary = summaryValue
    otherSizeSummary["size"] = ["scope": "fast_img_converted_this_run", "input_bytes": 1_024, "output_bytes": 600]
    let conflictingSize = try parsed([context, try payload("MFB_HISTORY_SUMMARY", sizedSummary),
                                      try payload("MFB_HISTORY_SUMMARY", otherSizeSummary), failed]).entries[0]
    try require(conflictingSize.summary?.sizeInvalid == true && conflictingSize.summary?.size == nil
                && conflictingSize.succeededLabel == "2" && conflictingSize.statusKey == "history.status.failed",
                "Conflicting size evidence erased counts or a terminal failure")
    sizedSummary["size"] = ["scope": "fast_img_converted_this_run", "input_bytes": 512, "output_bytes": 256]
    sizedSummary["img"] = inactive
    let inactiveSize = try parsed([context, try payload("MFB_HISTORY_SUMMARY", sizedSummary), completed]).entries[0]
    try require(inactiveSize.summary?.sizeInvalid == true && inactiveSize.summary?.size == nil
                && inactiveSize.summary?.vid.active == false,
                "Size evidence without an image run changed media outcomes")
    for (image, video) in [(media, inactive), (inactive, media), (media, media)] {
        var ordinarySummary = summaryValue
        ordinarySummary["img"] = image
        ordinarySummary["vid"] = video
        ordinarySummary["size_invalid"] = false
        for (input, output, outcome) in [
            (1_024 as Any, 512 as Any, localized("history.space.saved", HistoryEntry.sizeLabel(512))),
            (512 as Any, 1_024 as Any, localized("history.space.increased", HistoryEntry.sizeLabel(512))),
            (Int64.max as Any, Int64.max as Any, localized("history.space.unchanged")),
            (512 as Any, NSNull() as Any, localized("result.unknown")),
            (NSNull() as Any, 512 as Any, localized("result.unknown")),
            (NSNull() as Any, NSNull() as Any, localized("result.unknown"))
        ] {
            ordinarySummary["size"] = ["scope": "converted_this_run", "input_bytes": input, "output_bytes": output]
            let ordinary = try parsed([context, try payload("MFB_HISTORY_SUMMARY", ordinarySummary), completed]).entries[0]
            try require(ordinary.summary?.size != nil && ordinary.summary?.sizeInvalid == false
                        && ordinary.statusKey == "history.status.completed" && ordinary.sizeOutcome == outcome
                        && ordinary.summary?.size?.scopeKey == "history.space.converted_scope",
                        "IMG/VID converted bytes were rejected, unknown bytes became zero, or growth became savings")
        }
    }
    var ordinarySummary = summaryValue
    ordinarySummary["img"] = failedImage
    ordinarySummary["failed_files"] = ["/synthetic/failed.jpg"]
    ordinarySummary["size"] = ["scope": "converted_this_run", "input_bytes": 1_024, "output_bytes": 512]
    for invalidFlag in [true, "true", 1, NSNull()] as [Any] {
        ordinarySummary["size_invalid"] = invalidFlag
        for finish in [completed, failed] {
            let invalid = try parsed([context, try payload("MFB_HISTORY_SUMMARY", ordinarySummary), finish]).entries[0]
            try require(invalid.summary?.sizeInvalid == true && invalid.summary?.size == nil
                        && invalid.summary?.img.failed == 1 && invalid.succeededLabel == "2"
                        && invalid.summary?.failed_files == ["/synthetic/failed.jpg"]
                        && invalid.sizeOutcome == localized("result.unknown")
                        && invalid.statusKey == (finish == completed ? "history.status.file_failures" : "history.status.failed"),
                        "An invalid size flag hid file outcomes or terminal failure")
        }
    }
    ordinarySummary = summaryValue
    ordinarySummary["img"] = inactive
    ordinarySummary["size"] = ["scope": "converted_this_run", "input_bytes": NSNull(), "output_bytes": NSNull()]
    let inactiveOrdinary = try parsed([context, try payload("MFB_HISTORY_SUMMARY", ordinarySummary), completed]).entries[0]
    try require(inactiveOrdinary.summary?.sizeInvalid == true && inactiveOrdinary.summary?.size == nil,
                "Converted bytes were accepted without an active pipeline")
    ordinarySummary["vid"] = media
    ordinarySummary["size"] = ["scope": "fast_img_converted_this_run", "input_bytes": 512, "output_bytes": 256]
    let wrongPipeline = try parsed([context, try payload("MFB_HISTORY_SUMMARY", ordinarySummary), completed]).entries[0]
    try require(wrongPipeline.summary?.sizeInvalid == true && wrongPipeline.summary?.size == nil,
                "Fast IMG byte evidence was accepted for a video-only run")
    let imgConfigValue: [String: Any] = ["schema_version": 1, "pipeline": "img", "package_version": "0.12.0",
        "config": ["config_version": 1, "performance": ["mode": "tight"]],
        "sources": ["config_version": "default", "performance.mode": "CLI"]]
    var vidConfigValue = imgConfigValue
    vidConfigValue["pipeline"] = "vid"
    vidConfigValue["config"] = ["config_version": 1, "performance": ["mode": "relaxed"]]
    let imgConfig = try payload("MFB_HISTORY_CONFIG", imgConfigValue)
    let vidConfig = try payload("MFB_HISTORY_CONFIG", vidConfigValue)
    let launchValue: [String: Any] = ["schema_version": 1, "invocation_id": "synthetic-call-1", "pipeline": "img",
        "state": "launch_requested", "program_requested": "/synthetic/worker 界面/img", "program_path": "/synthetic/worker 界面/img",
        "arguments": ["run", "/synthetic/folder with spaces/界面.jpg", "--error-mode", "log-and-continue"],
        "binary": ["scope": "observed_before_launch", "bytes": 123, "blake3": String(repeating: "a", count: 64), "diagnostic": NSNull()]]
    let launch = try payload("MFB_HISTORY_INVOCATION", launchValue)
    var secondLaunchValue = launchValue
    secondLaunchValue["invocation_id"] = "synthetic-call-2"
    secondLaunchValue["pipeline"] = "vid"
    secondLaunchValue["program_requested"] = "/synthetic/vid"
    secondLaunchValue["program_path"] = NSNull()
    secondLaunchValue["binary"] = ["scope": "observed_before_launch", "bytes": NSNull(), "blake3": NSNull(), "diagnostic": "synthetic read denied"]
    let secondLaunch = try payload("MFB_HISTORY_INVOCATION", secondLaunchValue)
    var boundImageValue = imgConfigValue
    boundImageValue["invocation_id"] = "synthetic-call-1"
    var boundVideoValue = vidConfigValue
    boundVideoValue["invocation_id"] = "synthetic-call-2"
    let boundImage = try payload("MFB_HISTORY_CONFIG", boundImageValue)
    let boundVideo = try payload("MFB_HISTORY_CONFIG", boundVideoValue)
    let launched = try parsed([context, launch, boundImage, secondLaunch, boundVideo, summary, completed]).entries[0]
    try require(launched.invocations.count == 2 && launched.invocationConfigs.count == 2
                && launched.invocationConfigs["synthetic-call-1"]?.pipeline == "img"
                && launched.invocationConfigs["synthetic-call-2"]?.pipeline == "vid"
                && launched.invocations[0].arguments[1] == "/synthetic/folder with spaces/界面.jpg"
                && launched.invocations[1].program_path == nil && launched.invocations[1].binary.bytes == nil
                && launched.invocations[1].binary.diagnostic == "synthetic read denied"
                && launched.succeededLabel == "2" && launched.statusKey == "history.status.completed"
                && ProcessingHistoryPanel.render(launched).contains(localized("history.worker.note")),
                "Worker launch provenance lost exact argv, unknown evidence, configuration binding, or real processing counts")
    let launchBatches = try parsed([context, launch, boundImage, summary, completed, context, secondLaunch, boundVideo, summary, completed])
    try require(launchBatches.entries[0].invocations.map(\.invocation_id) == ["synthetic-call-1"]
                && launchBatches.entries[1].invocations.map(\.invocation_id) == ["synthetic-call-2"],
                "Worker launch evidence crossed selected-run boundaries")
    try require(success.entries[0].invocations.isEmpty
                && ProcessingHistoryPanel.render(success.entries[0]).contains(localized("history.worker.not_recorded")),
                "Old history fabricated a worker launch receipt")
    var wrongBindingValue = boundVideoValue
    wrongBindingValue["invocation_id"] = "synthetic-call-1"
    for invalidSequence in [[launch, launch], [launch, boundImage, boundImage], [launch, imgConfig],
                             [launch, try payload("MFB_HISTORY_CONFIG", wrongBindingValue)], [boundImage]] {
        let invalid = try parsed([context] + invalidSequence + [summary, failed]).entries[0]
        try require(!invalid.issues.isEmpty && invalid.succeededLabel == "2" && invalid.statusKey == "history.status.failed"
                    && (invalid.invocationEvidenceInvalid || invalid.configEvidenceInvalid),
                    "Duplicate, missing or mismatched launch binding replaced real counts or appeared trustworthy")
    }
    for (key, value) in [("schema_version", 2 as Any), ("state", "spawned"), ("invocation_id", ""),
                          ("arguments", Array(repeating: "x", count: 4097)), ("arguments", [String(repeating: "x", count: 256 * 1024 + 1)]),
                          ("arguments", ["invalid\0argument"]), ("program_path", "relative/img")] {
        var invalidLaunch = launchValue
        invalidLaunch[key] = value
        let invalid = try parsed([context, try payload("MFB_HISTORY_INVOCATION", invalidLaunch), summary, completed]).entries[0]
        try require(invalid.invocationEvidenceInvalid && invalid.invocations.isEmpty && invalid.succeededLabel == "2",
                    "Malformed worker provenance was accepted or changed processing counts")
    }
    let withConfigs = try parsed([context, imgConfig, imgConfig, vidConfig, summary, completed])
    let configEntry = withConfigs.entries[0]
    let shownValues = try JSONSerialization.jsonObject(with: Data(configEntry.configSnapshots[0].values.utf8)) as? [String: Any]
    let shownSources = try JSONSerialization.jsonObject(with: Data(configEntry.configSnapshots[0].sources.utf8)) as? [String: String]
    try require(configEntry.configInvocationCount == 3 && configEntry.configSnapshots.count == 2
                && configEntry.configSnapshots.map(\.pipeline) == ["img", "vid"]
                && ((shownValues?["performance"] as? [String: Any])?["mode"] as? String) == "tight"
                && shownSources?["performance.mode"] == "CLI"
                && configEntry.succeededLabel == "2" && configEntry.statusKey == "history.status.completed",
                "Child configuration snapshots changed media counts or lost distinct invocations")
    let configBatches = try parsed([context, imgConfig, summary, completed, context, vidConfig, summary, completed])
    try require(configBatches.entries.count == 2 && configBatches.entries[0].configSnapshots.map(\.pipeline) == ["img"]
                && configBatches.entries[1].configSnapshots.map(\.pipeline) == ["vid"],
                "Effective configurations crossed batch boundaries")
    let invalidConfigs: [[String: Any]] = [
        ["schema_version": 1, "pipeline": "img", "package_version": "0.12.0", "sources": [:]],
        ["schema_version": 1, "pipeline": "other", "package_version": "0.12.0", "config": [:], "sources": [:]],
        ["schema_version": 1, "pipeline": "img", "package_version": 12, "config": [:], "sources": [:]],
        ["schema_version": 1, "pipeline": "img", "package_version": "0.12.0", "config": [], "sources": [:]],
        ["schema_version": 1, "pipeline": "img", "package_version": "0.12.0", "config": [:], "sources": ["mode": 1]],
        ["schema_version": 2, "pipeline": "img", "package_version": "0.12.0", "config": [:], "sources": [:]],
    ]
    for invalidConfig in invalidConfigs {
        let invalidEvent = try payload("MFB_HISTORY_CONFIG", invalidConfig)
        let history = try parsed([context, imgConfig, invalidEvent, summary, completed])
        try require(history.entries[0].statusKey == "history.status.incomplete"
                    && history.entries[0].configSnapshots.count == 1
                    && history.entries[0].configInvocationCount == 1
                    && history.entries[0].summary?.img.succeeded == 2,
                    "Invalid config evidence erased a valid snapshot or processor counts")
    }
    let malformedConfig = try parsed([context, imgConfig, "MFB_HISTORY_CONFIG={", summary, failed])
    try require(malformedConfig.entries[0].configEvidenceInvalid
                && malformedConfig.entries[0].statusKey == "history.status.failed"
                && malformedConfig.entries[0].configSnapshots.count == 1
                && malformedConfig.entries[0].summary?.img.succeeded == 2,
                "Malformed config hid the batch failure, captured configuration or media counts")
    var configSequence = [context]
    for index in 0...HistoryStore.maximumConfigSnapshots {
        var distinct = imgConfigValue
        distinct["package_version"] = "0.12.\(index)"
        configSequence.append(try payload("MFB_HISTORY_CONFIG", distinct))
    }
    configSequence += [summary, completed]
    let limitedConfigs = try parsed(configSequence).entries[0]
    try require(limitedConfigs.configEvidenceInvalid
                && limitedConfigs.configSnapshots.count == HistoryStore.maximumConfigSnapshots
                && limitedConfigs.configInvocationCount == HistoryStore.maximumConfigSnapshots + 1
                && limitedConfigs.succeededLabel == "2", "Snapshot limit changed processor counts or concealed truncation")
    var oversizedConfigValue = imgConfigValue
    oversizedConfigValue["config"] = ["long": String(repeating: "x", count: HistoryStore.maximumConfigBytes)]
    let oversizedConfig = try parsed([context, try payload("MFB_HISTORY_CONFIG", oversizedConfigValue), summary, completed])
    try require(oversizedConfig.entries[0].configEvidenceInvalid && oversizedConfig.entries[0].configSnapshots.isEmpty,
                "Oversized effective configuration escaped the history display bound")
    let failure = try parsed([context, summary, failed])
    try require(failure.entries[0].statusKey == "history.status.failed" && failure.entries[0].succeededLabel == "2",
                "Converted items hid a final session failure")
    var mixedMedia = media
    mixedMedia["failed"] = 1
    mixedMedia["exit_code"] = 1
    var mixedSummary = summaryValue
    mixedSummary["img"] = mixedMedia
    mixedSummary["failed_files"] = ["/synthetic/failed.jpg"]
    let mixedEvent = try payload("MFB_HISTORY_SUMMARY", mixedSummary)
    for finish in [completed, failed] {
        let mixed = try parsed([context, mixedEvent, finish]).entries[0]
        try require(mixed.statusKey == "history.status.partial" && mixed.needsAttention
                    && mixed.summary?.img.exit_code == 1 && mixed.summary?.failed_files == ["/synthetic/failed.jpg"]
                    && mixed.succeededLabel == "2" && mixed.finished?.outcome == (finish == completed ? "completed" : "failed"),
                    "Partial history hid preserved success, failed items, or terminal evidence")
    }
    let cancelledMixed = try parsed([context, mixedEvent, cancelled]).entries[0]
    let unfinishedMixed = try parsed([context, mixedEvent]).entries[0]
    let contradictoryMixed = try parsed([context, mixedEvent, summary, completed]).entries[0]
    try require(cancelledMixed.statusKey == "history.status.cancelled" && !cancelledMixed.hasPartialSuccess
                && unfinishedMixed.statusKey != "history.status.partial" && !unfinishedMixed.hasPartialSuccess
                && contradictoryMixed.statusKey == "history.status.incomplete" && !contradictoryMixed.hasPartialSuccess,
                "Cancelled, unfinished, or contradictory history became an ordinary partial result")
    for key in ["succeeded", "skipped", "ignored", "failed", "unprocessed", "exit_code"] {
        var unknownMedia = mixedMedia
        unknownMedia[key] = NSNull()
        var unknownSummary = mixedSummary
        unknownSummary["img"] = unknownMedia
        let unknown = try parsed([context, try payload("MFB_HISTORY_SUMMARY", unknownSummary), failed]).entries[0]
        try require(!unknown.hasPartialSuccess && unknown.statusKey == "history.status.failed",
                    "Unknown \(key) evidence became a reliable partial result")
    }
    var pendingMixedMedia = mixedMedia
    pendingMixedMedia["unprocessed"] = 1
    var pendingMixedSummary = mixedSummary
    pendingMixedSummary["img"] = pendingMixedMedia
    let pendingMixed = try parsed([context, try payload("MFB_HISTORY_SUMMARY", pendingMixedSummary), failed]).entries[0]
    try require(!pendingMixed.hasPartialSuccess && pendingMixed.statusKey == "history.status.failed",
                "Unprocessed sources became a finished partial batch")
    var failedVideo = media
    failedVideo["succeeded"] = 0
    failedVideo["failed"] = 1
    failedVideo["exit_code"] = 1
    var combinedSummary = summaryValue
    combinedSummary["vid"] = failedVideo
    let combined = try parsed([context, try payload("MFB_HISTORY_SUMMARY", combinedSummary), failed]).entries[0]
    try require(combined.hasPartialSuccess && combined.summary?.img.succeeded == 2 && combined.summary?.vid.failed == 1,
                "Partial history only considered one active pipeline")
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
    let restoreContext = try payload("MFB_HISTORY_CONTEXT", ["schema_version": 1,
        "inputs": ["/synthetic/input"], "output": "/synthetic/restored_jpeg", "mode": "restore-jpeg",
        "dry_run": false, "in_place": false, "shortest_path": false])
    var restoreReceipt = verificationValue
    restoreReceipt["output_path"] = "/synthetic/restored_jpeg"
    let restored = try parsed([restoreContext, summary, try payload("MFB_HISTORY_VERIFICATION", restoreReceipt), completed]).entries[0]
    let restoredDetails = ProcessingHistoryPanel.render(restored)
    try require(restored.isRestoreJpeg && restored.modeLabel == localized("operation.restore_jpeg")
                && restored.statusKey == "history.status.completed" && restored.succeededLabel == "2"
                && restored.verification?.optimized_count == 2
                && restored.sizeOutcome == localized("history.space.not_applicable_restore")
                && restoredDetails.contains(localized("history.detail.restored_inventory") + ": 2")
                && !restoredDetails.contains(localized("history.detail.optimized_inventory"))
                && !restoredDetails.contains(localized("history.space.title"))
                && !restored.sizeMeasurementsComparable(to: reduced),
                "JPEG restoration used optimization labels, changed counters, or claimed comparable savings")
    let restoreWithoutReceipt = try parsed([restoreContext, summary, completed]).entries[0]
    try require(restoreWithoutReceipt.succeededLabel == "2" && restoreWithoutReceipt.verification == nil
                && !ProcessingHistoryPanel.render(restoreWithoutReceipt).contains("MATCH"),
                "Restoration without a directory receipt invented verified counts")
    let photosRestoreContext = try payload("MFB_HISTORY_CONTEXT", ["schema_version": 1,
        "inputs": ["/synthetic/debug.photoslibrary"], "mode": "restore-jpeg",
        "dry_run": false, "in_place": false, "shortest_path": false])
    let photosRestore = try parsed([photosRestoreContext, summary, completed]).entries[0]
    try require(photosRestore.context?.output == nil && photosRestore.verification == nil
                && photosRestore.issues.isEmpty && photosRestore.statusKey == "history.status.completed"
                && photosRestore.succeededLabel == "2" && !ProcessingHistoryPanel.render(photosRestore).contains("MATCH"),
                "A Photos restore audit without a directory receipt lost its counts or invented verification")
    var wrongRestoreSource = restoreReceipt
    wrongRestoreSource["source_path"] = "/synthetic/other_input"
    var wrongRestoreOutput = restoreReceipt
    wrongRestoreOutput["output_path"] = "/synthetic/other_output"
    var photosDirectoryReceipt = restoreReceipt
    photosDirectoryReceipt["source_path"] = "/synthetic/debug.photoslibrary"
    for (contextMarker, receipt) in [(restoreContext, wrongRestoreSource), (restoreContext, wrongRestoreOutput),
                                     (photosRestoreContext, photosDirectoryReceipt)] {
        let receiptMarker = try payload("MFB_HISTORY_VERIFICATION", receipt)
        let invalid = try parsed([contextMarker, summary, receiptMarker, completed]).entries[0]
        try require(invalid.verification == nil && !invalid.issues.isEmpty
                    && invalid.statusKey == "history.status.incomplete" && invalid.succeededLabel == "2"
                    && invalid.outputPaths == invalid.context?.output
                    && !ProcessingHistoryPanel.render(invalid).contains("MATCH"),
                    "An unrelated restore receipt replaced the bound output, claimed MATCH, or erased processing counts")
        let failedInvalid = try parsed([contextMarker, summary, receiptMarker, failed]).entries[0]
        try require(failedInvalid.verification == nil && failedInvalid.statusKey == "history.status.failed"
                    && failedInvalid.succeededLabel == "2",
                    "Rejecting a restore receipt hid the terminal failure or discarded processing counts")
    }
    var noRestoredMedia = media
    noRestoredMedia["succeeded"] = 0
    noRestoredMedia["skipped"] = 3
    var noRestoredSummary = summaryValue
    noRestoredSummary["img"] = noRestoredMedia
    let noRestored = try parsed([restoreContext, try payload("MFB_HISTORY_SUMMARY", noRestoredSummary), completed]).entries[0]
    try require(noRestored.succeededLabel == "0" && noRestored.summary?.img.skipped == 3
                && noRestored.verification == nil && noRestored.summary?.size == nil,
                "Ineligible JPEG restoration invented outputs or verification evidence")
    let restoreWithSize = try parsed([restoreContext, try payload("MFB_HISTORY_SUMMARY", otherSizeSummary), completed]).entries[0]
    let restoreWithSizeDetails = ProcessingHistoryPanel.render(restoreWithSize)
    try require(restoreWithSize.summary?.size == nil && restoreWithSize.summary?.sizeInvalid == true
                && restoreWithSize.succeededLabel == "2" && restoreWithSize.statusKey == "history.status.incomplete"
                && restoreWithSizeDetails.contains(localized("history.space.invalid"))
                && !restoreWithSizeDetails.contains(localized("history.space.saved", HistoryEntry.sizeLabel(424)))
                && !ProcessingHistoryPanel.renderComparison([restored, restoreWithSize]).contains(localized("history.space.title")),
                "A conversion-size receipt became valid restoration savings or erased real counters")
    let failedRestore = try parsed([restoreContext, summary, failed]).entries[0]
    try require(failedRestore.statusKey == "history.status.failed" && failedRestore.succeededLabel == "2"
                && failedRestore.verification == nil && failedRestore.sizeOutcome == localized("history.space.not_applicable_restore"),
                "Restoration presentation hid a failed batch or fabricated final verification")
    for (issueCount, countStatus) in [(1, "MATCH"), (0, "MISMATCH")] {
        var warningReceipt = restoreReceipt
        warningReceipt["issue_count"] = issueCount
        warningReceipt["count_status"] = countStatus
        let warningEntry = try parsed([restoreContext, summary, try payload("MFB_HISTORY_VERIFICATION", warningReceipt), completed]).entries[0]
        let warningComparison = ProcessingHistoryPanel.renderComparison([warningEntry, warningEntry])
        try require(warningEntry.statusKey == "history.status.verification_warnings"
                    && warningComparison.contains(localized("history.detail.verification_state") + ": WARNINGS")
                    && !warningComparison.contains(localized("history.detail.verification_state") + ": CLEAN"),
                    "History comparison hid a verifier issue or count mismatch behind CLEAN")
    }
    let sources = ["/synthetic/one", "/synthetic/two"]
    let outputs = ["/synthetic/one_optimized", "/synthetic/two_optimized"]
    let multiContext = try payload("MFB_HISTORY_CONTEXT", ["schema_version": 1, "inputs": sources,
        "mode": "standard", "dry_run": false, "in_place": false, "shortest_path": false])
    let planRows: [[String: Any]] = sources.indices.map {
        ["source": sources[$0], "output": outputs[$0], "in_place": false, "requires_verification": true]
    }
    let plan = try payload("MFB_HISTORY_INPUT_PLAN", ["schema_version": 1, "inputs": planRows])
    var firstReceipt = verificationValue
    firstReceipt["source_path"] = sources[0]
    firstReceipt["output_path"] = outputs[0]
    firstReceipt["count_status"] = "MATCH"
    var secondReceipt = firstReceipt
    secondReceipt["source_path"] = sources[1]
    secondReceipt["output_path"] = outputs[1]
    let firstEvent = try payload("MFB_HISTORY_VERIFICATION", firstReceipt)
    let secondEvent = try payload("MFB_HISTORY_VERIFICATION", secondReceipt)
    let multiVerified = try parsed([multiContext, plan, firstEvent, secondEvent, firstEvent, summary, completed]).entries[0]
    try require(multiVerified.issues.isEmpty && multiVerified.inputVerifications.count == 2
                && multiVerified.verification?.source_count == 6 && multiVerified.verification?.optimized_count == 4
                && multiVerified.verification?.count_status == "MATCH" && multiVerified.statusKey == "history.status.completed",
                "Multi-input history lost receipts, double-counted duplicates or rejected independent paths")
    let renderedMulti = ProcessingHistoryPanel.render(multiVerified)
    try require(sources.allSatisfy { renderedMulti.contains($0) } && outputs.allSatisfy { renderedMulti.contains($0) },
                "History hid a planned input or its actual output")
    var explainedReceipt = secondReceipt
    explainedReceipt["count_status"] = "EXPLAINED (1 fewer; all 1 listed below)"
    let multiExplained = try parsed([multiContext, plan, firstEvent,
        try payload("MFB_HISTORY_VERIFICATION", explainedReceipt), summary, completed]).entries[0]
    try require(multiExplained.issues.isEmpty && multiExplained.verification?.count_status == "EXPLAINED",
                "Explained count differences became a MATCH or were rejected")
    let missingReceipt = try parsed([multiContext, plan, firstEvent, summary, completed]).entries[0]
    try require(missingReceipt.verification == nil && missingReceipt.statusKey == "history.status.incomplete",
                "One directory receipt became a whole-batch MATCH")
    var changedReceipt = firstReceipt
    changedReceipt["optimized_count"] = 1
    let conflictingReceipt = try payload("MFB_HISTORY_VERIFICATION", changedReceipt)
    for extra in [conflictingReceipt, verification, "MFB_HISTORY_VERIFICATION={}"] {
        let invalid = try parsed([multiContext, plan, firstEvent, secondEvent, extra, summary, completed]).entries[0]
        try require(invalid.verification == nil && invalid.statusKey == "history.status.incomplete",
                    "Conflicting, unknown or malformed per-input evidence was accepted")
    }
    changedReceipt = firstReceipt
    changedReceipt["output_path"] = outputs[1]
    let wrongOutput = try parsed([multiContext, plan, try payload("MFB_HISTORY_VERIFICATION", changedReceipt), secondEvent, summary, completed]).entries[0]
    try require(wrongOutput.verification == nil && !wrongOutput.issues.isEmpty,
                "A receipt for the wrong planned output was accepted")
    changedReceipt = secondReceipt
    changedReceipt["source_count"] = Int.max
    let overflowing = try parsed([multiContext, plan, firstEvent, try payload("MFB_HISTORY_VERIFICATION", changedReceipt), summary, completed]).entries[0]
    try require(overflowing.verification == nil && overflowing.statusKey == "history.status.incomplete",
                "Overflowing multi-input inventory became MATCH")
    var invalidPlanRows = planRows
    invalidPlanRows[1]["source"] = sources[0]
    let invalidPlan = try payload("MFB_HISTORY_INPUT_PLAN", ["schema_version": 1, "inputs": invalidPlanRows])
    let duplicateInput = try parsed([multiContext, invalidPlan, summary, completed]).entries[0]
    try require(duplicateInput.inputPlan == nil && !duplicateInput.issues.isEmpty,
                "Duplicate or out-of-order input plans were accepted")
    var mixedPlanRows = planRows
    mixedPlanRows[1]["requires_verification"] = false
    let mixedPlan = try payload("MFB_HISTORY_INPUT_PLAN", ["schema_version": 1, "inputs": mixedPlanRows])
    let mixedInputs = try parsed([multiContext, mixedPlan, firstEvent, summary, completed]).entries[0]
    try require(mixedInputs.issues.isEmpty && mixedInputs.verification == nil
                && mixedInputs.inputVerifications.count == 1 && mixedInputs.statusKey == "history.status.completed",
                "Directory inventory was incorrectly attributed to an unmeasured single-file input")
    changedReceipt = firstReceipt
    changedReceipt["has_warnings"] = true
    changedReceipt["issue_count"] = 1
    let mixedWarning = try parsed([multiContext, mixedPlan, try payload("MFB_HISTORY_VERIFICATION", changedReceipt), summary, completed]).entries[0]
    try require(mixedWarning.verification == nil && mixedWarning.hasVerificationWarnings
                && mixedWarning.statusKey == "history.status.verification_warnings",
                "A per-input warning disappeared when the batch inventory was unavailable")
    let multiPreview = try payload("MFB_HISTORY_CONTEXT", ["schema_version": 1, "inputs": sources,
        "mode": "standard", "dry_run": true, "in_place": false, "shortest_path": false])
    let previewPlan = try parsed([multiPreview, plan, completed]).entries[0]
    try require(previewPlan.verification == nil && previewPlan.statusKey == "history.status.preview",
                "Preview required processing receipts or fabricated verification")
    try require(reduced.sizeMeasurementsComparable(to: expanded)
                && !reduced.sizeMeasurementsComparable(to: partialSize)
                && !reduced.sizeMeasurementsComparable(to: cancelledMixed),
                "History size comparison accepted missing or cancelled measurements")
    var scopeChanged = reduced
    scopeChanged.summary?.size = HistorySummary.Size(scope: "converted_this_run", input_bytes: 1_024, output_bytes: 512)
    try require(!reduced.sizeMeasurementsComparable(to: scopeChanged), "Different byte measurement scopes became directly comparable")
    var videoOnlySummary = summaryValue
    videoOnlySummary["img"] = inactive
    videoOnlySummary["vid"] = media
    videoOnlySummary["size"] = ["scope": "converted_this_run", "input_bytes": 1_024, "output_bytes": 512]
    let videoOnly = try parsed([context, try payload("MFB_HISTORY_SUMMARY", videoOnlySummary), completed]).entries[0]
    try require(!scopeChanged.sizeMeasurementsComparable(to: videoOnly), "Different active pipelines became comparable size measurements")
    let comparison = ProcessingHistoryPanel.renderComparison([reduced, verified.entries[0]])
    try require(comparison.contains(localized("history.compare.not_comparable"))
                && comparison.contains("MATCH (1 expected handoff gap)")
                && comparison.contains("/synthetic/input") && comparison.contains("/synthetic/output")
                && comparison.contains(HistoryEntry.sizeLabel(1_024))
                && !comparison.contains(localized("history.effective_config")),
                "History comparison lost recorded evidence or invented configuration differences")
    let configuredComparison = ProcessingHistoryPanel.renderComparison([configEntry, combined])
    try require(configuredComparison.contains("img: 0.12.0") && configuredComparison.contains("vid: 0.12.0")
                && configuredComparison.contains("synthetic final failure")
                && !configuredComparison.contains(configEntry.configSnapshots[0].values),
                "History comparison lost worker versions or buried outcomes in configuration JSON")
    func configured(_ marker: [String: Any]) throws -> HistoryEntry {
        try parsed([context, try payload("MFB_HISTORY_CONFIG", marker), summary, completed]).entries[0]
    }
    let oneConfig = try configured(imgConfigValue)
    let repeatedConfig = try parsed([context, imgConfig, imgConfig, summary, completed]).entries[0]
    let sameConfigComparison = ProcessingHistoryPanel.renderComparison([oneConfig, repeatedConfig])
    try require(sameConfigComparison.contains(localized("history.compare.config"))
                && sameConfigComparison.contains(localized("history.compare.config.values.same"))
                && sameConfigComparison.contains(localized("history.compare.config.sources.same"))
                && !sameConfigComparison.contains(localized("history.compare.config.unavailable"))
                && repeatedConfig.configInvocationCount == 2 && repeatedConfig.succeededLabel == "2",
                "Repeated identical snapshots became ambiguous or changed processor counts")
    var changedConfigValue = vidConfigValue
    changedConfigValue["pipeline"] = "img"
    let changedConfig = try configured(changedConfigValue)
    let valuesComparison = ProcessingHistoryPanel.renderComparison([oneConfig, changedConfig])
    try require(valuesComparison.contains(localized("history.compare.config.field", "performance.mode", "\"tight\"", "\"relaxed\""))
                && valuesComparison.contains(localized("history.compare.config.sources.same"))
                && !valuesComparison.contains(oneConfig.configSnapshots[0].values),
                "Recorded value changes were missing or replaced by full JSON")
    changedConfigValue = imgConfigValue
    changedConfigValue["sources"] = ["config_version": "default", "performance.mode": "project:synthetic"]
    let changedSource = try configured(changedConfigValue)
    let sourceComparison = ProcessingHistoryPanel.renderComparison([oneConfig, changedSource])
    try require(sourceComparison.contains(localized("history.compare.config.values.same"))
                && sourceComparison.contains(localized("history.compare.config.field", "performance.mode", "\"CLI\"", "\"project:synthetic\"")),
                "Source-only changes became configuration value changes")
    changedConfigValue["config"] = ["config_version": 1]
    changedConfigValue["sources"] = ["config_version": "default"]
    let missingField = try configured(changedConfigValue)
    for (first, second, oldValue, newValue) in [
        (oneConfig, missingField, "\"tight\"", localized("history.compare.config.missing")),
        (missingField, oneConfig, localized("history.compare.config.missing"), "\"tight\"")
    ] {
        let shown = ProcessingHistoryPanel.renderComparison([first, second])
        try require(shown.contains(localized("history.compare.config.field", "performance.mode", oldValue, newValue)),
                    "Missing configuration fields became defaults or disappeared from the comparison")
    }
    changedConfigValue["config"] = ["config_version": 1, "performance": ["flag": true]]
    changedConfigValue["sources"] = ["config_version": "default", "performance.flag": "CLI"]
    let booleanField = try configured(changedConfigValue)
    changedConfigValue["config"] = ["config_version": 1, "performance": ["flag": 1]]
    let numberField = try configured(changedConfigValue)
    try require(ProcessingHistoryPanel.renderComparison([booleanField, numberField])
                .contains(localized("history.compare.config.field", "performance.flag", "true", "1")),
                "Boolean and numeric configuration values were treated as equal")
    let collidingFields = HistoryConfigSnapshot(pipeline: "img", packageVersion: "0.12.0",
        values: "{\"a.b\":1,\"a\":{\"b\":2}}", sources: "{\"a.b\":\"CLI\"}")
    var rejectedCollidingFields = false
    do {
        _ = try collidingFields.comparisonFields()
    } catch { rejectedCollidingFields = true }
    try require(rejectedCollidingFields, "Configuration field paths silently overwrote each other")
    let ambiguousConfig = try parsed([context, imgConfig, try payload("MFB_HISTORY_CONFIG", [
        "schema_version": 1, "pipeline": "img", "package_version": "0.12.0",
        "config": ["config_version": 1, "performance": ["mode": "relaxed"]],
        "sources": ["config_version": "default", "performance.mode": "CLI"]]), summary, completed]).entries[0]
    var incompleteConfig = oneConfig
    incompleteConfig.issues.append(localized("history.issue.too_large"))
    for unavailable in [success.entries[0], malformedConfig.entries[0], ambiguousConfig, incompleteConfig] {
        let shown = ProcessingHistoryPanel.renderComparison([oneConfig, unavailable])
        try require(shown.contains(localized("history.compare.config.unavailable"))
                    && !shown.contains(localized("history.compare.config.values.same"))
                    && unavailable.succeededLabel == "2",
                    "Missing, invalid, ambiguous or incomplete configuration evidence appeared unchanged")
    }
    try require(ProcessingHistoryPanel.renderComparison([oneConfig, ambiguousConfig])
                .contains(localized("history.compare.config.snapshots", 1, 2)),
                "Ambiguous snapshot counts were hidden or worker invocations were guessed")
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

    let namedRoot = directory.appendingPathComponent("named-archive-case")
    let namedArchive = namedRoot.appendingPathComponent("Photos_Bundle_named")
    try FileManager.default.createDirectory(at: namedArchive, withIntermediateDirectories: true)
    try records([context, summary, completed]).write(to: namedArchive.appendingPathComponent("session_audit_named.jsonl"))
    let named = try HistoryStore.load(directory: namedRoot)
    try require(named.entries.count == 1
                && named.entries[0].audit.deletingLastPathComponent().resolvingSymlinksInPath().path
                    == namedArchive.resolvingSymlinksInPath().path,
                "Named archive was not discovered: \(named.entries.map { $0.audit.path })")

    let databaseURL = directory.appendingPathComponent("history.sqlite3")
    func writeDatabase(_ sessions: [(String, [String])], schema: Int = 1) throws {
        var database: OpaquePointer?
        guard sqlite3_open_v2(databaseURL.path, &database, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, nil) == SQLITE_OK,
              let database else { throw HostError(message: "Unable to create synthetic history database") }
        defer { sqlite3_close(database) }
        let setup = """
        DROP TABLE IF EXISTS history_output;
        DROP TABLE IF EXISTS history_events;
        DROP TABLE IF EXISTS history_sessions;
        CREATE TABLE history_sessions(session_id TEXT PRIMARY KEY, updated_at TEXT NOT NULL);
        CREATE TABLE history_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES history_sessions(session_id), ts TEXT NOT NULL, event TEXT NOT NULL);
        CREATE INDEX history_events_session_sequence ON history_events(session_id, sequence);
        CREATE INDEX history_sessions_updated_at ON history_sessions(updated_at);
        \(schema == 2 ? "CREATE TABLE history_output(sequence INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES history_sessions(session_id), ts TEXT NOT NULL, pipeline TEXT NOT NULL, line TEXT NOT NULL); CREATE INDEX history_output_session_sequence ON history_output(session_id, sequence);" : "")
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
        "history.status.preview", "history.status.unfinished", "history.status.partial",
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
    var outputContext: [String: Any] = ["schema_version": 1, "inputs": ["/synthetic/input"], "mode": "fast-img",
        "dry_run": false, "in_place": false, "shortest_path": false, "version": "0.12.0", "retry": true,
        "resume": false, "no_resume": false, "output_sequence": 0]
    let firstContext = try payload("MFB_HISTORY_CONTEXT", outputContext)
    outputContext["output_sequence"] = 1
    let secondContext = try payload("MFB_HISTORY_CONTEXT", outputContext)
    let firstFinished = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "failed", "output_sequence": 1, "error": "synthetic failure"])
    let secondFinished = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "completed", "output_sequence": HistoryStore.maximumOutputLines + 3])
    try writeDatabase([("output", [firstContext, firstFinished, secondContext, summary, secondFinished])], schema: 2)
    var outputDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &outputDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let outputDB else {
        throw HostError(message: "Synthetic output database open failed")
    }
    let outputSQL = """
    INSERT INTO history_output(session_id, ts, pipeline, line) VALUES ('output', '2026-10-07T01:02:03Z', 'IMG', 'synthetic first failure');
    WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM rows WHERE n<=\(HistoryStore.maximumOutputLines + 1))
    INSERT INTO history_output(session_id, ts, pipeline, line) SELECT 'output', '2026-10-07T01:02:04Z', 'IMG', 'synthetic next run ' || n FROM rows;
    UPDATE history_output SET line='\(processingPhase)' WHERE sequence=2;
    UPDATE history_output SET line='[WARN] /synthetic/item.jpg: warning 42' WHERE sequence BETWEEN 3 AND 1001;
    UPDATE history_output SET line='ERR: \u{001B}[33m[WARN] /synthetic/item.jpg: warning 42\u{001B}[0m' WHERE sequence=999;
    UPDATE history_output SET line='\(verificationPhase)' WHERE sequence=1000;
    UPDATE history_output SET pipeline='VID', line='[FAIL] /synthetic/item.jpg: warning 42' WHERE sequence=1002;
    """
    let inserted = sqlite3_exec(outputDB, outputSQL, nil, nil, nil)
    sqlite3_close(outputDB)
    try require(inserted == SQLITE_OK, "Synthetic output insert failed")
    let outputBytes = try Data(contentsOf: databaseURL)
    let outputEntries = try HistoryStore.load(directory: directory).entries.filter { $0.stamp == "output" }
        .sorted { ($0.context?.output_sequence ?? -1) < ($1.context?.output_sequence ?? -1) }
    try require(outputEntries.count == 2, "Output sessions merged watch-mode batches")
    let firstOutput = try HistoryStore.output(for: outputEntries[0])
    try require(firstOutput.total == 1 && firstOutput.shown == 1 && firstOutput.text.contains("first failure")
                && !firstOutput.text.contains("next run") && firstOutput.rows[0].pipeline == "IMG"
                && firstOutput.rows[0].line == "synthetic first failure", "Output leaked another batch or lost diagnostic identity")
    let nextOutput = try HistoryStore.output(for: outputEntries[1])
    try require(nextOutput.total == Int64(HistoryStore.maximumOutputLines + 2)
                && nextOutput.shown == HistoryStore.maximumOutputLines
                && !nextOutput.text.contains("first failure") && nextOutput.text.hasSuffix("synthetic next run 1002"),
                "Bounded output lost latest records or invented a count")
    let olderOutput = try HistoryStore.output(for: outputEntries[1], cursor: nextOutput.rows.first!.sequence,
                                            direction: .older, upperSnapshot: nextOutput.upper)
    try require(olderOutput.shown == 2 && !olderOutput.hasOlder && olderOutput.hasNewer
                && olderOutput.firstOrdinal == 1 && nextOutput.firstOrdinal == 3
                && olderOutput.rows.last!.sequence + 1 == nextOutput.rows.first!.sequence,
                "Keyset navigation overlapped, skipped, or miscounted output rows")
    let newerOutput = try HistoryStore.output(for: outputEntries[1], cursor: olderOutput.rows.last!.sequence,
                                            direction: .newer, upperSnapshot: olderOutput.upper)
    try require(newerOutput.rows.map(\.sequence) == nextOutput.rows.map(\.sequence)
                && !newerOutput.hasNewer && newerOutput.total == nextOutput.total,
                "Newer navigation did not reproduce the bounded snapshot")
    let interrupted = HistoryStore.parse(data: try records([firstContext, secondContext, secondFinished]), stamp: "output", audit: databaseURL)
    try require(try HistoryStore.output(for: interrupted.entries[0]).total == 1,
                "An incomplete earlier batch leaked later output")
    let missingBoundary = try parsed([firstContext, completed])
    try require(missingBoundary.entries[0].finished == nil && missingBoundary.entries[0].needsAttention,
                "Missing terminal output boundary appeared complete")
    try require(try Data(contentsOf: databaseURL) == outputBytes, "Output view mutated the history database")
    try MainActor.assumeIsolated { try ProcessingHistoryPanel(directory: directory).validateOutputForSelfTest() }
    try require(try Data(contentsOf: databaseURL) == outputBytes, "Output action mutated the history database")
    let batchIssues = try HistoryStore.batchIssues(for: outputEntries[1])
    try require(batchIssues.scannedRows == 1_002 && batchIssues.occurrences == 999 && batchIssues.groups.count == 3
                && batchIssues.start == 1 && batchIssues.upper == 1_003 && batchIssues.endSource == "terminal_boundary"
                && batchIssues.groups[0].count == 997 && batchIssues.groups[0].phase == "processing"
                && batchIssues.groups[0].firstSequence == 3 && batchIssues.groups[0].lastSequence == 999
                && batchIssues.groups[0].firstOriginal.contains("[WARN] /synthetic/item.jpg: warning 42")
                && batchIssues.groups[0].lastOriginal.contains("ERR: \u{001B}[33m")
                && batchIssues.groups[1].count == 1 && batchIssues.groups[1].phase == "verification"
                && batchIssues.groups[2].pipeline == "VID" && batchIssues.groups[2].phase == nil
                && batchIssues.groups[2].severity == "failure",
                "Whole batch issues lost earlier-page phases, original rows, exact occurrences, or pipeline/severity boundaries")
    let firstBatchIssues = try HistoryStore.batchIssues(for: outputEntries[0])
    try require(firstBatchIssues.scannedRows == 1 && firstBatchIssues.groups.isEmpty && firstBatchIssues.occurrences == 0,
                "Whole batch issues leaked another batch or guessed failure from ordinary output")
    let cancelledScan = HistoryStore.IssueScanControl()
    cancelledScan.cancel()
    do {
        _ = try HistoryStore.batchIssues(for: outputEntries[1], control: cancelledScan)
        throw HostError(message: "Cancelled issue scan returned counts")
    } catch {
        try require(error.localizedDescription == localized("history.batch_issues.cancelled"), "Issue cancellation hid its explicit reason")
    }
    try require(try Data(contentsOf: databaseURL) == outputBytes, "Whole batch issues mutated the history database")
    var fixtureDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &fixtureDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let fixtureDB else {
        throw HostError(message: "Synthetic byte-budget fixture open failed")
    }
    let byteSQL = """
    UPDATE history_output SET line=CAST(zeroblob(\(HistoryStore.maximumOutputBytes / 2)) AS TEXT) WHERE sequence IN (1001,1002);
    UPDATE history_output SET line=CAST(zeroblob(\(HistoryStore.maximumOutputBytes + 1)) AS TEXT) WHERE sequence=1003;
    UPDATE history_output SET line=x'ff' WHERE sequence=1000;
    """
    let byteResult = sqlite3_exec(fixtureDB, byteSQL, nil, nil, nil)
    sqlite3_close(fixtureDB)
    try require(byteResult == SQLITE_OK, "Synthetic byte-budget fixture insert failed")
    do {
        _ = try HistoryStore.batchIssues(for: outputEntries[1])
        throw HostError(message: "Invalid or oversized whole-batch output returned partial counts")
    } catch {
        try require(error.localizedDescription != "Invalid or oversized whole-batch output returned partial counts",
                    "Invalid whole-batch output appeared complete")
    }
    let byteSnapshot = try Data(contentsOf: databaseURL)
    let bytePage = try HistoryStore.output(for: outputEntries[1])
    try require(bytePage.text.utf8.count <= HistoryStore.maximumOutputBytes && !bytePage.issues.isEmpty
                && bytePage.rows.last?.sequence == 1003 && bytePage.hasOlder,
                "Oversized first row did not produce an explicit, bounded, navigable issue")
    let byteOlder = try HistoryStore.output(for: outputEntries[1], cursor: bytePage.rows.first!.sequence,
                                          direction: .older, upperSnapshot: bytePage.upper)
    try require(!byteOlder.rows.isEmpty && byteOlder.rows.last!.sequence < bytePage.rows.first!.sequence
                && byteOlder.text.utf8.count <= HistoryStore.maximumOutputBytes && !byteOlder.issues.isEmpty,
                "Byte-limited output navigation did not make forward progress")
    try require(try Data(contentsOf: databaseURL) == byteSnapshot, "Byte-budget navigation mutated the database")
    var ongoing = outputEntries[1]
    ongoing.finished = nil
    ongoing.outputUpperSequence = nil
    let ongoingPage = try HistoryStore.output(for: ongoing)
    var appendDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &appendDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let appendDB else {
        throw HostError(message: "Synthetic snapshot fixture open failed")
    }
    let appended = sqlite3_exec(appendDB, "INSERT INTO history_output(session_id, ts, pipeline, line) VALUES ('output', 'synthetic', 'IMG', 'appended after snapshot')", nil, nil, nil)
    sqlite3_close(appendDB)
    try require(appended == SQLITE_OK, "Synthetic snapshot append failed")
    let frozenPage = try HistoryStore.output(for: ongoing, upperSnapshot: ongoingPage.upper)
    let refreshedPage = try HistoryStore.output(for: ongoing)
    try require(frozenPage.total == ongoingPage.total && frozenPage.upper == ongoingPage.upper
                && frozenPage.rows.map(\.sequence) == ongoingPage.rows.map(\.sequence)
                && !frozenPage.text.contains("appended after snapshot")
                && refreshedPage.total == ongoingPage.total + 1 && refreshedPage.text.contains("appended after snapshot"),
                "Ongoing output snapshot shifted after an append")
    var emptyOutput = outputEntries[0]
    emptyOutput.finished = nil
    emptyOutput.outputUpperSequence = 0
    let emptyPage = try HistoryStore.output(for: emptyOutput)
    try require(emptyPage.rows.isEmpty && emptyPage.total == 0 && emptyPage.firstOrdinal == 0
                && !emptyPage.hasOlder && !emptyPage.hasNewer && emptyPage.issues.isEmpty,
                "Empty output invented a range, issue, or navigation")
    func refuseExport(_ operation: () throws -> Void, _ message: String) throws {
        do { try operation() } catch { return }
        throw HostError(message: message)
    }
    try writeDatabase([("issue_limit", [firstContext])], schema: 2)
    var limitDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &limitDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let limitDB else {
        throw HostError(message: "Synthetic issue limit database open failed")
    }
    let issueLimitSQL = """
    WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM rows WHERE n<4097)
    INSERT INTO history_output(session_id, ts, pipeline, line)
    SELECT 'issue_limit', 'synthetic', 'IMG', '[WARN] /synthetic/item.jpg: exact value ' || n FROM rows;
    """
    guard sqlite3_exec(limitDB, issueLimitSQL, nil, nil, nil) == SQLITE_OK else {
        sqlite3_close(limitDB)
        throw HostError(message: "Synthetic issue limit fixture insert failed")
    }
    let limitEntry = try HistoryStore.load(directory: directory).entries.first { $0.stamp == "issue_limit" }!
    try refuseExport({ _ = try HistoryStore.batchIssues(for: limitEntry) }, "Excessive issue groups returned partial counts")
    guard sqlite3_exec(limitDB, "UPDATE history_output SET line='synthetic ordinary line'; WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM rows WHERE n<\(HistoryStore.maximumRecordsPerSession - 4096)) INSERT INTO history_output(session_id, ts, pipeline, line) SELECT 'issue_limit', 'synthetic', 'IMG', 'synthetic ordinary line' FROM rows", nil, nil, nil) == SQLITE_OK else {
        sqlite3_close(limitDB)
        throw HostError(message: "Synthetic issue row limit fixture insert failed")
    }
    sqlite3_close(limitDB)
    let limitBytes = try Data(contentsOf: databaseURL)
    try refuseExport({ _ = try HistoryStore.batchIssues(for: limitEntry) }, "Excessive ordinary rows appeared as zero complete issues")
    try require(try Data(contentsOf: databaseURL) == limitBytes, "Refused issue scans mutated their source")
    let exportConfig = try payload("MFB_HISTORY_CONFIG", ["schema_version": 1, "pipeline": "img", "package_version": "0.12.0", "invocation_id": "synthetic-call-1",
        "config": ["config_version": 1, "execution": ["error_mode": "log-and-continue"]],
        "sources": ["config_version": "default", "execution.error_mode": "CLI"]])
    var exportContextValue = outputContext
    exportContextValue["output_sequence"] = 2
    let exportContext = try payload("MFB_HISTORY_CONTEXT", exportContextValue)
    exportContextValue["output_sequence"] = 6
    let adjacentContext = try payload("MFB_HISTORY_CONTEXT", exportContextValue)
    var unknownSummaryValue = summaryValue
    unknownSummaryValue["img"] = ["active": true, "succeeded": NSNull(), "skipped": NSNull(), "failed": NSNull(),
        "ignored": NSNull(), "unprocessed": NSNull(), "exit_code": NSNull()]
    let unknownSummary = try payload("MFB_HISTORY_SUMMARY", unknownSummaryValue)
    let exportFinished = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "failed",
        "error": "synthetic terminal failure", "output_sequence": 6])
    try writeDatabase([("export", [exportContext, launch, exportConfig, unknownSummary, exportFinished, adjacentContext, secondLaunch])], schema: 2)
    var exportDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &exportDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let exportDB else {
        throw HostError(message: "Synthetic export database open failed")
    }
    let warningLine = "ERR: \u{001B}[33m[WARN] /synthetic/one.jpg: exact warning 1\u{001B}[0m"
    let errorLine = "[FAIL] /synthetic/two.jpg: exact error 2"
    var outputInsert: OpaquePointer?
    guard sqlite3_prepare_v2(exportDB, "INSERT INTO history_output(session_id, ts, pipeline, line) VALUES ('export', 'synthetic timestamp', 'IMG', ?)", -1,
                            &outputInsert, nil) == SQLITE_OK, let insertOutput = outputInsert else {
        throw HostError(message: "Synthetic export output prepare failed")
    }
    for line in ["[FAIL] prior batch must not leak", processingPhase, processingPhase, warningLine,
                 "MFB_PROGRESS={\"ignored\":true}", errorLine, "[FAIL] adjacent batch must not leak"] {
        sqlite3_reset(insertOutput)
        sqlite3_clear_bindings(insertOutput)
        guard line.withCString({ sqlite3_bind_text(insertOutput, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
              sqlite3_step(insertOutput) == SQLITE_DONE else { throw HostError(message: "Synthetic export output insert failed") }
    }
    sqlite3_finalize(insertOutput)
    sqlite3_close(exportDB)
    let exportEntries = try HistoryStore.load(directory: directory).entries.filter { $0.stamp == "export" }
        .sorted { ($0.context?.output_sequence ?? -1) < ($1.context?.output_sequence ?? -1) }
    try require(exportEntries.count == 2, "Synthetic export batches merged")
    var unknownFinishedBoundary = exportEntries[0]
    unknownFinishedBoundary.finished = HistoryFinished(schema_version: 1, outcome: "failed", error: "synthetic failure", output_sequence: nil)
    unknownFinishedBoundary.outputUpperSequence = 6
    try require(unknownFinishedBoundary.outputEndSource == "next_batch_boundary",
                "A finished record without an output sequence fabricated terminal boundary provenance")
    unknownFinishedBoundary.outputUpperSequence = nil
    try require(unknownFinishedBoundary.outputEndSource == "snapshot_only",
                "An unknown finished boundary was promoted to an authoritative end sequence")
    let unidentifiedExport = HistoryEntry(id: "export:0", stamp: "export", audit: databaseURL, timestamp: "synthetic")
    try refuseExport({ _ = try HistoryStore.diagnosticArtifact(for: unidentifiedExport) },
                     "Diagnostic export guessed a batch without stored context and output boundaries")
    let databaseBeforeExport = try Data(contentsOf: databaseURL)
    let exportIssues = try HistoryStore.batchIssues(for: exportEntries[0])
    let ongoingIssues = try HistoryStore.batchIssues(for: exportEntries[1])
    try require(exportIssues.scannedRows == 4 && exportIssues.occurrences == 2
                && exportIssues.groups.map(\.phase) == ["processing", "processing"]
                && exportIssues.start == 2 && exportIssues.upper == 6 && exportIssues.endSource == "terminal_boundary"
                && ongoingIssues.scannedRows == 1 && ongoingIssues.occurrences == 1
                && ongoingIssues.endSource == "snapshot_only" && ongoingIssues.groups.first?.phase == nil,
                "Batch issue snapshots leaked adjacent batches or invented final boundaries and earlier phases")
    let artifact = try HistoryStore.diagnosticArtifact(for: exportEntries[0])
    guard let document = try JSONSerialization.jsonObject(with: artifact) as? [String: Any],
          let scope = document["scope"] as? [String: Any], let exportedSummary = document["summary"] as? [String: Any],
          let exportedImage = exportedSummary["img"] as? [String: Any],
          let exportedConfig = document["configuration"] as? [String: Any],
          let snapshots = exportedConfig["snapshots"] as? [[String: Any]],
          let workerLaunches = document["worker_launches"] as? [[String: Any]],
          let exportedRows = document["records"] as? [[String: Any]] else { throw HostError(message: "Diagnostic artifact schema is invalid") }
    try require(document["schema_version"] as? Int == 1 && document["authoritative_state"] as? Bool == false
                && document["full_run_manifest"] as? Bool == false && scope["total_output_rows"] as? Int == 4
                && scope["retained_output_rows"] as? Int == 3 && scope["diagnostic_log_rows"] as? Int == 2
                && scope["phase_control_rows"] as? Int == 1 && scope["omitted_non_diagnostic_output_rows"] as? Int == 1
                && scope["end_source"] as? String == "terminal_boundary"
                && exportedImage["succeeded"] is NSNull && snapshots.count == 1
                && workerLaunches.count == 1 && workerLaunches[0]["invocation_id"] as? String == "synthetic-call-1"
                && workerLaunches[0]["arguments"] as? [String] == launchValue["arguments"] as? [String]
                && (workerLaunches[0]["configuration"] as? [String: Any])?["package_version"] as? String == "0.12.0"
                && (snapshots[0]["sources"] as? [String: String])?["execution.error_mode"] == "CLI"
                && exportedRows.map({ $0["sequence"] as? Int }) == [3, 4, 6]
                && exportedRows[1]["line"] as? String == warningLine && exportedRows[2]["line"] as? String == errorLine
                && exportedRows[1]["phase"] as? String == "processing"
                && !String(decoding: artifact, as: UTF8.self).contains("must not leak"),
                "Diagnostic export mixed batches, inferred unknown counts, dropped exact messages, or misstated exclusions")
    let ongoingArtifact = try JSONSerialization.jsonObject(with: HistoryStore.diagnosticArtifact(for: exportEntries[1])) as? [String: Any]
    let ongoingScope = ongoingArtifact?["scope"] as? [String: Any]
    let ongoingRows = ongoingArtifact?["records"] as? [[String: Any]]
    let ongoingLaunch = (ongoingArtifact?["worker_launches"] as? [[String: Any]])?.first
    try require(ongoingArtifact?["summary"] is NSNull && ongoingArtifact?["finished"] is NSNull
                && ongoingScope?["known_end_sequence"] is NSNull && ongoingScope?["end_source"] as? String == "snapshot_only"
                && ongoingLaunch?["program_path"] is NSNull && ongoingLaunch?["configuration"] is NSNull
                && (ongoingLaunch?["binary"] as? [String: Any])?["blake3"] is NSNull
                && ongoingRows?.first?["phase"] is NSNull,
                "Ongoing diagnostic export invented a final state, counts, or phase")
    try require(try Data(contentsOf: databaseURL) == databaseBeforeExport, "Diagnostic export mutated its source database")
    let destination = directory.appendingPathComponent("diagnostic.json")
    let staged = try HistoryDiagnosticStaging.prepare(artifact, destination: destination)
    try require(!FileManager.default.fileExists(atPath: destination.path), "Staging exposed a partial diagnostic artifact")
    try staged.publish()
    let exportedPermissions = try FileManager.default.attributesOfItem(atPath: destination.path)[.posixPermissions] as? NSNumber
    try require(try Data(contentsOf: destination) == artifact && exportedPermissions?.intValue == 0o600,
                "Diagnostic publication changed the artifact or exposed it to other users")
    try refuseExport({ _ = try HistoryDiagnosticStaging.prepare(artifact, destination: destination) }, "Diagnostic export replaced an existing file")
    let linkDestination = directory.appendingPathComponent("diagnostic-link.json")
    try FileManager.default.createSymbolicLink(at: linkDestination, withDestinationURL: destination)
    try refuseExport({ _ = try HistoryDiagnosticStaging.prepare(artifact, destination: linkDestination) }, "Diagnostic export followed an existing link")
    let raceDestination = directory.appendingPathComponent("diagnostic-race.json")
    let racing = try HistoryDiagnosticStaging.prepare(artifact, destination: raceDestination)
    let sentinel = Data("synthetic destination preserved".utf8)
    try sentinel.write(to: raceDestination)
    try refuseExport({ try racing.publish() }, "Diagnostic export clobbered a file created after staging")
    try racing.discard()
    try require(try Data(contentsOf: raceDestination) == sentinel && Data(contentsOf: destination) == artifact,
                "Refused publication changed an existing destination")
    let gitFolder = directory.appendingPathComponent("synthetic-worktree")
    try FileManager.default.createDirectory(at: gitFolder, withIntermediateDirectories: false)
    try sentinel.write(to: gitFolder.appendingPathComponent(".git"))
    try refuseExport({ _ = try HistoryDiagnosticStaging.prepare(artifact, destination: gitFolder.appendingPathComponent("private.json")) },
                     "Diagnostic export wrote private state into a Git worktree")
    let alias = directory.appendingPathComponent("workspace-alias")
    try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: gitFolder)
    try refuseExport({ _ = try HistoryDiagnosticStaging.prepare(artifact, destination: alias.appendingPathComponent("private.json")) },
                     "A directory alias bypassed the Git diagnostic export restriction")
    let lateGitFolder = directory.appendingPathComponent("late-worktree")
    try FileManager.default.createDirectory(at: lateGitFolder, withIntermediateDirectories: false)
    let lateGit = try HistoryDiagnosticStaging.prepare(artifact, destination: lateGitFolder.appendingPathComponent("private.json"))
    try sentinel.write(to: lateGitFolder.appendingPathComponent(".git"))
    try refuseExport({ try lateGit.publish() }, "A new Git worktree bypassed the final publication check")
    try lateGit.discard()
    var overlappingContextValue = exportContextValue
    overlappingContextValue["output_sequence"] = 4
    let overlappingContext = try payload("MFB_HISTORY_CONTEXT", overlappingContextValue)
    var boundaryDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &boundaryDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let boundaryDB else {
        throw HostError(message: "Synthetic export boundary open failed")
    }
    var boundaryUpdate: OpaquePointer?
    guard sqlite3_prepare_v2(boundaryDB, "UPDATE history_events SET event=? WHERE session_id='export' AND event=?", -1,
                            &boundaryUpdate, nil) == SQLITE_OK, let boundaryUpdate else {
        throw HostError(message: "Synthetic export boundary prepare failed")
    }
    for (replacement, previous) in [(overlappingContext, adjacentContext), (adjacentContext, overlappingContext)] {
        sqlite3_reset(boundaryUpdate)
        sqlite3_clear_bindings(boundaryUpdate)
        guard replacement.withCString({ sqlite3_bind_text(boundaryUpdate, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
              previous.withCString({ sqlite3_bind_text(boundaryUpdate, 2, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
              sqlite3_step(boundaryUpdate) == SQLITE_DONE, sqlite3_changes(boundaryDB) == 1 else {
            throw HostError(message: "Synthetic export boundary update failed")
        }
        if replacement == overlappingContext {
            let overlapping = try HistoryStore.load(directory: directory).entries.first { $0.id == exportEntries[0].id }
            try require(overlapping?.issues.isEmpty == true && overlapping?.finished?.output_sequence == 6
                        && overlapping?.outputUpperSequence == 4,
                        "Overlapping export fixture did not isolate the contradictory batch boundary")
            try refuseExport({ _ = try HistoryStore.diagnosticArtifact(for: exportEntries[0]) },
                             "Diagnostic export included output beyond the next batch boundary")
            try refuseExport({ _ = try HistoryStore.batchIssues(for: exportEntries[0]) },
                             "Batch issues included output beyond a conflicting next-batch boundary")
        }
    }
    sqlite3_finalize(boundaryUpdate)
    sqlite3_close(boundaryDB)
    var activityExport: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &activityExport, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let activityExport else {
        throw HostError(message: "Synthetic Gate activity export open failed")
    }
    defer { sqlite3_close(activityExport) }
    guard sqlite3_exec(activityExport, #"UPDATE history_output SET line='MFB_GATE_ACTIVITY={"schema_version":1,"stage_id":"export-gate","gate":"local","check":"metadata","completed":3,"total":6,"state":"running"}' WHERE sequence=5"#, nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic Gate activity export write failed")
    }
    let activityDocument = try JSONSerialization.jsonObject(with: HistoryStore.diagnosticArtifact(for: exportEntries[0])) as? [String: Any]
    let activityScope = activityDocument?["scope"] as? [String: Any]
    let activityRows = activityDocument?["records"] as? [[String: Any]]
    try require(activityScope?["total_output_rows"] as? Int == 4
                && activityScope?["retained_output_rows"] as? Int == 4
                && activityScope?["verification_activity_rows"] as? Int == 1
                && activityScope?["diagnostic_log_rows"] as? Int == 2
                && activityScope?["phase_control_rows"] as? Int == 1
                && activityScope?["omitted_non_diagnostic_output_rows"] as? Int == 0
                && activityRows?.contains(where: { $0["kind"] as? String == "verification_activity" && $0["sequence"] as? Int == 5 }) == true,
                "Diagnostic export discarded verification activity or counted it as a failed file")
    guard sqlite3_exec(activityExport, #"UPDATE history_output SET line='MFB_GATE_ACTIVITY={"schema_version":1,"stage_id":"export-inventory","gate":"local","check":"inventory","completed":0,"total":6,"state":"running"}' WHERE sequence=3; UPDATE history_output SET line='MFB_GATE_ACTIVITY={"schema_version":1,"stage_id":"export-inventory","gate":"local","check":"inventory","completed":0,"total":6,"state":"failed"}' WHERE sequence=5"#, nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic inventory failure export write failed")
    }
    let inventoryDocument = try JSONSerialization.jsonObject(with: HistoryStore.diagnosticArtifact(for: exportEntries[0])) as? [String: Any]
    let inventoryScope = inventoryDocument?["scope"] as? [String: Any]
    let inventoryRows = inventoryDocument?["records"] as? [[String: Any]]
    try require(inventoryScope?["total_output_rows"] as? Int == 4
                && inventoryScope?["retained_output_rows"] as? Int == 4
                && inventoryScope?["verification_activity_rows"] as? Int == 2
                && inventoryScope?["diagnostic_log_rows"] as? Int == 2
                && inventoryScope?["phase_control_rows"] as? Int == 0
                && inventoryRows?.filter({ $0["kind"] as? String == "verification_activity" }).map({ $0["sequence"] as? Int }) == [3, 5],
                "Diagnostic export rejected a valid zero-check inventory failure or fabricated file counts")
    guard sqlite3_exec(activityExport, #"UPDATE history_output SET line='MFB_LOG_PHASE={"schema_version":1,"phase":"processing"}' WHERE sequence=3"#, nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic export phase restoration failed")
    }
    guard sqlite3_exec(activityExport, "UPDATE history_output SET line='MFB_GATE_ACTIVITY={invalid}' WHERE sequence=5", nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic invalid Gate activity write failed")
    }
    try refuseExport({ _ = try HistoryStore.diagnosticArtifact(for: exportEntries[0]) },
                     "Invalid Gate activity evidence exported a misleading artifact")
    guard sqlite3_exec(activityExport, "UPDATE history_output SET line='ordinary output omitted' WHERE sequence=5", nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic Gate activity restoration failed")
    }
    var corruptExport: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &corruptExport, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let corruptExport else {
        throw HostError(message: "Synthetic export corruption open failed")
    }
    guard sqlite3_exec(corruptExport, "UPDATE history_output SET line='MFB_LOG_PHASE={invalid}' WHERE sequence=3", nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic export corruption write failed")
    }
    sqlite3_close(corruptExport)
    try refuseExport({ _ = try HistoryDiagnosticStaging.prepare(HistoryStore.diagnosticArtifact(for: exportEntries[0]), destination: destination) },
                     "Invalid phase evidence exported a misleading diagnostic artifact")
    try refuseExport({ _ = try HistoryStore.batchIssues(for: exportEntries[0]) },
                     "Invalid phase evidence invented warning counts in a complete issue scan")
    var limitedExport: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &limitedExport, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let limitedExport else {
        throw HostError(message: "Synthetic export limit open failed")
    }
    guard sqlite3_exec(limitedExport, "UPDATE history_output SET line=CAST(zeroblob(\(HistoryStore.maximumOutputBytes + 1)) AS TEXT) WHERE sequence=3", nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic export limit write failed")
    }
    sqlite3_close(limitedExport)
    try refuseExport({ _ = try HistoryStore.diagnosticArtifact(for: exportEntries[0]) }, "Oversized export evidence was silently omitted")
    try require(try Data(contentsOf: destination) == artifact
                && !FileManager.default.contentsOfDirectory(atPath: directory.path).contains(where: { $0.hasPrefix(".mfb-diagnostic-") }),
                "Failed diagnostic export altered its destination or left staging files")
    try writeDatabase([("test", [context, summary, completed])], schema: 99)
    let futureDatabase = try Data(contentsOf: databaseURL)
    try requireReadFailure("Unsupported database schema appeared successful")
    try require(try Data(contentsOf: databaseURL) == futureDatabase, "History reader migrated an unsupported schema")
    try Data("synthetic corrupt database".utf8).write(to: databaseURL)
    try requireReadFailure("Corrupt database fell back to successful legacy evidence")
    try FileManager.default.removeItem(at: databaseURL)
    _ = try HistoryStore.load(directory: directory)
    try require(!FileManager.default.fileExists(atPath: databaseURL.path), "History reader created an absent database")
}
