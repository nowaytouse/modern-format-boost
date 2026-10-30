import AppKit
import Foundation
import SQLite3
import Darwin
import CoreFoundation
import UniformTypeIdentifiers

enum HistoryLayout {
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
    let run_id: String?
}

func validHistoryIdentity(_ value: String) -> Bool {
    !value.isEmpty && value.utf8.count <= 256 && value.utf8.allSatisfy { (33...126).contains($0) }
}

private enum HistoryFileFailureError: Error { case invalid }

struct HistoryFileFailure: Codable, Hashable {
    let schema_version: Int
    let pipeline: String
    let source_path: String
    let absolute_path: String?
    let message: String
    let run_id: String
    let invocation_id: String
    let reason_code: String?

    var userReason: String {
        switch reason_code {
        case "jpeg_lossless_encode_unavailable": localized("history.file_failure.reason.jpeg_lossless_encode_unavailable")
        case "jpeg_missing_eoi": localized("history.file_failure.reason.jpeg_missing_eoi")
        default: message + (reason_code.map { " [\($0)]" } ?? "")
        }
    }

    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.schema_version == rhs.schema_version && lhs.pipeline == rhs.pipeline
            && lhs.run_id == rhs.run_id && lhs.invocation_id == rhs.invocation_id
            && lhs.reason_code == rhs.reason_code
            && lhs.source_path.utf8.elementsEqual(rhs.source_path.utf8)
            && lhs.absolute_path.map { Data($0.utf8) } == rhs.absolute_path.map { Data($0.utf8) }
            && lhs.message.utf8.elementsEqual(rhs.message.utf8)
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(schema_version); hasher.combine(pipeline)
        hasher.combine(run_id); hasher.combine(invocation_id); hasher.combine(reason_code)
        hasher.combine(Data(source_path.utf8)); hasher.combine(absolute_path.map { Data($0.utf8) })
        hasher.combine(Data(message.utf8))
    }

    static let prefix = "MFB_FILE_FAILURE="

    static func decode(_ line: String) throws -> Self? {
        guard line.hasPrefix(prefix) else { return nil }
        let data = Data(line.dropFirst(prefix.count).utf8)
        guard data.count <= 64 * 1024 - prefix.utf8.count - 1 else { throw HistoryFileFailureError.invalid }
        guard let fields = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(fields.keys).isSubset(of: ["schema_version", "pipeline", "source_path", "absolute_path", "message", "run_id", "invocation_id", "reason_code"])
        else { throw HistoryFileFailureError.invalid }
        let value = try JSONDecoder().decode(Self.self, from: data)
        guard value.schema_version == 1, ["img", "vid"].contains(value.pipeline),
              validHistoryIdentity(value.run_id), validHistoryIdentity(value.invocation_id),
              !value.source_path.isEmpty, !value.source_path.contains("\0"),
              !value.message.isEmpty, !value.message.contains("\0"),
              value.absolute_path.map({ $0.hasPrefix("/") && !$0.contains("\0") }) ?? true,
              value.reason_code.map(validHistoryIdentity) ?? true else { throw HistoryFileFailureError.invalid }
        return value
    }

    fileprivate static func parse(_ line: String, context: HistoryContext, invocations: [HistoryInvocation], pipeline: String) throws -> Self? {
        guard let value = try decode(line) else { return nil }
        guard value.pipeline == pipeline.lowercased(), value.run_id == context.run_id,
              invocations.contains(where: { $0.invocation_id == value.invocation_id && $0.pipeline == value.pipeline && $0.run_id == value.run_id })
        else { throw HistoryFileFailureError.invalid }
        return value
    }
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

struct FastImgDeliveryReceipt: Codable, Equatable {
    static let prefix = "MFB_FAST_IMG_RESULT="
    static let historyPrefix = "MFB_HISTORY_FAST_IMG_RESULT="
    let schemaVersion: Int
    let succeeded: Int
    let skipped: Int
    let ignored: Int
    let failed: Int
    let unprocessed: Int
    let encoded: Int
    let photosVerified: Int
    let sourceRetained: Int?
    let failedSourcesRetained: Int?
    let failedSourcesVerified: Int?
    let failedSourceVerificationError: String?
    let retentionUnknownReason: String?
    let primaryCleanupComplete: Bool
    let finalizationError: String?
    let photosDeliveryDeferred: Bool?
    let primaryVerificationComplete: Bool?
    let sourceDirectory: String?
    let outputDirectory: String?

    var counts: [Int?] { [succeeded, skipped, failed, ignored] }
    var isDeferred: Bool { photosDeliveryDeferred == true }
    var isValid: Bool {
        guard schemaVersion == 1,
              let total = processingCountTotal(counts + [unprocessed]),
              let capacity = processingCountTotal([succeeded, unprocessed]),
              encoded >= 0, encoded <= capacity, photosVerified >= 0, photosVerified <= capacity else { return false }
        if let failedSourcesRetained, !(0...failed).contains(failedSourcesRetained) { return false }
        if let failedSourcesVerified, !(0...failed).contains(failedSourcesVerified) { return false }
        if let sourceRetained {
            guard sourceRetained >= 0, sourceRetained <= total, retentionUnknownReason == nil,
                  failedSourcesRetained.map({ $0 <= sourceRetained }) != false else { return false }
        } else if retentionUnknownReason?.isEmpty != false { return false }
        if isDeferred {
            guard primaryVerificationComplete == true, unprocessed > 0, !primaryCleanupComplete,
                  finalizationError?.isEmpty == false,
                  let sourceDirectory, let outputDirectory,
                  Self.absolute(sourceDirectory), Self.absolute(outputDirectory) else { return false }
        }
        return true
    }
    static func absolute(_ path: String) -> Bool { path.hasPrefix("/") && !path.utf8.contains(0) }
    static func samePath(_ left: String?, _ right: String?) -> Bool {
        guard let left, let right else { return false }
        return Data(left.utf8) == Data(right.utf8)
    }
    static func decode(_ line: String, prefix: String = FastImgDeliveryReceipt.prefix) throws -> Self? {
        guard line.hasPrefix(prefix) else { return nil }
        let data = Data(line.dropFirst(prefix.count).utf8)
        guard data.count <= 32_768,
              let fields = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              ["source_retained", "failed_sources_retained", "failed_sources_verified", "failed_source_verification_error",
               "retention_unknown_reason", "finalization_error"].allSatisfy({ fields[$0] != nil }) else {
            throw HostError(message: localized("result.delivery_invalid"))
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let receipt = try decoder.decode(Self.self, from: data)
        guard receipt.isValid else { throw HostError(message: localized("result.delivery_invalid")) }
        return receipt
    }
    var deferredDetails: String {
        localized("result.delivery_deferred_facts", String(encoded), String(photosVerified)) + "\n"
            + localized("result.delivery_recorded_output", outputDirectory ?? localized("result.unknown")) + "\n"
            + localized("result.delivery_recorded_note")
    }
}

struct OutputCleanupReceipt: Decodable, Equatable {
    static let prefix = "MFB_OUTPUT_CLEANUP="
    static let historyPrefix = "MFB_HISTORY_OUTPUT_CLEANUP="
    let schema_version: Int
    let run_id: String?
    let output_path: String
    let outcome: String
    let removed_files: Int?
    let already_absent_files: Int?
    let pruned_directories: Int?
    let error: String?

    static func decode(_ line: String, prefix: String = OutputCleanupReceipt.prefix) throws -> Self? {
        guard line.hasPrefix(prefix) else { return nil }
        let data = Data(line.dropFirst(prefix.count).utf8)
        let fields: Set<String> = ["schema_version", "run_id", "output_path", "outcome",
            "removed_files", "already_absent_files", "pruned_directories", "error"]
        guard line.utf8.count < 64 * 1_024,
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(object.keys).isSubset(of: fields),
              ["schema_version", "output_path", "outcome", "error"].allSatisfy({ object[$0] != nil })
        else { throw HostError(message: localized("cleanup.invalid")) }
        let receipt = try JSONDecoder().decode(Self.self, from: data)
        guard receipt.valid else { throw HostError(message: localized("cleanup.invalid")) }
        return receipt
    }

    var valid: Bool {
        guard schema_version == 1, output_path.hasPrefix("/"), !output_path.utf8.contains(0),
              run_id.map(validHistoryIdentity) ?? true else { return false }
        let counts = [removed_files, already_absent_files, pruned_directories]
        let measured = counts.compactMap { $0 }
        guard measured.allSatisfy({ $0 >= 0 }),
              measured.isEmpty || processingCountTotal(measured.map(Optional.some)) != nil else { return false }
        switch outcome {
        case "removed", "residual": return counts.allSatisfy({ $0 != nil }) && error == nil
        case "already_absent", "not_requested", "verification_blocked":
            return counts.allSatisfy({ $0 == nil }) && error == nil
        case "failed": return error?.isEmpty == false
        default: return false
        }
    }

    var needsAttention: Bool { ["residual", "failed", "verification_blocked"].contains(outcome) }
    var statusText: String { localized("cleanup.outcome.\(outcome)") }
    var detailText: String {
        let unknown = localized("cleanup.not_measured")
        var lines = [statusText, output_path,
            localized("cleanup.counts", removed_files.map(String.init) ?? unknown,
                already_absent_files.map(String.init) ?? unknown, pruned_directories.map(String.init) ?? unknown)]
        if let error { lines.append(error) }
        lines.append(localized("cleanup.scope"))
        return lines.joined(separator: "\n")
    }

    var recordedObject: [String: Any] {
        ["schema_version": schema_version, "run_id": run_id as Any? ?? NSNull(),
         "output_path": output_path, "outcome": outcome, "removed_files": removed_files as Any? ?? NSNull(),
         "already_absent_files": already_absent_files as Any? ?? NSNull(),
         "pruned_directories": pruned_directories as Any? ?? NSNull(), "error": error as Any? ?? NSNull()]
    }
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
    var failed_sources_verified: Int? = nil
    var primary_cleanup_complete: Bool? = nil

    var valid: Bool {
        schema_version == 1 && [issue_count, source_count, optimized_count, skipped_count, failed_count,
                               source_remaining_count, verified_deleted_count].allSatisfy { $0 >= 0 }
            && (failed_sources_verified.map { $0 >= 0 && $0 <= failed_count } ?? true)
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
    let run_id: String?
    let pipeline: String
    let state: String
    let program_requested: String
    let program_path: String?
    let arguments: [String]
    let binary: Binary

    var valid: Bool {
        guard schema_version == 1, !invocation_id.isEmpty, invocation_id.utf8.count <= 256,
              run_id.map(validHistoryIdentity) ?? true,
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

struct HistoryWorkerExecution: Codable, Equatable {
    let schema_version: Int
    let scope: String
    let invocation_id: String
    let run_id: String?
    let pipeline: String
    let state: String
    let pid: UInt32
    let exit_code: Int32?
    let signal: Int32?

    var valid: Bool {
        guard schema_version == 1, scope == "os_child_process", pid > 0,
              validHistoryIdentity(invocation_id), run_id.map(validHistoryIdentity) ?? true,
              ["img", "vid", "cmd"].contains(pipeline) else { return false }
        if state == "spawned" { return exit_code == nil && signal == nil }
        return state == "exited" && ((exit_code != nil && signal == nil)
            || (exit_code == nil && signal.map { $0 > 0 } == true))
    }
}

private struct HistoryOutputStorage: Codable, Equatable {
    static let prefix = "MFB_HISTORY_OUTPUT_STORAGE="
    let schema_version: Int
    let scope: String
    let run_id: String?
    let invocation_id: String
    let pipeline: String
    let state: String
    let observed_records: UInt64?
    let committed_records: UInt64
    let committed_utf8_bytes: UInt64
    let capture_complete: Bool
    let diagnostic: String?

    var valid: Bool {
        guard schema_version == 1, scope == "captured_worker_output",
              validHistoryIdentity(invocation_id), run_id.map(validHistoryIdentity) ?? true,
              ["img", "vid", "cmd"].contains(pipeline),
              observed_records.map({ $0 >= committed_records }) ?? true else { return false }
        if state == "committed" { return observed_records == committed_records && diagnostic == nil }
        return state == "failed" && diagnostic.map {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && $0.utf8.count <= 16 * 1024
        } == true
    }

    var detailText: String {
        var lines = [localized(state == "committed" ? "history.output_storage.committed" : "history.output_storage.failed"),
                     localized(capture_complete ? "history.output_storage.capture_complete" : "history.output_storage.capture_incomplete"),
                     localized("history.output_storage.counts", observed_records.map(String.init) ?? localized("result.unknown"),
                               String(committed_records), String(committed_utf8_bytes))]
        if let diagnostic { lines.append(diagnostic) }
        return lines.joined(separator: "\n")
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
    var outputCleanups: [OutputCleanupReceipt] = []
    var outputCleanupInvalid = false
    var fastDeliveries: [FastImgDeliveryReceipt] = []
    var fastDeliveryInvalid = false
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
    var workerExecutions: [String: [HistoryWorkerExecution]] = [:]
    var workerExecutionEvidenceInvalid = false
    var outputStorage: [String: HistoryOutputStorage] = [:]
    var invalidOutputStorageInvocations: Set<String> = []
    var unboundOutputStorageInvalid = false
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
    func outputStorageText(for invocation: String) -> String {
        if invocationEvidenceInvalid || invalidOutputStorageInvocations.contains(invocation) {
            return localized("history.output_storage.invalid")
        }
        return outputStorage[invocation]?.detailText ?? localized("history.output_storage.not_recorded")
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
    var hasVerificationFailure: Bool {
        (summary?.integrity.issue_count ?? 0) > 0
            || (verification?.issue_count ?? 0) > 0 || verification?.count_status?.hasPrefix("MISMATCH") == true
            || inputVerifications.values.contains {
                $0.issue_count > 0 || $0.count_status?.hasPrefix("MISMATCH") == true
            }
    }
    var hasVerificationWarnings: Bool {
        verification?.has_warnings == true || verification?.count_status?.hasPrefix("MISMATCH") == true
            || (verification?.issue_count ?? 0) > 0
            || inputVerifications.values.contains { $0.has_warnings || $0.issue_count > 0 || $0.count_status?.hasPrefix("MISMATCH") == true }
            || (summary?.integrity.issue_count ?? 0) > 0 || summary?.integrity.state == "WARNINGS"
    }
    var hasPending: Bool { (summary?.img.unprocessed ?? 0) > 0 || (summary?.vid.unprocessed ?? 0) > 0 }
    var deliveryIsDeferred: Bool {
        guard !legacy, issues.isEmpty, contradictoryFields.isEmpty, !fastDeliveryInvalid,
              !workerExecutionEvidenceInvalid, !invocationEvidenceInvalid,
              workerExecutions.values.allSatisfy({ $0.last?.state == "exited" && $0.last?.signal == nil && $0.last?.exit_code == 2 }),
              !hasVerificationFailure, context?.dry_run == false, context?.mode == "fast-img",
              context?.inputs.count == 1, inputPlan == nil,
              context?.shortest_path == true, let summary, summary.valid, summary.img.complete,
              summary.integrity.state == nil || ["CLEAN", "WARNINGS"].contains(summary.integrity.state!),
              !summary.vid.active, summary.img.exit_code == 2, fastDeliveries.count == 1,
              fastDeliveries.contains(where: \.isDeferred),
              finished?.outcome == "failed", finished?.error == "fast-img exited with code 2",
              summary.img.unprocessed == processingCountTotal(fastDeliveries.map(\.unprocessed)),
              fastDeliveries.allSatisfy({ $0.isDeferred && $0.sourceRetained != nil && $0.retentionUnknownReason == nil
                  && $0.failedSourcesVerified == $0.failed && $0.failedSourcesRetained == $0.failed
                  && $0.failedSourceVerificationError == nil }) else { return false }
        let counts = [summary.img.succeeded, summary.img.skipped, summary.img.failed, summary.img.ignored]
        return (0..<4).allSatisfy { index in counts[index] == processingCountTotal(fastDeliveries.map { $0.counts[index] }) }
    }
    var hasPartialSuccess: Bool {
        guard !legacy, issues.isEmpty, contradictoryFields.isEmpty, !hasVerificationFailure, context?.dry_run == false,
              let summary, summary.valid, summary.img.complete, summary.vid.complete, !hasPending,
              context?.mode == "fast-img", context?.inputs.count == 1, !summary.vid.active,
              summary.img.exit_code == 1, let failed = summary.img.failed, failed > 0,
              let verification, verification.issue_count == 0, verification.failed_count == failed,
              verification.failed_sources_verified == failed, verification.primary_cleanup_complete == true,
              verification.source_path == context?.inputs.first,
              let output = verification.output_path, output.hasPrefix("/"),
              context?.output.map({ $0 == output }) ?? true,
              let countStatus = verification.count_status,
              countStatus == "MATCH" || countStatus == "EXPLAINED"
                || ((countStatus.hasPrefix("MATCH (") || countStatus.hasPrefix("EXPLAINED (")) && countStatus.hasSuffix(")")),
              let finished,
              (finished.outcome == "completed" && finished.error == nil)
                || (finished.outcome == "failed" && finished.error == "fast-img exited with code 1") else { return false }
        let active = [summary.img, summary.vid].filter(\.active)
        return hasPartialProcessingSuccess(succeeded: active.map(\.succeeded), failed: active.map(\.failed),
                                           countsComplete: true)
    }
    var statusKey: String {
        if hasVerificationFailure { return "result.unconfirmed" }
        if finished?.outcome == "cancelled" { return "history.status.cancelled" }
        if deliveryIsDeferred { return "result.delivery_deferred" }
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
    var hasCleanupAttention: Bool { outputCleanupInvalid || outputCleanups.contains(where: \.needsAttention) }
    var hasOutputStorageAttention: Bool {
        unboundOutputStorageInvalid || !invalidOutputStorageInvocations.isEmpty
            || outputStorage.values.contains { $0.state == "failed" || !$0.capture_complete }
    }
    var auxiliaryAttention: String {
        [hasCleanupAttention ? localized("cleanup.attention.short") : nil,
         hasOutputStorageAttention ? localized("history.output_storage.attention") : nil]
            .compactMap { $0 }.joined(separator: " · ")
    }
    var statusLabel: String {
        localized(statusKey) + (auxiliaryAttention.isEmpty ? "" : " · " + auxiliaryAttention)
    }
    var needsAttention: Bool {
        !["history.status.completed", "history.status.preview"].contains(statusKey)
            || hasVerificationWarnings || hasCleanupAttention || hasOutputStorageAttention
    }
    var statusColor: NSColor {
        if hasVerificationFailure { return .systemRed }
        if deliveryIsDeferred { return .systemOrange }
        if hasPartialSuccess { return .systemOrange }
        if finished?.outcome == "failed" || hasFailures { return .systemRed }
        return needsAttention ? .systemOrange : .labelColor
    }
    var searchable: String {
        ([stamp, timestamp, context?.mode ?? "", modeLabel ?? "", statusLabel] + (context?.inputs ?? [])
            + [context?.output ?? "", context?.backup ?? "", finished?.error ?? ""]
            + outputCleanups.flatMap { [$0.output_path, $0.outcome, $0.statusText, $0.error ?? ""] })
            .joined(separator: " ") + " " + fastDeliveries.filter(\.isDeferred).map(\.deferredDetails).joined(separator: " ")
            + " " + outputStorage.values.map(\.detailText)
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
                + (entry.finished == nil ? 0 : 1) + entry.fastDeliveries.count
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
        let fileFailures: [HistoryFileFailure]
        let scannedRows: Int64
        let start: Int64
        let upper: Int64
        let endSource: String
        var ignoredSources: [HistoryIgnoredSource] = []
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
        var runIDs: Set<String> = []
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
                if event.hasPrefix("MFB_HISTORY_"), !event.hasPrefix("MFB_HISTORY_CONTEXT=") {
                    guard let separator = event.firstIndex(of: "="),
                          let payload = try JSONSerialization.jsonObject(with: Data(event[event.index(after: separator)...].utf8)) as? [String: Any]
                    else { throw HistoryParseError.invalid }
                    let identity = payload["run_id"]
                    if let expected = current.context?.run_id {
                        guard identity as? String == expected else { throw HistoryParseError.invalid }
                    } else if let identity, !(identity is NSNull) {
                        throw HistoryParseError.invalid
                    }
                }
                if event.hasPrefix("MFB_HISTORY_CONTEXT=") {
                    let context = try decode(HistoryContext.self, from: event, prefix: "MFB_HISTORY_CONTEXT=")
                    guard context.schema_version == 1, !context.mode.isEmpty,
                          context.output_sequence.map({ $0 >= 0 }) ?? true,
                          context.run_id.map(validHistoryIdentity) ?? true else { throw HistoryParseError.invalid }
                    if let identity = context.run_id {
                        guard runIDs.insert(identity).inserted else { throw HistoryParseError.invalid }
                    }
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
                } else if event.hasPrefix(FastImgDeliveryReceipt.historyPrefix) {
                    guard let receipt = try FastImgDeliveryReceipt.decode(event, prefix: FastImgDeliveryReceipt.historyPrefix),
                          let context = current.context, context.mode == "fast-img", !context.dry_run,
                          context.run_id != nil, !current.fastDeliveryInvalid, current.fastDeliveries.count < 64,
                          !current.fastDeliveries.contains(where: { $0.sourceDirectory == receipt.sourceDirectory }) else {
                        throw HistoryParseError.invalid
                    }
                    current.fastDeliveries.append(receipt)
                } else if event.hasPrefix(OutputCleanupReceipt.historyPrefix) {
                    guard let receipt = try OutputCleanupReceipt.decode(event, prefix: OutputCleanupReceipt.historyPrefix),
                          let context = current.context, context.mode == "fast-img", !context.dry_run,
                          let runID = context.run_id, receipt.run_id == runID,
                          context.shortest_path == (receipt.outcome != "not_requested"),
                          !current.outputCleanupInvalid, current.outputCleanups.count < 64 else { throw HistoryParseError.invalid }
                    if let previous = current.outputCleanups.first(where: { Data($0.output_path.utf8) == Data(receipt.output_path.utf8) }) {
                        guard previous == receipt else { throw HistoryParseError.invalid }
                    } else { current.outputCleanups.append(receipt) }
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
                } else if event.hasPrefix("MFB_HISTORY_WORKER_EXECUTION=") {
                    let observation = try decode(HistoryWorkerExecution.self, from: event, prefix: "MFB_HISTORY_WORKER_EXECUTION=")
                    guard current.finished == nil, !current.workerExecutionEvidenceInvalid, !current.invocationEvidenceInvalid,
                          observation.valid,
                          let invocation = current.invocations.first(where: { $0.invocation_id == observation.invocation_id }),
                          observation.pipeline == invocation.pipeline, observation.run_id == invocation.run_id else {
                        throw HistoryParseError.invalid
                    }
                    let previous = current.workerExecutions[observation.invocation_id] ?? []
                    if observation.state == "spawned" {
                        guard previous.isEmpty else { throw HistoryParseError.invalid }
                    } else {
                        guard previous.count == 1, previous[0].state == "spawned", previous[0].pid == observation.pid else {
                            throw HistoryParseError.invalid
                        }
                    }
                    current.workerExecutions[observation.invocation_id] = previous + [observation]
                } else if event.hasPrefix(HistoryOutputStorage.prefix) {
                    let data = Data(event.dropFirst(HistoryOutputStorage.prefix.count).utf8)
                    guard data.count <= 32 * 1024,
                          let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                          Set(object.keys).isSubset(of: ["schema_version", "scope", "run_id", "invocation_id", "pipeline", "state",
                                                       "observed_records", "committed_records", "committed_utf8_bytes", "capture_complete", "diagnostic"]),
                          ["run_id", "observed_records", "diagnostic"].allSatisfy({ object[$0] != nil })
                    else { throw HistoryParseError.invalid }
                    let receipt = try decoder.decode(HistoryOutputStorage.self, from: data)
                    guard receipt.valid,
                          let invocation = current.invocations.first(where: { $0.invocation_id == receipt.invocation_id }),
                          receipt.run_id == invocation.run_id, receipt.pipeline == invocation.pipeline,
                          !current.invalidOutputStorageInvocations.contains(receipt.invocation_id),
                          current.outputStorage[receipt.invocation_id].map({ $0 == receipt }) ?? true
                    else { throw HistoryParseError.invalid }
                    current.outputStorage[receipt.invocation_id] = receipt
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
                if event.hasPrefix(FastImgDeliveryReceipt.historyPrefix) {
                    current.fastDeliveries.removeAll()
                    current.fastDeliveryInvalid = true
                }
                if event.hasPrefix(HistoryOutputStorage.prefix) {
                    let object = try? JSONSerialization.jsonObject(with: Data(event.dropFirst(HistoryOutputStorage.prefix.count).utf8)) as? [String: Any]
                    if let identity = object?["invocation_id"] as? String,
                       current.invocations.contains(where: { $0.invocation_id == identity }) {
                        current.invalidOutputStorageInvocations.insert(identity)
                    } else { current.unboundOutputStorageInvalid = true }
                    continue
                }
                if event.hasPrefix(OutputCleanupReceipt.historyPrefix) {
                    current.outputCleanups.removeAll()
                    current.outputCleanupInvalid = true
                }
                if event.hasPrefix("MFB_HISTORY_CONFIG=") { current.configEvidenceInvalid = true }
                if event.hasPrefix("MFB_HISTORY_INVOCATION=") {
                    current.invocations.removeAll()
                    current.invocationConfigs.removeAll()
                    current.invocationEvidenceInvalid = true
                    current.contradictoryFields.insert("invocation")
                }
                if event.hasPrefix("MFB_HISTORY_WORKER_EXECUTION=") || event.hasPrefix("MFB_HISTORY_INVOCATION=") {
                    current.workerExecutions.removeAll()
                    current.workerExecutionEvidenceInvalid = true
                    current.contradictoryFields.insert("worker_execution")
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
        for index in entries.indices {
            entries[index].aggregateInputVerification()
            let entry = entries[index]
            if entry.fastDeliveries.contains(where: { receipt in
                guard let source = receipt.sourceDirectory, let output = receipt.outputDirectory else { return true }
                if let plan = entry.inputPlan {
                    return !plan.inputs.contains(where: { FastImgDeliveryReceipt.samePath($0.source, source)
                        && FastImgDeliveryReceipt.samePath($0.output, output) })
                }
                return entry.context?.inputs.count != 1
                    || !FastImgDeliveryReceipt.samePath(entry.context?.inputs.first, source)
                    || entry.context?.output.map({ !FastImgDeliveryReceipt.samePath($0, output) }) == true
            }) {
                entries[index].fastDeliveries.removeAll()
                entries[index].fastDeliveryInvalid = true
                entries[index].issues.append(localized("result.delivery_invalid"))
            }
            let planOutputs = entries[index].inputPlan?.inputs.compactMap(\.output)
            let contextOutput = entries[index].context?.output
            let verifiedOutput = entries[index].verification?.output_path
            let outputs = planOutputs ?? [verifiedOutput ?? contextOutput].compactMap { $0 }
            let conflictingOutputs: Bool
            if planOutputs == nil, let contextOutput, let verifiedOutput {
                conflictingOutputs = Data(contextOutput.utf8) != Data(verifiedOutput.utf8)
            } else { conflictingOutputs = false }
            if entries[index].outputCleanups.contains(where: { receipt in
                conflictingOutputs || !outputs.contains(where: { Data($0.utf8) == Data(receipt.output_path.utf8) })
            }) {
                entries[index].outputCleanups.removeAll()
                entries[index].outputCleanupInvalid = true
                entries[index].issues.append(localized("cleanup.invalid"))
            }
        }
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

    static func needsReadOnlyPreparation(directory: URL) throws -> Bool {
        let url = directory.appendingPathComponent("history.sqlite3")
        guard regularFile(url) else { return false }
        var database: OpaquePointer?
        defer { if let database { sqlite3_close(database) } }
        let opened = sqlite3_open_v2(try databasePath(url), &database, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil)
        if opened & 0xff == SQLITE_CANTOPEN { return true }
        guard opened == SQLITE_OK, let database else { return false }
        return sqlite3_exec(database, "PRAGMA user_version", nil, nil, nil) & 0xff == SQLITE_CANTOPEN
    }

    static func loadForDisplay(directory: URL, prepare: () throws -> Void) throws -> Result {
        if try needsReadOnlyPreparation(directory: directory) { try prepare() }
        return try load(directory: directory)
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

    static func batchIssues(for selected: HistoryEntry, control: IssueScanControl = IssueScanControl(), ignoredOnly: Bool = false) throws -> BatchIssues {
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
            if ignoredOnly {
                guard HistoryIgnoredSource.prefix.withCString({ sqlite3_bind_text(query, 4, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK else {
                    throw databaseError(database)
                }
            }
        }
        let markerFilter = ignoredOnly ? " AND substr(CAST(line AS BLOB),1,\(HistoryIgnoredSource.prefix.utf8.count))=CAST(?4 AS BLOB)" : ""
        let count = try statement(database, "SELECT COUNT(*) FROM history_output WHERE session_id=? AND sequence>? AND sequence<=?" + markerFilter)
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
        let query = try statement(database, "SELECT sequence, \(columns) FROM history_output WHERE session_id=? AND sequence>? AND sequence<=?" + markerFilter + " ORDER BY sequence")
        defer { sqlite3_finalize(query) }
        try bind(query)
        var accumulator = Output.DiagnosticAccumulator()
        var fileFailures: [HistoryFileFailure] = []
        var ignoredSources: [HistoryIgnoredSource] = []
        var failureBytes = 0
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
            do {
                if let ignored = try HistoryIgnoredSource.decode(line) {
                    guard ignored.run_id == selectedContext.run_id, ignored.pipeline == pipeline.lowercased(),
                          entry.invocations.contains(where: { $0.invocation_id == ignored.invocation_id && $0.run_id == ignored.run_id && $0.pipeline == ignored.pipeline }) else {
                        throw HistoryIgnoredSource.invalid()
                    }
                    if ignoredOnly {
                        guard ignoredSources.count < 4_096, length <= maximumOutputBytes - failureBytes else {
                            throw HostError(message: localized("history.batch_issues.limit"))
                        }
                        ignoredSources.append(ignored)
                        failureBytes += length
                    }
                } else if !ignoredOnly, let failure = try HistoryFileFailure.parse(line, context: selectedContext, invocations: entry.invocations, pipeline: pipeline) {
                    guard fileFailures.count < 4_096, length <= maximumOutputBytes - failureBytes else {
                        throw HostError(message: localized("history.batch_issues.limit"))
                    }
                    fileFailures.append(failure)
                    failureBytes += length
                }
            } catch let error as HostError { throw error }
            catch { throw HostError(message: localized("history.batch_issues.invalid")) }
            if !ignoredOnly && !line.hasPrefix(HistoryIgnoredSource.prefix) {
                let tone = LogTone.classify(line)
                accumulator.ingest(Output.Row(sequence: sequence, text: "[\(timestamp)] \(pipeline): \(line)",
                    diagnostic: tone == .warning || tone == .failure, pipeline: pipeline, line: line))
            }
            guard !accumulator.invalidPhaseSeen else { throw HostError(message: localized("history.batch_issues.invalid")) }
            guard accumulator.groups.count <= 4_096, accumulator.retainedBytes <= maximumOutputBytes - failureBytes else {
                throw HostError(message: localized("history.batch_issues.limit"))
            }
            bytes += length
            scanned += 1
            previous = sequence
        }
        try control.check()
        guard scanned == total else { throw HostError(message: localized("history.batch_issues.invalid")) }
        if ignoredOnly { ignoredSources = try HistoryIgnoredSource.reconcile(ignoredSources, expected: entry.summary?.img.ignored) }
        guard sqlite3_exec(database, "COMMIT", nil, nil, nil) == SQLITE_OK else {
            try control.check()
            throw databaseError(database)
        }
        return BatchIssues(groups: accumulator.groups, occurrences: accumulator.occurrences,
            fileFailures: fileFailures, scannedRows: scanned, start: start, upper: end, endSource: entry.outputEndSource,
            ignoredSources: ignoredSources)
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
        var fileFailureCount = 0
        var ignoredObservations: [HistoryIgnoredSource] = []
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
            do {
                if let ignored = try HistoryIgnoredSource.decode(line) {
                    guard ignored.run_id == context.run_id, ignored.pipeline == pipeline.lowercased(),
                          entry.invocations.contains(where: { $0.invocation_id == ignored.invocation_id && $0.run_id == ignored.run_id && $0.pipeline == ignored.pipeline }),
                          ignoredObservations.count < 4_096 else { throw HistoryIgnoredSource.invalid() }
                    ignoredObservations.append(ignored)
                    retained.append(["sequence": sequence, "timestamp": timestamp, "pipeline": pipeline, "line": line,
                        "kind": "ignored_source_observation", "observation": try JSONSerialization.jsonObject(with: ignored.recordedJSON())])
                    continue
                }
                if let failure = try HistoryFileFailure.parse(line, context: context, invocations: entry.invocations, pipeline: pipeline) {
                    fileFailureCount += 1
                    retained.append(["sequence": sequence, "timestamp": timestamp, "pipeline": pipeline, "line": line,
                        "kind": "file_failure_observation", "observation": try JSONSerialization.jsonObject(with: JSONEncoder().encode(failure))])
                    continue
                }
            } catch { throw HostError(message: localized("history.export.invalid")) }
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
             "source_path": nullable(value.source_path), "output_path": nullable(value.output_path),
             "failed_sources_verified": nullable(value.failed_sources_verified),
             "primary_cleanup_complete": nullable(value.primary_cleanup_complete)]
        }
        let contextObject: [String: Any] = ["schema_version": context.schema_version, "inputs": context.inputs,
            "output": nullable(context.output), "backup": nullable(context.backup), "mode": context.mode,
            "dry_run": context.dry_run, "in_place": context.in_place, "shortest_path": context.shortest_path,
            "version": nullable(context.version), "retry": nullable(context.retry), "resume": nullable(context.resume),
            "no_resume": nullable(context.no_resume), "output_sequence": nullable(context.output_sequence), "run_id": nullable(context.run_id)]
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
            object["process_observations"] = try entry.workerExecutions[invocation.invocation_id].map {
                try JSONSerialization.jsonObject(with: JSONEncoder().encode($0))
            } ?? NSNull()
            let invalidStorage = entry.invocationEvidenceInvalid || entry.invalidOutputStorageInvocations.contains(invocation.invocation_id)
            object["output_storage_state"] = invalidStorage ? "invalid" : entry.outputStorage[invocation.invocation_id]?.state ?? "not_recorded"
            if let receipt = entry.outputStorage[invocation.invocation_id] {
                guard var storage = try JSONSerialization.jsonObject(with: JSONEncoder().encode(receipt)) as? [String: Any] else {
                    throw HostError(message: localized("history.export.invalid"))
                }
                storage["run_id"] = nullable(receipt.run_id)
                storage["observed_records"] = nullable(receipt.observed_records)
                storage["diagnostic"] = nullable(receipt.diagnostic)
                object["output_storage"] = storage
            } else { object["output_storage"] = NSNull() }
            return object
        }
        let plan: [String: Any]? = entry.inputPlan.map {
            ["schema_version": $0.schema_version, "inputs": $0.inputs.map {
                ["source": $0.source, "output": nullable($0.output), "in_place": $0.in_place, "requires_verification": $0.requires_verification]
            }]
        }
        let ignoredInventory: [String: Any]
        do {
            let rows = try HistoryIgnoredSource.reconcile(ignoredObservations, expected: entry.summary?.img.ignored)
            ignoredInventory = ["state": "matches_reported_total", "listed_items": rows.count,
                "reported_ignored": nullable(entry.summary?.img.ignored), "diagnostic": NSNull()]
        } catch {
            ignoredInventory = ["state": ignoredObservations.isEmpty ? "unavailable" : "inconsistent",
                "listed_items": NSNull(), "reported_ignored": nullable(entry.summary?.img.ignored), "diagnostic": error.localizedDescription]
        }
        let artifact: [String: Any] = ["schema_version": 2, "artifact_kind": "mfb_selected_run_diagnostic",
            "authoritative_state": false, "full_run_manifest": false,
            "exported_at": ISO8601DateFormatter().string(from: Date()), "session_id": entry.stamp,
            "history_entry_id": entry.id, "run_id": nullable(context.run_id),
            "scope": ["output_start_exclusive": start, "output_end_inclusive": end, "known_end_sequence": nullable(knownEnd),
                "end_source": entry.outputEndSource,
                "total_output_rows": total, "retained_output_rows": retained.count, "diagnostic_log_rows": diagnosticCount,
                "phase_control_rows": phaseCount, "verification_activity_rows": activityCount, "file_failure_observation_rows": fileFailureCount,
                "ignored_source_observation_rows": ignoredObservations.count,
                "omitted_non_diagnostic_output_rows": total - retained.count,
                "scan_complete": true, "count_scope": "log_records_not_files", "contains_local_paths_and_diagnostics": true],
            "context": contextObject, "summary": nullable(summaryObject), "finished": nullable(finishedObject),
            "configuration": ["marker_seen": entry.configMarkerSeen, "invocation_count": entry.configInvocationCount, "snapshots": configurations],
            "worker_launches": launches.isEmpty ? NSNull() : launches,
            "output_storage_unbound_invalid": entry.unboundOutputStorageInvalid,
            "input_plan": nullable(plan), "verification": nullable(entry.verification.map(verification)),
            "input_verifications": entry.inputVerifications.mapValues(verification), "ignored_inventory": ignoredInventory,
            "fast_img_delivery": ["invalid": entry.fastDeliveryInvalid, "receipts": try entry.fastDeliveries.map { receipt in
                let encoder = JSONEncoder()
                encoder.keyEncodingStrategy = .convertToSnakeCase
                return try JSONSerialization.jsonObject(with: encoder.encode(receipt))
            }],
            "output_cleanup": ["state": entry.outputCleanups.isEmpty ? "unavailable" : "recorded",
                "receipts": entry.outputCleanups.map(\.recordedObject)], "records": retained]
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
            let removalTable = try statement(database, "SELECT type FROM sqlite_master WHERE name='history_removed_sessions_v1'")
            let removalResult = sqlite3_step(removalTable)
            var hasRemovedSessions = false
            if removalResult == SQLITE_ROW {
                hasRemovedSessions = try databaseText(removalTable, 0, database: database) == "table"
                guard hasRemovedSessions, sqlite3_step(removalTable) == SQLITE_DONE else {
                    sqlite3_finalize(removalTable); throw databaseError(database, "invalid removal registry")
                }
            } else if removalResult != SQLITE_DONE {
                sqlite3_finalize(removalTable); throw databaseError(database)
            }
            sqlite3_finalize(removalTable)
            if hasRemovedSessions {
                let columns = try statement(database, "PRAGMA table_info(history_removed_sessions_v1)")
                defer { sqlite3_finalize(columns) }
                guard sqlite3_step(columns) == SQLITE_ROW,
                      try databaseText(columns, 1, database: database) == "session_id",
                      try databaseText(columns, 2, database: database).uppercased() == "TEXT",
                      sqlite3_column_int(columns, 3) == 1, sqlite3_column_int(columns, 5) == 1,
                      sqlite3_step(columns) == SQLITE_DONE else { throw databaseError(database, "unsupported removal registry") }
            }
            let retainedSessions = hasRemovedSessions
                ? " WHERE NOT EXISTS (SELECT 1 FROM history_removed_sessions_v1 r WHERE r.session_id=history_sessions.session_id)" : ""
            let sessions = try statement(database, "SELECT session_id, updated_at FROM history_sessions\(retainedSessions) ORDER BY updated_at DESC, session_id DESC LIMIT \(maximumSessions + 1)")
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
            let retiredMembership = hasRemovedSessions ? " UNION ALL SELECT 1 FROM history_removed_sessions_v1 WHERE session_id=?1" : ""
            let membership = try statement(database, "SELECT 1 FROM history_sessions WHERE session_id = ?1\(retiredMembership) LIMIT 1")
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
                        if !left.fastDeliveries.isEmpty && !right.fastDeliveries.isEmpty,
                           left.fastDeliveries.sorted(by: { ($0.sourceDirectory ?? "") < ($1.sourceDirectory ?? "") })
                            != right.fastDeliveries.sorted(by: { ($0.sourceDirectory ?? "") < ($1.sourceDirectory ?? "") }) {
                            conflictingCopies = true
                        }
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

private struct HistoryRunSelection: Codable, Hashable {
    let session_id: String
    let run_id: String
}

private struct HistoryRemovalRequest: Encodable {
    let schema_version = 1
    let runs: [HistoryRunSelection]
    var snapshot_token: String?

    init(entries: [HistoryEntry], directory: URL) throws {
        guard !entries.isEmpty, entries.count <= 128 else { throw HostError(message: localized("history.remove.unsupported")) }
        runs = try entries.map { entry in
            guard !entry.legacy, entry.finished != nil, entry.hasStoredOutput,
                  entry.audit.standardizedFileURL == directory.appendingPathComponent("history.sqlite3").standardizedFileURL,
                  validHistoryIdentity(entry.stamp), let run = entry.context?.run_id, validHistoryIdentity(run) else {
                throw HostError(message: localized("history.remove.unsupported"))
            }
            return HistoryRunSelection(session_id: entry.stamp, run_id: run)
        }
        guard Set(runs).count == runs.count, Set(runs.map(\.run_id)).count == runs.count else {
            throw HostError(message: localized("history.remove.unsupported"))
        }
    }

    func arguments(directory: URL) throws -> [String] {
        let encoded = try JSONEncoder().encode(self)
        guard encoded.count <= 65536 else { throw HostError(message: localized("history.remove.unsupported")) }
        return ["--history-directory", directory.path,
                snapshot_token == nil ? "--history-cleanup-preview" : "--history-cleanup",
                String(decoding: encoded, as: UTF8.self)] + (snapshot_token == nil ? [] : ["--yes"]) + ["--json"]
    }
}

private struct HistoryRemovalArtifact: Codable {
    let path: String
    let scope: String
    let status: String
    let reason: String

    var valid: Bool { path.hasPrefix("/") && !path.contains("\0") && !scope.isEmpty && !reason.isEmpty }
}

private struct HistoryRemovalPreview: Codable {
    let schema_version: Int
    let kind: String
    let source: String
    let snapshot_token: String
    let runs: [HistoryRunSelection]
    let event_rows: UInt64
    let output_rows: UInt64
    let event_bytes: UInt64
    let output_bytes: UInt64
    let artifacts: [HistoryRemovalArtifact]
    let can_execute: Bool
    let external_log_scope: String

    static func decode(_ data: Data, request: HistoryRemovalRequest, directory: URL) throws -> Self {
        let preview = try JSONDecoder().decode(Self.self, from: data)
        guard preview.schema_version == 1, preview.kind == "history_cleanup_preview", preview.can_execute,
              preview.source.hasPrefix("/"), !preview.source.contains("\0"),
              URL(fileURLWithPath: preview.source).standardizedFileURL == directory.resolvingSymlinksInPath().appendingPathComponent("history.sqlite3").standardizedFileURL,
              preview.snapshot_token.utf8.count == 64, preview.snapshot_token.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
              preview.runs.count == request.runs.count, Set(preview.runs) == Set(request.runs),
              preview.external_log_scope == "registered_session_logs_only",
              [preview.event_rows, preview.output_rows, preview.event_bytes, preview.output_bytes].allSatisfy({ $0 <= UInt64(Int64.max) }),
              preview.event_rows >= UInt64(request.runs.count * 2), preview.artifacts.count <= request.runs.count * 4,
              preview.artifacts.allSatisfy({ $0.valid && ["delete", "retained"].contains($0.status)
                  && ["registered_file", "registered_file_problem", "unregistered_external_logs"].contains($0.scope)
                  && ($0.status != "delete" || $0.scope == "registered_file") }) else {
            throw HostError(message: localized("history.remove.invalid"))
        }
        return preview
    }

    func detail(privacy: Bool) -> String {
        var rows = [localized("history.remove.scope"),
            localized("history.remove.counts", runs.count, event_rows, output_rows),
            localized("history.remove.artifacts", artifacts.filter { $0.status == "delete" }.count,
                      artifacts.filter { $0.status == "retained" }.count)]
        for artifact in artifacts {
            rows.append("\n" + localized(artifact.status == "delete" ? "history.remove.confirm" : "history.remove.retained")
                + ": " + DisplayPrivacy.path(artifact.path, hidden: privacy, problem: artifact.scope == "registered_file_problem") + "\n" + artifact.reason)
        }
        return rows.joined(separator: "\n")
    }
}

private struct HistoryRemovalReceipt: Codable {
    let schema_version: Int
    let kind: String
    let status: String
    let database_committed: Bool
    let removed_runs: UInt64
    let event_rows: UInt64
    let output_rows: UInt64
    let removed_artifacts: UInt64
    let retained_artifacts: [HistoryRemovalArtifact]
    let failed_artifacts: [HistoryRemovalArtifact]

    static func decode(_ data: Data, preview: HistoryRemovalPreview) throws -> Self {
        let receipt = try JSONDecoder().decode(Self.self, from: data)
        guard receipt.schema_version == 1, receipt.kind == "history_cleanup_receipt", receipt.database_committed,
              ["completed", "partial"].contains(receipt.status),
              receipt.removed_runs == UInt64(preview.runs.count), receipt.event_rows == preview.event_rows,
              receipt.output_rows == preview.output_rows,
              receipt.removed_artifacts <= UInt64(preview.artifacts.filter { $0.status == "delete" }.count),
              receipt.retained_artifacts.count <= preview.artifacts.count,
              receipt.failed_artifacts.count <= preview.artifacts.count + 1,
              receipt.retained_artifacts.allSatisfy({ artifact in
                  artifact.valid && artifact.status == "retained" && preview.artifacts.contains {
                      $0.status == "retained" && $0.path == artifact.path && $0.scope == artifact.scope && $0.reason == artifact.reason
                  }
              }),
              receipt.failed_artifacts.allSatisfy({ $0.valid && $0.status == "failed"
                  && ["registered_file", "ownership_registry"].contains($0.scope) }),
              receipt.removed_artifacts + UInt64(receipt.failed_artifacts.filter { $0.scope == "registered_file" }.count)
                  == UInt64(preview.artifacts.filter { $0.status == "delete" }.count),
              (receipt.status == "completed") == (receipt.failed_artifacts.isEmpty
                  && !receipt.retained_artifacts.contains { $0.scope == "registered_file_problem" }) else {
            throw HostError(message: localized("history.remove.invalid"))
        }
        return receipt
    }

    func detail(privacy: Bool) -> String {
        ([localized(status == "completed" ? "history.remove.completed" : "history.remove.partial", removed_runs, removed_artifacts)]
            + (retained_artifacts + failed_artifacts).map { artifact in
                "\n" + localized(artifact.status == "failed" ? "history.remove.failed" : "history.remove.retained") + ": "
                    + DisplayPrivacy.path(artifact.path, hidden: privacy, problem: artifact.status == "failed" || artifact.scope == "registered_file_problem")
                    + "\n" + artifact.reason
            }).joined(separator: "\n")
    }
}

private func historyRemovalOutput(_ request: HistoryRemovalRequest, directory: URL) throws -> Data {
    guard let binary = ProcessorLocator.resolveTool(named: "cache_cleaner") else {
        throw HostError(message: localized("settings.cache.backend_missing"))
    }
    // A destructive operation must finish its transaction and report recovery paths before it exits.
    return try settingsToolOutput(binary, arguments: request.arguments(directory: directory), timeout: request.snapshot_token == nil ? 10 : nil)
}

private struct HistoryReadOnlyAccessReceipt: Decodable {
    let schema_version: Int
    let kind: String
    let source: String
    let history_schema_version: Int

    static func validate(_ data: Data, directory: URL) throws {
        let value = try JSONDecoder().decode(Self.self, from: data)
        guard value.schema_version == 1, value.kind == "history_readonly_access",
              value.source == (try HistoryStore.databasePath(directory.appendingPathComponent("history.sqlite3"))),
              value.history_schema_version == 1 || value.history_schema_version == 2 else {
            throw HostError(message: localized("history.issue.read"))
        }
    }
}

private func prepareHistoryReadOnlyAccess(directory: URL) throws {
    guard let binary = ProcessorLocator.resolveTool(named: "cache_cleaner") else {
        throw HostError(message: localized("settings.cache.backend_missing"))
    }
    let output = try settingsToolOutput(binary, arguments: ["--history-prepare-readonly", "--history-directory", directory.path, "--json"], timeout: 10)
    try HistoryReadOnlyAccessReceipt.validate(output, directory: directory)
}

private func historyRemovalFixtureOutput(_ request: HistoryRemovalRequest, directory: URL) throws -> Data {
    let retained = HistoryRemovalArtifact(path: directory.path, scope: "unregistered_external_logs", status: "retained",
        reason: "Synthetic example: older logs have no ownership record and are kept.")
    let eventRows = UInt64(request.runs.count * 4)
    let outputRows = UInt64(request.runs.count * 8)
    if request.snapshot_token != nil {
        return try JSONEncoder().encode(HistoryRemovalReceipt(schema_version: 1, kind: "history_cleanup_receipt",
            status: "completed", database_committed: true, removed_runs: UInt64(request.runs.count),
            event_rows: eventRows, output_rows: outputRows, removed_artifacts: 1,
            retained_artifacts: [retained], failed_artifacts: []))
    }
    return try JSONEncoder().encode(HistoryRemovalPreview(schema_version: 1, kind: "history_cleanup_preview",
        source: directory.resolvingSymlinksInPath().appendingPathComponent("history.sqlite3").path,
        snapshot_token: String(repeating: "a", count: 64), runs: request.runs,
        event_rows: eventRows, output_rows: outputRows, event_bytes: 1024, output_bytes: 4096,
        artifacts: [HistoryRemovalArtifact(path: directory.appendingPathComponent("Private holiday photographs.log").path,
            scope: "registered_file", status: "delete", reason: "Synthetic owned log"), retained],
        can_execute: true, external_log_scope: "registered_session_logs_only"))
}

final class ProcessingHistoryPanel: NSWindowController, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate, NSWindowDelegate {
    private let table = NSTableView()
    private let emptyState = NSStackView()
    private let emptyTitle = NSTextField(labelWithString: "")
    private let emptyHint = NSTextField(wrappingLabelWithString: "")
    private let search = NSSearchField()
    private let filter = NSSegmentedControl(labels: ["", ""], trackingMode: .selectOne, target: nil, action: nil)
    private let detail = NSTextView()
    private let detailMode = NSSegmentedControl(labels: ["", "", "", "", ""], trackingMode: .selectOne, target: nil, action: nil)
    private let ignoredView = HistoryIgnoredView()
    private var ignoredSources: [HistoryIgnoredSource]?
    private let outputFilter = NSSegmentedControl(labels: ["", ""], trackingMode: .selectOne, target: nil, action: nil)
    private let failedTable = NSTableView()
    private let failedSearch = NSSearchField()
    private let failedStatus = NSTextField(wrappingLabelWithString: "")
    private let failedContent = NSStackView()
    private var failedCopyButton = NSButton()
    private var failedRevealButton = NSButton()
    private var failedDetailsButton = NSButton()
    private var failedPaths: [String] = []
    private struct FailureSourceKey: Hashable {
        let runID: String
        let invocationID: String
        let pipeline: String
        let sourcePath: String
        let absolutePath: String?

        init(_ failure: HistoryFileFailure) {
            runID = failure.run_id
            invocationID = failure.invocation_id
            pipeline = failure.pipeline
            sourcePath = failure.source_path
            absolutePath = failure.absolute_path
        }

        var ordering: [String] { [sourcePath, absolutePath ?? "", runID, invocationID, pipeline] }

        static func == (lhs: Self, rhs: Self) -> Bool {
            lhs.runID == rhs.runID && lhs.invocationID == rhs.invocationID && lhs.pipeline == rhs.pipeline
                && lhs.sourcePath.utf8.elementsEqual(rhs.sourcePath.utf8)
                && lhs.absolutePath.map { Data($0.utf8) } == rhs.absolutePath.map { Data($0.utf8) }
        }

        func hash(into hasher: inout Hasher) {
            hasher.combine(runID); hasher.combine(invocationID); hasher.combine(pipeline)
            hasher.combine(Data(sourcePath.utf8)); hasher.combine(absolutePath.map { Data($0.utf8) })
        }
    }
    private struct FailureObservation {
        let failure: HistoryFileFailure
        let repetitions: Int
    }
    private struct FailureDisplayRow {
        let path: String
        let absolutePath: String?
        let reason: String
        let source: FailureSourceKey?
        let observations: [FailureObservation]

        var searchable: String {
            ([path, absolutePath ?? "", reason] + observations.flatMap { observation in
                let failure = observation.failure
                return [failure.run_id, failure.invocation_id, failure.pipeline, failure.source_path,
                        failure.absolute_path ?? "", failure.message, failure.reason_code ?? "", failure.userReason]
            }).joined(separator: "\n")
        }
    }
    private var failureDisplayRows: [FailureDisplayRow] = []
    private var failureDetailAlert: NSAlert?
    private var failureDetailGeneration = UUID()
    private var cleanupDetailAlert: NSAlert?
    private var cleanupDetailGeneration = UUID()
    private var detailScroll = NSScrollView()
    private var outputNavigation = NSStackView()
    private var olderButton = NSButton()
    private var newerButton = NSButton()
    private var outputPage: HistoryStore.Output?
    private var outputLoading = false
    private var batchIssueResult: HistoryStore.BatchIssues?
    private var comparisonIssues: [HistoryStore.BatchIssues]?
    private var issueScanControl: HistoryStore.IssueScanControl?
    private var issueScanIsIgnored = false
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
    private var cleanupButton = NSButton()
    private var removeButton = NSButton()
    private var removalBusy = false
    private var removalAlert: NSAlert?
    private var removalQuery = historyRemovalOutput
    private var removalFixture = false
    private var entries: [HistoryEntry] = []
    private var displayed: [HistoryEntry] = []
    private var directory: URL
    private let preferences: UserDefaults
    private var privacyEnabled: Bool { DisplayPrivacy.enabled(in: preferences) }
    private var generation = UUID()
    private var outputGeneration = UUID()
    private var exportGeneration = UUID()
    private var exportBusy = false
    private var loadIssues: [String] = []
    private var limited = false
    private var initialDividerConfigured = false
    private var selectionInitialized = false

    init(directory: URL, preferences: UserDefaults = .standard) {
        self.directory = directory
        self.preferences = preferences
        outputGeneration = UUID()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1040, height: 620),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        super.init(window: window)
        window.minSize = NSSize(width: 780, height: 440)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.setFrameAutosaveName("MFBProcessingHistory")
        buildInterface()
        NotificationCenter.default.addObserver(self, selector: #selector(displayPrivacyChanged),
            name: DisplayPrivacy.changed, object: preferences)
    }

    @objc private func displayPrivacyChanged() {
        clearCleanupDetails()
        let selection = Set(selectedEntries.map(\.id))
        if let sheet = removalAlert?.window, sheet.sheetParent != nil { sheet.sheetParent?.endSheet(sheet, returnCode: .alertSecondButtonReturn) }
        if exportBusy {
            exportGeneration = UUID()
            exportBusy = false
            refreshButton.isEnabled = true
            progress.stopAnimation(nil)
        }
        if privacyEnabled, detailMode.selectedSegment == 4 { detailMode.selectedSegment = 0 }
        table.reloadData()
        table.selectRowIndexes(IndexSet(displayed.indices.filter { selection.contains(displayed[$0].id) }), byExtendingSelection: false)
        copyButton.toolTip = localized(privacyEnabled ? "privacy.display.copy_visible" : "history.copy_sources")
        copyButton.title = copyButton.toolTip ?? ""
        copyButton.setAccessibilityLabel(copyButton.toolTip)
        detailModeChanged()
    }

    required init?(coder: NSCoder) { nil }

    func show() {
        updateLocalization()
        if !isBackgroundSelfTest {
            showWindow(nil)
            window?.makeKeyAndOrderFront(nil)
        }
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
        guard !removalBusy else { return }
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
            let result = Swift.Result {
                try HistoryStore.loadForDisplay(directory: source) {
                    DispatchQueue.main.async { [weak self] in
                        guard let self, self.generation == token else { return }
                        self.status.stringValue = localized("history.preparing_read")
                    }
                    try prepareHistoryReadOnlyAccess(directory: source)
                }
            }
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
                    self.loadIssues = [error.localizedDescription]
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
        table.usesAlternatingRowBackgroundColors = false
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
        let listContainer = NSView(frame: listScroll.frame)
        listContainer.addSubview(listScroll)
        emptyState.orientation = .vertical
        emptyState.alignment = .centerX
        emptyState.spacing = HistoryLayout.spacing
        emptyState.translatesAutoresizingMaskIntoConstraints = false
        let emptyIcon = NSImageView(image: NSImage(systemSymbolName: "clock.arrow.circlepath", accessibilityDescription: nil) ?? NSImage())
        emptyIcon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 28, weight: .regular)
        emptyIcon.contentTintColor = .secondaryLabelColor
        emptyTitle.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
        emptyHint.font = HistoryLayout.bodyFont
        emptyHint.textColor = .secondaryLabelColor
        for label in [emptyTitle, emptyHint] { label.alignment = .center }
        for view in [emptyIcon, emptyTitle, emptyHint] { emptyState.addArrangedSubview(view) }
        listContainer.addSubview(emptyState)
        let emptyWidth = emptyState.widthAnchor.constraint(equalToConstant: 300)
        emptyWidth.priority = .defaultHigh
        NSLayoutConstraint.activate([
            listScroll.leadingAnchor.constraint(equalTo: listContainer.leadingAnchor),
            listScroll.trailingAnchor.constraint(equalTo: listContainer.trailingAnchor),
            listScroll.topAnchor.constraint(equalTo: listContainer.topAnchor),
            listScroll.bottomAnchor.constraint(equalTo: listContainer.bottomAnchor),
            emptyState.centerXAnchor.constraint(equalTo: listContainer.centerXAnchor),
            emptyState.centerYAnchor.constraint(equalTo: listContainer.centerYAnchor),
            emptyWidth,
            emptyState.widthAnchor.constraint(lessThanOrEqualTo: listContainer.widthAnchor, constant: -40)
        ])
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
        listController.view = listContainer
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
        let detailRoot = NSStackView(views: [detailMode, outputNavigation, detailScroll, failedContent, ignoredView])
        detailRoot.orientation = .vertical
        detailRoot.alignment = .leading
        detailRoot.spacing = 6
        detailRoot.translatesAutoresizingMaskIntoConstraints = false
        detailScroll.setContentHuggingPriority(.defaultLow, for: .vertical)
        detailScroll.widthAnchor.constraint(equalTo: detailRoot.widthAnchor).isActive = true
        failedContent.widthAnchor.constraint(equalTo: detailRoot.widthAnchor).isActive = true
        ignoredView.widthAnchor.constraint(equalTo: detailRoot.widthAnchor).isActive = true
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

        logButton = iconButton("doc.text", key: "history.open_log", action: #selector(openLog), showsTitle: true)
        revealButton = iconButton("folder", key: "history.reveal", action: #selector(revealAudit), showsTitle: true)
        copyButton = iconButton("doc.on.doc", key: "history.copy_sources", action: #selector(copySources), showsTitle: true)
        exportButton = iconButton("arrow.up.doc", key: "history.export.title", action: #selector(exportSelectedRun), showsTitle: true)
        cleanupButton = iconButton("tray.full", key: "cleanup.inspect", action: #selector(openCleanupDetails), showsTitle: true)
        removeButton = iconButton("trash", key: "history.remove.title", action: #selector(removeSelectedHistory), showsTitle: true)
        removeButton.hasDestructiveAction = true
        let divider = NSBox()
        divider.boxType = .separator
        root.addArrangedSubview(divider)
        divider.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        for buttons in [[logButton, cleanupButton, exportButton], [revealButton, copyButton, removeButton]] {
            let actions = NSStackView(views: buttons)
            actions.orientation = .horizontal
            actions.spacing = HistoryLayout.spacing
            root.addArrangedSubview(actions)
            actions.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        }
        progress.style = .spinning
        progress.controlSize = .small
        progress.isDisplayedWhenStopped = false
        status.font = HistoryLayout.smallFont
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingMiddle
        status.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let footer = NSStackView(views: [progress, status])
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
        failedTable.target = self
        failedTable.doubleAction = #selector(openFailureDetails)
        failedTable.rowHeight = HistoryLayout.rowHeight
        failedTable.usesAlternatingRowBackgroundColors = true
        failedTable.style = .inset
        failedTable.allowsEmptySelection = true
        let path = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("failedPath"))
        path.minWidth = 100
        path.width = 180
        failedTable.addTableColumn(path)
        let reason = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("failedReason"))
        reason.minWidth = 130
        reason.width = 230
        failedTable.addTableColumn(reason)
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
        failedDetailsButton = iconButton("info.circle", key: "history.file_failure.detail", action: #selector(openFailureDetails))
        let toolbar = NSStackView(views: [failedSearch, failedDetailsButton, failedCopyButton, failedRevealButton])
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

    private func iconButton(_ symbol: String, key: String, action: Selector, showsTitle: Bool = false) -> NSButton {
        let button = NSButton(image: NSImage(systemSymbolName: symbol, accessibilityDescription: localized(key)) ?? NSImage(),
                              target: self, action: action)
        button.bezelStyle = showsTitle ? .rounded : .texturedRounded
        button.title = showsTitle ? localized(key) : ""
        button.imagePosition = showsTitle ? .imageLeading : .imageOnly
        button.toolTip = localized(key)
        button.setAccessibilityLabel(localized(key))
        if !showsTitle { button.widthAnchor.constraint(equalToConstant: HistoryLayout.buttonSize).isActive = true }
        button.setContentCompressionResistancePriority(.required, for: .horizontal)
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
        detailMode.setLabel(localized("history.ignored.tab"), forSegment: 4)
        ignoredView.updateLocalization()
        detailMode.setAccessibilityLabel(localized("history.details"))
        failedSearch.placeholderString = localized("history.failed_files.search")
        failedSearch.setAccessibilityLabel(localized("history.failed_files.search"))
        failedTable.setAccessibilityLabel(localized("history.failed_files"))
        failedTable.tableColumns.first?.title = localized("history.failed_files.path")
        failedTable.tableColumns.last?.title = localized("history.failed_files.reason")
        outputFilter.setLabel(localized("history.output.all_page"), forSegment: 0)
        outputFilter.setLabel(localized("history.output.diagnostics_page"), forSegment: 1)
        outputFilter.setAccessibilityLabel(localized("history.output.filter_page"))
        for column in table.tableColumns { column.title = localized("history.column.\(column.identifier.rawValue)") }
        for (button, key) in [(refreshButton, "history.refresh"), (logButton, "history.open_log"),
                              (revealButton, "history.reveal"), (copyButton, "history.copy_sources"),
                              (exportButton, "history.export.title"),
                               (cleanupButton, "cleanup.inspect"),
                               (removeButton, "history.remove.title"),
                               (failedDetailsButton, "history.file_failure.detail"),
                              (failedCopyButton, "history.failed_files.copy"), (failedRevealButton, "history.failed_files.reveal"),
                              (olderButton, "history.output.older"), (newerButton, "history.output.newer")] {
            let label = localized(button === copyButton && privacyEnabled ? "privacy.display.copy_visible" : key)
            button.toolTip = label
            if button.imagePosition != .imageOnly { button.title = label }
            button.setAccessibilityLabel(label)
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
        status.toolTip = DisplayPrivacy.path(directory.path, hidden: privacyEnabled, problem: !loadIssues.isEmpty)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { tableView === failedTable ? failedPaths.count : displayed.count }
    func tableViewSelectionDidChange(_ notification: Notification) {
        if (notification.object as? NSTableView) === failedTable { updateActions() }
        else { showSelected() }
    }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if tableView === failedTable {
            guard failureDisplayRows.indices.contains(row) else { return nil }
            let isReason = tableColumn?.identifier.rawValue == "failedReason"
            let value = isReason ? failureDisplayRows[row].reason : failureDisplayRows[row].path
            let label = NSTextField(labelWithString: value)
            label.font = HistoryLayout.smallFont
            label.lineBreakMode = isReason ? .byTruncatingTail : .byTruncatingMiddle
            label.toolTip = value
            label.setAccessibilityLabel(value)
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
            if privacyEnabled {
                label.stringValue = localized("privacy.display.hidden")
                label.toolTip = label.stringValue
            } else if let paths = entry.context?.inputs, !paths.isEmpty {
                let first = URL(fileURLWithPath: paths[0]).lastPathComponent
                label.stringValue = paths.count == 1 ? first : localized("history.source_multiple", first, paths.count)
                label.toolTip = paths.joined(separator: "\n")
            } else { label.stringValue = localized("result.unknown") }
        case "status":
            label.stringValue = localized(entry.statusKey)
                + (entry.auxiliaryAttention.isEmpty ? "" : "\n" + entry.auxiliaryAttention)
            label.toolTip = entry.statusLabel
            label.setAccessibilityLabel(entry.statusLabel)
            label.maximumNumberOfLines = 2
            label.lineBreakMode = .byWordWrapping
            resultColor = entry.statusColor
        case "verification":
            label.stringValue = entry.verification?.count_status ?? localized("result.unknown")
            resultColor = entry.hasVerificationFailure ? .systemRed : .secondaryLabelColor
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
        detailMode.setLabel(localized(selection.count == 2 ? "history.compare.failures" : "history.failed_files"), forSegment: 2)
        if selection.count == 2 { detail.string = Self.renderComparison(selection, privacy: privacyEnabled) }
        else if selection.count > 2 { detail.string = localized("history.compare.select_two") }
        else { detail.string = selected.map { Self.render($0, privacy: privacyEnabled) } ?? localized(displayed.isEmpty && !entries.isEmpty ? "history.no_match" : "history.select") }
        detail.scrollToBeginningOfDocument(nil)
        updateActions()
    }

    fileprivate static func renderComparison(_ entries: [HistoryEntry], privacy: Bool = false) -> String {
        guard entries.count == 2 else { return localized("history.compare.select_two") }
        if privacy {
            return ([localized("history.compare")] + entries.enumerated().map { index, entry in
                localized("history.compare.batch", index + 1) + "\n" + renderPrivate(entry)
            }).joined(separator: "\n\n")
        }
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

    private static func renderPrivate(_ entry: HistoryEntry) -> String {
        let unknown = localized("result.unknown")
        func count(_ value: Int?) -> String { value.map(String.init) ?? unknown }
        func row(_ key: String, _ value: String) -> String { localized("history.detail.\(key)") + ": " + value }
        var lines = [entry.statusLabel, timestampLabel(entry.timestamp), localized("privacy.display.scope"), "",
            row("mode", entry.modeLabel ?? unknown), entry.sizeOutcome]
        for (key, media) in [("img", entry.summary?.img), ("vid", entry.summary?.vid)] {
            lines.append(localized("history.media.\(key)"))
            if media?.active == false { lines.append(localized("history.not_run")); continue }
            lines += [localized("result.counts", count(media?.succeeded), count(media?.skipped),
                count(media?.failed), count(media?.ignored)), localized("result.unprocessed", count(media?.unprocessed)),
                row("exit", count(media?.exit_code))]
        }
        lines += [row("sources", localized("privacy.display.hidden")), row("output", localized("privacy.display.hidden")),
            row("integrity", entry.summary?.integrity.state ?? unknown), row("issues", count(entry.summary?.integrity.issue_count))]
        for verification in entry.inputVerifications.isEmpty ? [entry.verification].compactMap({ $0 }) : Array(entry.inputVerifications.values) {
            lines += [row("source_inventory", String(verification.source_count)),
                row("optimized_inventory", String(verification.optimized_count)), row("failed_inventory", String(verification.failed_count)),
                row("remaining_inventory", String(verification.source_remaining_count)), row("deleted_inventory", String(verification.verified_deleted_count))]
            if verification.has_warnings || verification.issue_count > 0 || verification.count_status?.hasPrefix("MISMATCH") == true {
                lines += [row("verified_source", verification.source_path ?? unknown), row("count_status", verification.count_status ?? unknown)]
            }
        }
        func numericFields(_ object: [String: Any], prefix: String = "") -> [String] {
            object.keys.sorted().flatMap { key -> [String] in
                let field = prefix.isEmpty ? key : prefix + "." + key
                if let nested = object[key] as? [String: Any] { return numericFields(nested, prefix: field) }
                if let value = object[key] as? NSNumber {
                    return [field + ": " + (CFGetTypeID(value) == CFBooleanGetTypeID() ? (value.boolValue ? "true" : "false") : value.stringValue)]
                }
                return []
            }
        }
        for snapshot in entry.configSnapshots {
            lines += ["", localized("history.effective_config"), row("config_pipeline", snapshot.pipeline)]
            if let object = try? JSONSerialization.jsonObject(with: Data(snapshot.values.utf8)) as? [String: Any] {
                lines += numericFields(object)
            } else { lines.append(localized("history.config.invalid")) }
        }
        if let error = entry.finished?.error { lines += ["", row("error", error)] }
        lines += entry.issues
        if let summary = entry.summary, !summary.failed_files.isEmpty { lines += ["", localized("history.failed_files")] + summary.failed_files }
        for receipt in entry.outputCleanups where receipt.needsAttention { lines += ["", receipt.detailText] }
        for receipt in entry.fastDeliveries where receipt.isDeferred { lines += ["", receipt.deferredDetails] }
        for invocation in entry.invocations {
            if let error = invocation.binary.diagnostic { lines += ["", row("worker_observation_error", error)] }
        }
        for storage in entry.outputStorage.values where storage.state == "failed" || !storage.capture_complete {
            lines += ["", storage.detailText]
        }
        return lines.joined(separator: "\n")
    }

    fileprivate static func render(_ entry: HistoryEntry, privacy: Bool = false) -> String {
        if privacy { return renderPrivate(entry) }
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
        var lines = [entry.statusLabel, Self.timestampLabel(entry.timestamp), ""]
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
        lines.append(row("run_id", context.run_id ?? unknown))
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
        lines.append(localized("history.output_storage.note"))
        if entry.unboundOutputStorageInvalid { lines.append(localized("history.output_storage.unbound")) }
        if entry.invocationEvidenceInvalid {
            lines.append(localized("history.worker.invalid"))
        } else if entry.invocations.isEmpty {
            lines.append(localized("history.worker.not_recorded"))
            lines.append(localized("history.output_storage.not_recorded"))
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
                if entry.workerExecutionEvidenceInvalid {
                    lines.append(localized("history.worker.execution.invalid"))
                } else if let observations = entry.workerExecutions[invocation.invocation_id], let last = observations.last {
                    lines.append(localized("history.worker.execution.pid", String(last.pid)))
                    if last.state == "spawned" { lines.append(localized("history.worker.execution.spawned")) }
                    else if let signal = last.signal { lines.append(localized("history.worker.execution.signal", String(signal))) }
                    else { lines.append(localized("history.worker.execution.exited", last.exit_code.map(String.init) ?? unknown)) }
                } else { lines.append(localized("history.worker.execution.unknown")) }
                lines += [localized("history.output_storage.title"), entry.outputStorageText(for: invocation.invocation_id)]
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
        if entry.context?.mode == "fast-img" {
            if entry.fastDeliveryInvalid { lines += ["", localized("result.delivery_invalid")] }
            for receipt in entry.fastDeliveries where receipt.isDeferred {
                lines += ["", localized("result.delivery_record_title"), receipt.deferredDetails]
                if let error = receipt.finalizationError { lines.append(error) }
            }
            lines += ["", localized("cleanup.history_title")]
            if entry.outputCleanupInvalid { lines.append(localized("cleanup.invalid")) }
            else if entry.outputCleanups.isEmpty { lines.append(localized("cleanup.not_recorded")) }
            else { lines += entry.outputCleanups.map(\.detailText) }
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
        let ready = refreshButton.isEnabled && !exportBusy && !removalBusy
        emptyState.isHidden = !ready || !displayed.isEmpty
        emptyTitle.stringValue = localized(!loadIssues.isEmpty ? "history.read_issues" : entries.isEmpty ? "history.empty" : "history.no_match")
        emptyHint.stringValue = localized(!loadIssues.isEmpty ? "history.empty.read_hint" : entries.isEmpty ? "history.empty.hint" : "history.filtered.hint")
        let single = ready && selected != nil
        let pair = ready && selectedEntries.count == 2 && selectedEntries.allSatisfy(\.hasStoredOutput)
        table.isEnabled = ready
        search.isEnabled = ready
        filter.isEnabled = ready
        let failuresVisible = single && detailMode.selectedSegment == 2
        let ignoredVisible = single && detailMode.selectedSegment == 4
        failedContent.isHidden = !failuresVisible
        ignoredView.isHidden = !ignoredVisible
        ignoredView.setEnabled(ignoredVisible)
        detailScroll.isHidden = failuresVisible || ignoredVisible
        outputNavigation.isHidden = detailMode.selectedSegment != 1
        failedSearch.isEnabled = failuresVisible
        failedTable.isEnabled = failuresVisible
        failedCopyButton.isEnabled = failuresVisible && selectedFailedPath != nil
        failedRevealButton.isEnabled = failuresVisible && selectedFailedURL != nil
        failedDetailsButton.isEnabled = failuresVisible && failureDisplayRows.indices.contains(failedTable.selectedRow)
            && !failureDisplayRows[failedTable.selectedRow].observations.isEmpty
        logButton.isEnabled = single && !outputLoading && (selected?.hasStoredOutput == true || (!privacyEnabled && selected?.logs.isEmpty == false))
        detailMode.setEnabled(single && selected?.hasStoredOutput == true, forSegment: 1)
        detailMode.setEnabled(single || pair, forSegment: 2)
        detailMode.setEnabled(single && selected?.hasStoredOutput == true, forSegment: 3)
        detailMode.setEnabled(single && !privacyEnabled && selected?.hasStoredOutput == true && selected?.context?.mode == "fast-img", forSegment: 4)
        olderButton.isEnabled = single && detailMode.selectedSegment == 1 && !outputLoading && outputPage?.hasOlder == true
        newerButton.isEnabled = single && detailMode.selectedSegment == 1 && !outputLoading && outputPage?.hasNewer == true
        outputFilter.isEnabled = single && detailMode.selectedSegment == 1 && !outputLoading && outputPage != nil
        revealButton.isEnabled = single && !privacyEnabled
        copyButton.isEnabled = ready && (privacyEnabled ? !selectedEntries.isEmpty : selectedEntries.contains { $0.context?.inputs.isEmpty == false })
        copyButton.toolTip = localized(privacyEnabled ? "privacy.display.copy_visible" : "history.copy_sources")
        copyButton.title = copyButton.toolTip ?? ""
        copyButton.setAccessibilityLabel(copyButton.toolTip)
        exportButton.isEnabled = single && !privacyEnabled && !outputLoading && issueScanControl == nil && selected?.hasStoredOutput == true
        exportButton.toolTip = privacyEnabled ? localized("privacy.display.raw_disabled") : localized("history.export.scope")
        cleanupButton.isEnabled = single && selected?.context?.mode == "fast-img" && (!privacyEnabled || selected?.hasCleanupAttention == true)
        removeButton.isEnabled = ready && (try? HistoryRemovalRequest(entries: selectedEntries, directory: directory)) != nil
        removeButton.toolTip = localized(removeButton.isEnabled || selectedEntries.isEmpty ? "history.remove.title" : "history.remove.unsupported")
    }

    func windowWillClose(_ notification: Notification) {
        clearCleanupDetails()
        clearFailureDetails()
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

    @objc private func removeSelectedHistory() {
        guard removeButton.isEnabled, let window, window.isVisible, window.attachedSheet == nil,
              let request = try? HistoryRemovalRequest(entries: selectedEntries, directory: directory) else { return }
        let source = directory
        let query = removalQuery
        removalBusy = true
        refreshButton.isEnabled = false
        status.stringValue = localized("history.remove.loading")
        progress.startAnimation(nil)
        updateActions()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Swift.Result { try HistoryRemovalPreview.decode(query(request, source), request: request, directory: source) }
            DispatchQueue.main.async {
                guard let self else { return }
                do {
                    let preview = try result.get()
                    guard self.window?.isVisible == true, window.attachedSheet == nil else { self.finishRemoval(localized("history.remove.cancelled")); return }
                    self.progress.stopAnimation(nil)
                    let alert = NSAlert()
                    alert.messageText = localized("history.remove.preview")
                    alert.informativeText = localized("history.remove.scope")
                    Self.setRemovalDetails(preview.detail(privacy: self.privacyEnabled), on: alert)
                    alert.alertStyle = .warning
                    alert.addButton(withTitle: localized("history.remove.confirm"))
                    alert.addButton(withTitle: localized("history.remove.cancel"))
                    alert.buttons.first?.hasDestructiveAction = true
                    self.removalAlert = alert
                    alert.beginSheetModal(for: window) { [weak self] response in
                        guard let self else { return }
                        self.removalAlert = nil
                        guard response == .alertFirstButtonReturn, window.isVisible else {
                            self.finishRemoval(localized("history.remove.cancelled")); return
                        }
                        var approved = request
                        approved.snapshot_token = preview.snapshot_token
                        self.executeRemoval(approved, preview: preview, source: source)
                    }
                } catch { self.finishRemoval(error.localizedDescription) }
            }
        }
    }

    private func finishRemoval(_ message: String) {
        removalBusy = false
        refreshButton.isEnabled = true
        progress.stopAnimation(nil)
        status.stringValue = message
        updateActions()
    }

    private static func setRemovalDetails(_ text: String, on alert: NSAlert) {
        let view = NSTextView(frame: NSRect(x: 0, y: 0, width: 560, height: 280))
        view.string = text
        view.isEditable = false
        view.isSelectable = true
        view.isHorizontallyResizable = false
        view.isVerticallyResizable = true
        view.minSize = .zero
        view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        view.autoresizingMask = [.width]
        view.textContainer?.widthTracksTextView = true
        view.font = HistoryLayout.detailFont
        view.textContainerInset = NSSize(width: HistoryLayout.spacing, height: HistoryLayout.spacing)
        view.setAccessibilityLabel(localized("history.remove.preview"))
        let scroll = NSScrollView(frame: view.frame)
        scroll.hasVerticalScroller = true
        scroll.documentView = view
        alert.accessoryView = scroll
    }

    private func executeRemoval(_ request: HistoryRemovalRequest, preview: HistoryRemovalPreview, source: URL) {
        status.stringValue = localized("history.remove.running")
        progress.startAnimation(nil)
        let query = removalQuery
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Swift.Result { try HistoryRemovalReceipt.decode(query(request, source), preview: preview) }
            DispatchQueue.main.async {
                guard let self else { return }
                do {
                    let receipt = try result.get()
                    let receiptDetail = receipt.detail(privacy: self.privacyEnabled)
                    self.finishRemoval(receiptDetail)
                    if self.removalFixture { self.entries = []; self.applyFilter() }
                    else { self.refresh(directory: source) }
                    if let window = self.window, window.isVisible {
                        let alert = NSAlert()
                        alert.messageText = localized(receipt.status == "completed" ? "history.remove.completed" : "history.remove.partial", receipt.removed_runs, receipt.removed_artifacts)
                        alert.informativeText = localized("history.remove.scope")
                        Self.setRemovalDetails(receiptDetail, on: alert)
                        alert.alertStyle = receipt.status == "completed" ? .informational : .warning
                        alert.beginSheetModal(for: window)
                    }
                } catch {
                    // A failed response can follow a committed transaction; never claim that nothing changed.
                    self.finishRemoval(error.localizedDescription)
                    self.detail.string = error.localizedDescription
                }
            }
        }
    }

    func showRemovalFixture() {
        removalFixture = true
        var fixture = HistoryEntry(id: "fixture:0", stamp: "fixture-session", audit: directory.appendingPathComponent("history.sqlite3"), timestamp: "2026-10-25T09:30:00Z")
        fixture.context = HistoryContext(schema_version: 1, inputs: ["/synthetic/Private holiday photographs"], output: nil,
            backup: nil, mode: "standard", dry_run: false, in_place: false, shortest_path: false,
            version: "0.12.0", retry: nil, resume: nil, no_resume: nil, output_sequence: 0, run_id: "fixture-run-one")
        fixture.finished = HistoryFinished(schema_version: 1, outcome: "completed", error: nil, output_sequence: 4)
        fixture.summary = HistorySummary(schema_version: 1, count_scope: "processor_outcomes",
            img: HistoryMedia(active: true, succeeded: 12, skipped: 2, ignored: 0, failed: 0, unprocessed: 0, exit_code: 0),
            vid: HistoryMedia(active: false, succeeded: nil, skipped: nil, ignored: nil, failed: nil, unprocessed: nil, exit_code: nil),
            integrity: HistorySummary.Integrity(state: "CLEAN", issue_count: 0), failed_files: [], skipped_files: [])
        var second = HistoryEntry(id: "fixture:1", stamp: "fixture-session-two", audit: fixture.audit, timestamp: "2026-10-25T09:00:00Z",
            context: fixture.context, summary: fixture.summary, finished: fixture.finished)
        second.context = HistoryContext(schema_version: 1, inputs: ["/synthetic/Second private folder"], output: nil,
            backup: nil, mode: "standard", dry_run: false, in_place: false, shortest_path: false,
            version: "0.12.0", retry: nil, resume: nil, no_resume: nil, output_sequence: 0, run_id: "fixture-run-two")
        entries = [fixture, second]
        removalQuery = historyRemovalFixtureOutput
        applyFilter()
        show()
    }

    @objc private func exportSelectedRun() {
        guard !privacyEnabled, exportButton.isEnabled, let entry = selected, let window, window.isVisible else { return }
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
                    guard let self, !self.privacyEnabled, self.exportGeneration == token, self.window?.isVisible == true,
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
        guard !privacyEnabled, let url = selected?.logs.first, HistoryStore.regularFile(url), NSWorkspace.shared.open(url) else {
            status.stringValue = localized("history.issue.open_log"); return
        }
    }

    private func resetOutput() {
        clearCleanupDetails()
        clearFailureDetails()
        cancelIssueScan()
        batchIssueResult = nil
        ignoredSources = nil
        ignoredView.reset(localized("history.ignored.loading"))
        comparisonIssues = nil
        outputGeneration = UUID()
        outputLoading = false
        outputPage = nil
        outputFilter.selectedSegment = 0
        detailMode.selectedSegment = 0
    }

    @objc private func detailModeChanged() {
        clearCleanupDetails()
        clearFailureDetails()
        if privacyEnabled, detailMode.selectedSegment == 4 { detailMode.selectedSegment = 0 }
        if selectedEntries.count == 2 {
            if detailMode.selectedSegment == 2 {
                if let issues = comparisonIssues { displayFailureComparison(issues) }
                else { loadFailureComparison(entries: selectedEntries) }
            } else {
                cancelIssueScan()
                detail.string = Self.renderComparison(selectedEntries, privacy: privacyEnabled)
                updateHistoryStatus()
            }
            updateActions()
            return
        }
        guard selected != nil else { showSelected(); return }
        outputGeneration = UUID()
        outputLoading = false
        if ![2, 3, 4].contains(detailMode.selectedSegment)
            || (issueScanControl != nil && issueScanIsIgnored != (detailMode.selectedSegment == 4)) { cancelIssueScan() }
        if detailMode.selectedSegment == 0 {
            detail.string = selected.map { Self.render($0, privacy: privacyEnabled) } ?? localized("history.select")
            updateHistoryStatus()
        }
        else if detailMode.selectedSegment == 2 {
            filterFailedFiles()
            updateHistoryStatus()
            if batchIssueResult == nil, issueScanControl == nil, let entry = selected, entry.hasStoredOutput { loadBatchIssues(entry: entry) }
        }
        else if detailMode.selectedSegment == 3 {
            if batchIssueResult != nil { renderBatchIssues() }
            else if issueScanControl == nil, let entry = selected { loadBatchIssues(entry: entry) }
        }
        else if detailMode.selectedSegment == 4 {
            if let ignoredSources { ignoredView.show(ignoredSources) }
            else if issueScanControl == nil, let entry = selected { loadBatchIssues(entry: entry) }
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
        guard failureDisplayRows.indices.contains(failedTable.selectedRow) else { return nil }
        let row = failureDisplayRows[failedTable.selectedRow]
        let path = row.absolutePath ?? row.path
        guard path.hasPrefix("/"), !path.utf8.contains(0) else { return nil }
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
        clearFailureDetails()
        let paths = selected?.summary?.failed_files ?? []
        let query = failedSearch.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let grouped = Dictionary(grouping: batchIssueResult?.fileFailures ?? [], by: FailureSourceKey.init)
        let sourceRows = grouped.map { key, failures -> FailureDisplayRow in
            var repetitions: [HistoryFileFailure: Int] = [:]
            for failure in failures { repetitions[failure, default: 0] += 1 }
            let observations = repetitions.map { FailureObservation(failure: $0.key, repetitions: $0.value) }
                .sorted { ($0.failure.message, $0.failure.reason_code ?? "") < ($1.failure.message, $1.failure.reason_code ?? "") }
            let reason = observations.count == 1 ? observations[0].failure.userReason
                : localized("history.file_failure.reason.multiple", observations.count)
            return FailureDisplayRow(path: key.sourcePath, absolutePath: key.absolutePath,
                                     reason: reason, source: key, observations: observations)
        }.sorted { $0.source!.ordering.lexicographicallyPrecedes($1.source!.ordering) }
        let recordedPaths = Set(sourceRows.map { Data($0.path.utf8) })
        var seenPaths = Set<Data>()
        let missingRows = paths.filter {
            let bytes = Data($0.utf8)
            return !recordedPaths.contains(bytes) && seenPaths.insert(bytes).inserted
        }.map {
            FailureDisplayRow(path: $0, absolutePath: nil, reason: localized("history.file_failure.not_recorded"),
                              source: nil, observations: [])
        }
        let candidates = sourceRows + missingRows
        var matches = 0
        var bytes = 0
        failureDisplayRows = []
        for row in candidates where query.isEmpty || row.searchable.localizedCaseInsensitiveContains(query) {
            matches += 1
            let length = row.path.utf8.count + row.reason.utf8.count + (row.absolutePath?.utf8.count ?? 0)
            if failureDisplayRows.count < HistoryStore.maximumOutputLines, length <= HistoryStore.maximumOutputBytes - bytes {
                failureDisplayRows.append(row)
                bytes += length
            }
        }
        failedPaths = failureDisplayRows.map(\.path)
        failedTable.deselectAll(nil)
        failedTable.reloadData()
        failedStatus.stringValue = (candidates.isEmpty ? localized("history.failed_files.empty")
            : localized("history.failed_files.range", failedPaths.count, matches, candidates.count))
            + "\n" + (batchIssueResult == nil ? localized("history.failed_files.note")
                : localized("history.file_failure.source_observations", sourceRows.count, batchIssueResult!.fileFailures.count)
                    + "\n" + localized("history.file_failure.note"))
        updateActions()
    }

    private func clearCleanupDetails() {
        cleanupDetailGeneration = UUID()
        if let alert = cleanupDetailAlert, let window, window.attachedSheet === alert.window {
            window.endSheet(alert.window)
        }
        cleanupDetailAlert = nil
    }

    private static func cleanupDirectories(for entry: HistoryEntry, privacy: Bool = false) -> [URL] {
        guard !entry.outputCleanupInvalid else { return [] }
        return entry.outputCleanups.compactMap { receipt in
            if privacy && !receipt.needsAttention { return nil }
            let url = URL(fileURLWithPath: receipt.output_path)
            guard let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey]),
                  values.isDirectory == true, values.isSymbolicLink != true else { return nil }
            return url
        }
    }

    private static func cleanupDetailText(for entry: HistoryEntry, privacy: Bool = false) throws -> String {
        if privacy {
            return ([localized("cleanup.history_title"), localized("privacy.display.scope")]
                + entry.outputCleanups.filter(\.needsAttention).map(\.detailText)
                + (entry.outputCleanupInvalid ? [localized("cleanup.invalid")] : [])).joined(separator: "\n\n")
        }
        let state = entry.outputCleanupInvalid ? "inconsistent" : entry.outputCleanups.isEmpty ? "unavailable" : "recorded"
        let object: [String: Any] = ["run_id": entry.context?.run_id as Any? ?? NSNull(),
            "state": state, "receipts": entry.outputCleanupInvalid ? [] : entry.outputCleanups.map(\.recordedObject)]
        let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        let details = entry.outputCleanupInvalid ? localized("cleanup.invalid") : entry.outputCleanups.isEmpty
            ? localized("cleanup.not_recorded") : entry.outputCleanups.map(\.detailText).joined(separator: "\n\n")
        return [localized("cleanup.history_title"), localized("cleanup.inspect.note"), details,
            localized("history.file_failure.detail.json"), String(decoding: data, as: UTF8.self)].joined(separator: "\n\n")
    }

    @objc private func openCleanupDetails() {
        guard cleanupButton.isEnabled, let entry = selected, let window, window.attachedSheet == nil else { return }
        do {
            let text = try Self.cleanupDetailText(for: entry, privacy: privacyEnabled)
            let alert = NSAlert()
            alert.alertStyle = .informational
            alert.messageText = localized("cleanup.inspect")
            alert.informativeText = localized("cleanup.inspect.note")
            let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 320))
            textView.string = text
            textView.isEditable = false
            textView.isSelectable = true
            textView.isHorizontallyResizable = false
            textView.isVerticallyResizable = true
            textView.minSize = .zero
            textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
            textView.autoresizingMask = [.width]
            textView.textContainer?.widthTracksTextView = true
            textView.font = HistoryLayout.detailFont
            textView.textContainerInset = NSSize(width: HistoryLayout.spacing, height: HistoryLayout.spacing)
            textView.setAccessibilityLabel(localized("cleanup.history_title"))
            let scroll = NSScrollView(frame: textView.frame)
            scroll.hasVerticalScroller = true
            scroll.documentView = textView
            alert.accessoryView = scroll
            alert.addButton(withTitle: localized("history.file_failure.detail.close"))
            alert.addButton(withTitle: localized("cleanup.inspect.copy"))
            let reveal = alert.addButton(withTitle: localized("cleanup.inspect.reveal"))
            reveal.isEnabled = !Self.cleanupDirectories(for: entry, privacy: privacyEnabled).isEmpty
            if !reveal.isEnabled { reveal.toolTip = localized("cleanup.inspect.unavailable") }
            let token = UUID()
            cleanupDetailGeneration = token
            cleanupDetailAlert = alert
            alert.beginSheetModal(for: window) { [weak self] response in
                guard let self, self.cleanupDetailGeneration == token else { return }
                self.cleanupDetailAlert = nil
                if response == .alertSecondButtonReturn {
                    NSPasteboard.general.clearContents()
                    self.status.stringValue = localized(NSPasteboard.general.setString(text, forType: .string)
                        ? "cleanup.inspect.copied" : "cleanup.inspect.copy_failed")
                } else if response == .alertThirdButtonReturn {
                    let urls = Self.cleanupDirectories(for: entry, privacy: self.privacyEnabled)
                    guard !urls.isEmpty else {
                        self.status.stringValue = localized("cleanup.inspect.unavailable"); return
                    }
                    NSWorkspace.shared.activateFileViewerSelecting(urls)
                }
            }
        } catch { status.stringValue = error.localizedDescription }
    }

    private func clearFailureDetails() {
        failureDetailGeneration = UUID()
        if let alert = failureDetailAlert, let window, window.attachedSheet === alert.window {
            window.endSheet(alert.window)
        }
        failureDetailAlert = nil
    }

    private static func failureWorkerDetails(for source: FailureSourceKey, entry: HistoryEntry?, privacy: Bool) throws -> [String] {
        var lines = [localized("history.file_failure.worker.title"), localized("history.file_failure.worker.note")]
        guard let entry, !entry.invocationEvidenceInvalid, entry.context?.run_id == source.runID else {
            return lines + [localized("history.file_failure.worker.unavailable")]
        }
        let matches = entry.invocations.filter { $0.invocation_id == source.invocationID }
        guard matches.count == 1, let invocation = matches.first, invocation.valid,
              invocation.run_id == source.runID, invocation.pipeline == source.pipeline else {
            return lines + [localized("history.file_failure.worker.unavailable")]
        }
        func row(_ key: String, _ value: String) -> String { localized("history.detail.\(key)") + ": " + value }
        let unknown = localized("result.unknown")
        if privacy {
            // A worker can receive normal files alongside this failure; its paths and argv are not file-scoped.
            lines.append(localized("history.file_failure.worker.private"))
        } else {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .withoutEscapingSlashes]
            lines += [row("worker_program", invocation.program_requested), row("worker_path", invocation.program_path ?? unknown),
                      row("worker_arguments", String(decoding: try encoder.encode(invocation.arguments), as: UTF8.self))]
        }
        if let bytes = invocation.binary.bytes, let hash = invocation.binary.blake3 {
            lines.append(row("worker_binary", "\(bytes) B · \(hash)"))
        } else {
            lines.append(row("worker_observation_error", privacy ? localized("privacy.display.hidden") : invocation.binary.diagnostic ?? unknown))
        }
        let observations = entry.workerExecutions[source.invocationID] ?? []
        guard !entry.workerExecutionEvidenceInvalid else {
            return lines + [localized("history.worker.execution.invalid")]
        }
        guard !observations.isEmpty else { return lines + [localized("history.worker.execution.unknown")] }
        guard observations.count <= 2, let first = observations.first, let last = observations.last,
              first.state == "spawned", observations.allSatisfy({
                  $0.valid && $0.run_id == source.runID && $0.invocation_id == source.invocationID
                      && $0.pipeline == source.pipeline && $0.pid == first.pid
              }), observations.count == 1 || last.state == "exited" else {
            return lines + [localized("history.worker.execution.invalid")]
        }
        lines.append(localized("history.worker.execution.pid", String(last.pid)))
        if last.state == "spawned" { lines.append(localized("history.worker.execution.spawned")) }
        else if let signal = last.signal { lines.append(localized("history.worker.execution.signal", String(signal))) }
        else { lines.append(localized("history.worker.execution.exited", last.exit_code.map(String.init) ?? unknown)) }
        return lines
    }

    private func failureDetailText(for row: FailureDisplayRow) throws -> String {
        guard let source = row.source, !row.observations.isEmpty else {
            throw HostError(message: localized("history.file_failure.not_recorded"))
        }
        let jsonEncoder = JSONEncoder()
        jsonEncoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        var lines = [localized("history.detail.run_id") + ": " + source.runID,
                     localized("history.detail.invocation_id") + ": " + source.invocationID,
                     localized("history.media.\(source.pipeline)") + ": " + source.pipeline,
                     localized("history.failed_files.path") + ": " + source.sourcePath,
                     localized("history.file_failure.detail.absolute_path") + ": " + (source.absolutePath ?? localized("result.unknown")),
                     localized("history.file_failure.detail.note"), ""]
        lines += try Self.failureWorkerDetails(for: source, entry: selected, privacy: privacyEnabled)
        lines.append("")
        for observation in row.observations {
            let json = try jsonEncoder.encode(observation.failure)
            lines += [observation.failure.userReason, observation.failure.message,
                      localized("history.file_failure.detail.observations", observation.repetitions),
                      localized("history.file_failure.detail.json"), String(decoding: json, as: UTF8.self), ""]
        }
        return lines.joined(separator: "\n")
    }

    @objc private func openFailureDetails() {
        guard detailMode.selectedSegment == 2, refreshButton.isEnabled,
              failureDisplayRows.indices.contains(failedTable.selectedRow),
              let window else { return }
        let row = failureDisplayRows[failedTable.selectedRow]
        do {
            let text = try failureDetailText(for: row)
            let alert = NSAlert()
            alert.alertStyle = .informational
            alert.messageText = localized("history.file_failure.detail.title")
            alert.informativeText = localized("history.file_failure.detail.observations",
                row.observations.reduce(0) { $0 + $1.repetitions })
            let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: 640, height: 340))
            textView.string = text
            textView.isEditable = false
            textView.isSelectable = true
            textView.isHorizontallyResizable = false
            textView.isVerticallyResizable = true
            textView.minSize = .zero
            textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
            textView.autoresizingMask = [.width]
            textView.textContainer?.widthTracksTextView = true
            textView.setAccessibilityLabel(localized("history.file_failure.detail.title"))
            textView.font = HistoryLayout.detailFont
            textView.textContainerInset = NSSize(width: HistoryLayout.spacing, height: HistoryLayout.spacing)
            let scroll = NSScrollView(frame: textView.frame)
            scroll.hasVerticalScroller = true
            scroll.documentView = textView
            alert.accessoryView = scroll
            alert.addButton(withTitle: localized("history.file_failure.detail.copy"))
            alert.addButton(withTitle: localized("history.file_failure.detail.close"))
            let token = UUID()
            failureDetailGeneration = token
            failureDetailAlert = alert
            alert.beginSheetModal(for: window) { [weak self] response in
                guard let self, self.failureDetailGeneration == token else { return }
                self.failureDetailAlert = nil
                if response == .alertFirstButtonReturn {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                    self.status.stringValue = localized("history.file_failure.detail.copied")
                }
            }
        } catch {
            status.stringValue = error.localizedDescription
        }
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
        if privacyEnabled {
            let rows = page.rows.filter { row in
                row.line.map { DisplayPrivacy.problemTone($0) != nil } ?? true
            }
            detail.string = label + "\n" + localized("privacy.display.output_notice") + "\n\n"
                + rows.map(\.text).joined(separator: "\n")
        } else if outputFilter.selectedSegment == 1 {
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

    private struct FailureComparisonKey: Hashable {
        let pipeline: String
        let path: String
        let absolutePath: String?
        let message: String
        let code: String?

        init(_ failure: HistoryFileFailure) {
            pipeline = failure.pipeline
            path = failure.source_path
            absolutePath = failure.absolute_path
            message = failure.message
            code = failure.reason_code
        }
        var ordering: [String] { [path, absolutePath ?? "", pipeline, message, code ?? ""] }
    }

    fileprivate static func renderFailureComparison(_ results: [HistoryStore.BatchIssues]) throws -> String {
        guard results.count == 2 else { throw HostError(message: localized("history.compare.select_two")) }
        let observations = results.map { result in
            Dictionary(grouping: result.fileFailures, by: FailureComparisonKey.init).mapValues(\.count)
        }
        let keys = Set(observations[0].keys).union(observations[1].keys)
        guard keys.count <= 1_000 else { throw HostError(message: localized("history.batch_issues.limit")) }
        let shared = keys.filter { observations[0][$0] != nil && observations[1][$0] != nil }
        let first = keys.filter { observations[1][$0] == nil }
        let second = keys.filter { observations[0][$0] == nil }
        var sections = [localized("history.compare.failures"), localized("history.compare.failures.note")]
        for (index, result) in results.enumerated() {
            sections.append(localized("history.compare.batch", index + 1) + "\n"
                + localized(result.endSource == "snapshot_only" ? "history.batch_issues.scope_snapshot" : "history.batch_issues.scope_closed",
                            result.scannedRows, result.start, result.upper))
        }
        sections.append(localized("history.compare.failures.totals", shared.count, first.count, second.count))
        for (label, group) in [("shared", shared), ("first_only", first), ("second_only", second)] {
            var lines = [localized("history.compare.failures.\(label)")]
            for key in group.sorted(by: { $0.ordering.lexicographicallyPrecedes($1.ordering) }) {
                lines += ["", localized("history.media.\(key.pipeline)") + ": " + key.path,
                          localized("history.compare.failures.observations", observations[0][key] ?? 0, observations[1][key] ?? 0),
                          key.message]
                if let absolute = key.absolutePath, absolute != key.path { lines.append(absolute) }
                if let code = key.code { lines.append(code) }
            }
            if group.isEmpty { lines.append(localized("history.compare.failures.empty")) }
            sections.append(lines.joined(separator: "\n"))
        }
        let text = sections.joined(separator: "\n\n")
        guard text.utf8.count <= 2 * 1_024 * 1_024 else { throw HostError(message: localized("history.batch_issues.limit")) }
        return text
    }

    private func displayFailureComparison(_ results: [HistoryStore.BatchIssues]) {
        do {
            let identities = selectedEntries.enumerated().map { index, entry in
                localized("history.compare.batch", index + 1) + ": " + entry.stamp + " · " + Self.timestampLabel(entry.timestamp)
                    + "\n" + localized("history.detail.run_id") + ": " + (entry.context?.run_id ?? localized("result.unknown"))
            }
            let text = identities.joined(separator: "\n\n") + "\n\n" + (try Self.renderFailureComparison(results))
            guard text.utf8.count <= HistoryStore.maximumOutputBytes else { throw HostError(message: localized("history.batch_issues.limit")) }
            detail.string = text
        }
        catch { detail.string = error.localizedDescription }
        detail.scrollToBeginningOfDocument(nil)
        updateHistoryStatus()
    }

    private func loadFailureComparison(entries: [HistoryEntry]) {
        cancelIssueScan()
        let token = UUID()
        issueGeneration = token
        let control = HistoryStore.IssueScanControl()
        issueScanControl = control
        comparisonIssues = nil
        detail.string = localized("history.compare.failures.loading")
        status.stringValue = detail.string
        updateActions()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Swift.Result { try entries.map { try HistoryStore.batchIssues(for: $0, control: control) } }
            DispatchQueue.main.async {
                guard let self, self.issueGeneration == token, self.selectedEntries.map(\.id) == entries.map(\.id),
                      self.detailMode.selectedSegment == 2, !control.isCancelled else { return }
                self.issueScanControl = nil
                switch result {
                case let .success(issues):
                    self.comparisonIssues = issues
                    self.displayFailureComparison(issues)
                case let .failure(error):
                    self.comparisonIssues = nil
                    self.detail.string = error.localizedDescription + "\n" + localized("history.compare.failures.unavailable")
                    self.status.stringValue = error.localizedDescription
                }
                self.updateActions()
            }
        }
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
        let ignoredOnly = detailMode.selectedSegment == 4
        issueScanIsIgnored = ignoredOnly
        if ignoredOnly {
            ignoredSources = nil
            ignoredView.reset(localized("history.ignored.loading"))
        } else { batchIssueResult = nil }
        detail.string = localized("history.batch_issues.loading")
        if detailMode.selectedSegment == 2 { failedStatus.stringValue = localized("history.file_failure.loading") }
        status.stringValue = detail.string
        updateActions()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Swift.Result { try HistoryStore.batchIssues(for: entry, control: control, ignoredOnly: ignoredOnly) }
            DispatchQueue.main.async {
                guard let self, self.issueGeneration == token, self.selected?.id == entry.id,
                      (ignoredOnly ? self.detailMode.selectedSegment == 4 : [2, 3].contains(self.detailMode.selectedSegment)), !control.isCancelled else { return }
                self.issueScanControl = nil
                switch result {
                case let .success(issues):
                    if ignoredOnly {
                        self.ignoredSources = issues.ignoredSources
                        self.ignoredView.show(issues.ignoredSources)
                        self.updateHistoryStatus()
                    } else {
                        self.batchIssueResult = issues
                        if self.detailMode.selectedSegment == 2 {
                            self.filterFailedFiles()
                            self.updateHistoryStatus()
                        }
                        else { self.renderBatchIssues() }
                    }
                case let .failure(error):
                    if ignoredOnly {
                        self.ignoredSources = nil
                        self.ignoredView.reset(error.localizedDescription)
                    } else {
                        self.batchIssueResult = nil
                        self.detail.string = error.localizedDescription
                        if self.detailMode.selectedSegment == 2 {
                            self.filterFailedFiles()
                            self.failedStatus.stringValue = error.localizedDescription + "\n" + localized("history.file_failure.not_recorded")
                        }
                    }
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
    private var visibleCopyText: String {
        if !failedContent.isHidden {
            return ([failedStatus.stringValue] + failureDisplayRows.map { $0.path + "\n" + $0.reason }).joined(separator: "\n")
        }
        return detail.string
    }

    @objc private func copySources() {
        if privacyEnabled {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(visibleCopyText, forType: .string)
            status.stringValue = localized("history.copied")
            return
        }
        var seen = Set<String>()
        let paths = selectedEntries.flatMap { $0.context?.inputs ?? [] }.filter { seen.insert($0).inserted }
        guard !paths.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(paths.joined(separator: "\n"), forType: .string)
        status.stringValue = localized("history.copied")
    }

    fileprivate func validateCleanupForSelfTest(_ entry: HistoryEntry) throws {
        entries = [entry]
        filter.selectedSegment = 0
        search.stringValue = ""
        applyFilter()
        show()
        guard cleanupButton.isEnabled else { throw HostError(message: "Cleanup inspection is missing for a recorded Fast IMG batch") }
        let text = try Self.cleanupDetailText(for: entry)
        guard text.contains(entry.context!.run_id!), text.contains("\"removed_files\""),
              text.contains("\"outcome\""), text.contains("\"residual\"") else {
            throw HostError(message: "Cleanup inspection lost the original run-bound receipt")
        }
        openCleanupDetails()
        guard let alert = cleanupDetailAlert, window?.attachedSheet === alert.window,
              alert.buttons.count == 3, alert.buttons[1].title == localized("cleanup.inspect.copy"),
              alert.buttons[2].isEnabled == false,
              let scroll = alert.accessoryView as? NSScrollView, let view = scroll.documentView as? NSTextView,
              !view.isEditable, view.isSelectable, !scroll.hasHorizontalScroller,
              view.string == text else { throw HostError(message: "Cleanup inspector is not read-only or enables an unavailable folder") }
        resetOutput()
        guard cleanupDetailAlert == nil, window?.attachedSheet == nil else {
            throw HostError(message: "A stale cleanup inspector remained after selection reset")
        }
        var old = entry
        old.outputCleanups = []
        guard try Self.cleanupDetailText(for: old).contains(localized("cleanup.not_recorded")),
              Self.cleanupDirectories(for: old).isEmpty else {
            throw HostError(message: "Missing cleanup evidence invented actionable folders")
        }
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        defer {
            do { try FileManager.default.removeItem(at: root) }
            catch { fputs("MFB: cleanup inspector fixture removal failed: \(error.localizedDescription)\n", stderr) }
        }
        let folder = root.appendingPathComponent("output\n映像", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
        let recorded = entry.outputCleanups[0]
        var current = entry
        current.outputCleanups = [OutputCleanupReceipt(schema_version: 1, run_id: recorded.run_id, output_path: folder.path,
            outcome: recorded.outcome, removed_files: recorded.removed_files, already_absent_files: recorded.already_absent_files,
            pruned_directories: recorded.pruned_directories, error: recorded.error)]
        guard Self.cleanupDirectories(for: current).map(\.path) == [folder.path] else {
            throw HostError(message: "An accessible recorded directory cannot be located")
        }
        try FileManager.default.removeItem(at: folder)
        try FileManager.default.createSymbolicLink(at: folder, withDestinationURL: root)
        guard Self.cleanupDirectories(for: current).isEmpty else {
            throw HostError(message: "A replaced directory symlink became a cleanup action")
        }
        try FileManager.default.removeItem(at: folder)
        try Data("synthetic non-directory".utf8).write(to: folder)
        guard Self.cleanupDirectories(for: current).isEmpty else {
            throw HostError(message: "A regular file became an output folder action")
        }
        current.outputCleanupInvalid = true
        guard Self.cleanupDirectories(for: current).isEmpty,
              try Self.cleanupDetailText(for: current).contains(localized("cleanup.invalid")) else {
            throw HostError(message: "Conflicting cleanup records became actionable")
        }
        window?.close()
    }

    fileprivate func validateIgnoredForSelfTest(_ entry: HistoryEntry, records: [HistoryIgnoredSource]) throws {
        entries = [entry]
        applyFilter()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        ignoredSources = records
        detailMode.selectedSegment = 4
        detailModeChanged()
        guard !ignoredView.isHidden, failedContent.isHidden, detailScroll.isHidden,
              entry.summary?.img.ignored == records.count, detailMode.isEnabled(forSegment: 4) else {
            throw HostError(message: "Ignored history tab mixed failures, changed counts or lost supported controls")
        }
        try ignoredView.validateForSelfTest(records)
        let originalWindow = window
        let originalContent = window?.contentView
        defer { window?.orderOut(nil) }
        for width in [1_040.0, 780.0, 1_040.0] {
            window?.setContentSize(NSSize(width: width, height: 620))
            show()
            ignoredSources = records
            detailMode.selectedSegment = 4
            detailModeChanged()
            window?.contentView?.layoutSubtreeIfNeeded()
            window?.displayIfNeeded()
            try ignoredView.validateForSelfTest(records)
            close()
            show()
            window?.contentView?.layoutSubtreeIfNeeded()
            window?.displayIfNeeded()
            guard window === originalWindow, window?.contentView === originalContent,
                  entries.map(\.id) == [entry.id], entries.first?.summary?.img.ignored == records.count else {
                throw HostError(message: "Ignored history lost its window, content or authoritative entry after reopening")
            }
            guard window?.isVisible == !isBackgroundSelfTest else {
                throw HostError(message: "Ignored history violated the requested background or interactive presentation")
            }
        }
        resetOutput()
        updateActions()
        guard ignoredSources == nil, ignoredView.isHidden else {
            throw HostError(message: "History reset retained another batch's ignored inventory")
        }
    }

    fileprivate func validateFileReasonsForSelfTest(_ entry: HistoryEntry, issues: HistoryStore.BatchIssues) throws {
        try validateFailureWorkerDetailsForSelfTest(entry, failure: issues.fileFailures[0])
        entries = [entry]
        filter.selectedSegment = 0
        applyFilter()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        batchIssueResult = issues
        detailMode.selectedSegment = 2
        detailModeChanged()
        guard failedTable.numberOfRows == 2, entry.summary?.img.failed == 1,
              failureDisplayRows[0].reason == localized("history.file_failure.reason.multiple", 2),
              failureDisplayRows[0].observations.map(\.repetitions).sorted() == [1, 2],
              failureDisplayRows[1].reason == localized("history.file_failure.not_recorded"),
              failedStatus.stringValue.contains(localized("history.file_failure.source_observations", 1, 3)) else {
            throw HostError(message: "File-linked grouping lost distinct observations or changed final failed-file counts")
        }
        let fullDetails = try failureDetailText(for: failureDisplayRows[0])
        guard fullDetails.contains("synthetic decoder failed\nno guessed root cause"),
              fullDetails.contains("synthetic metadata failure"), fullDetails.contains("decoder_failed"),
              fullDetails.contains(localized("history.file_failure.worker.title")),
              fullDetails.contains(issues.fileFailures[0].source_path),
              fullDetails.contains(issues.fileFailures[0].invocation_id) else {
            throw HostError(message: "Grouped failure details lost original diagnostics, Unicode paths or invocation identity")
        }
        failedTable.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        updateActions()
        guard selectedFailedPath == issues.fileFailures[0].source_path,
              selectedFailedURL == nil, failedCopyButton.isEnabled, !failedRevealButton.isEnabled, failedDetailsButton.isEnabled,
              (tableView(failedTable, viewFor: failedTable.tableColumns[1], row: 0) as? NSTableCellView)?.textField?.toolTip == failureDisplayRows[0].reason else {
            throw HostError(message: "Recorded reasons lost exact paths/tooltips or enabled unavailable Finder navigation")
        }
        failedSearch.stringValue = "METADATA"
        filterFailedFiles()
        guard failedPaths == [issues.fileFailures[0].source_path], failureDisplayRows[0].observations.count == 2,
              !failedCopyButton.isEnabled, !failedDetailsButton.isEnabled else {
            throw HostError(message: "Failure-reason search lost a grouped cause or retained stale actions")
        }
        failedSearch.stringValue = ""
        let original = issues.fileFailures[0]
        let anotherCall = HistoryFileFailure(schema_version: original.schema_version, pipeline: original.pipeline,
            source_path: original.source_path, absolute_path: original.absolute_path, message: original.message,
            run_id: original.run_id, invocation_id: "another-call", reason_code: original.reason_code)
        batchIssueResult = HistoryStore.BatchIssues(groups: [], occurrences: 0,
            fileFailures: [original, original, original, anotherCall], scannedRows: 4, start: 0, upper: 4, endSource: "snapshot_only")
        filterFailedFiles()
        guard failureDisplayRows.filter({ $0.source != nil }).count == 2,
              failedStatus.stringValue.contains(localized("history.file_failure.source_observations", 2, 4)),
              failureDisplayRows.flatMap(\.observations).map(\.repetitions).sorted() == [1, 3] else {
            throw HostError(message: "Source grouping merged different invocations or confused observations with files")
        }
        let composed = HistoryFileFailure(schema_version: 1, pipeline: "img", source_path: "/synthetic/\u{e9}.jpg",
            absolute_path: nil, message: "unknown original", run_id: "r", invocation_id: "i", reason_code: "unrecognized")
        let decomposed = HistoryFileFailure(schema_version: 1, pipeline: "img", source_path: "/synthetic/e\u{301}.jpg",
            absolute_path: nil, message: "unknown original", run_id: "r", invocation_id: "i", reason_code: "unrecognized")
        guard Set([composed, decomposed]).count == 2, Set([FailureSourceKey(composed), FailureSourceKey(decomposed)]).count == 2,
              composed.userReason == "unknown original [unrecognized]" else {
            throw HostError(message: "Source grouping normalized recorded paths or invented an unknown reason")
        }
        batchIssueResult = issues
        detailMode.selectedSegment = 0
        detailModeChanged()
        guard detail.string.contains(entry.context!.run_id!) else { throw HostError(message: "History overview lost run identity") }
    }

    private func validateFailureWorkerDetailsForSelfTest(_ original: HistoryEntry, failure: HistoryFileFailure) throws {
        let source = FailureSourceKey(failure)
        let arguments = ["--input", "/synthetic/routine.jpg", "line\n入力"]
        func invocation(_ id: String, run: String? = nil, pipeline: String = "img", program: String = "synthetic-worker-one") -> HistoryInvocation {
            HistoryInvocation(schema_version: 1, invocation_id: id, run_id: run ?? source.runID,
                pipeline: pipeline, state: "launch_requested", program_requested: program,
                program_path: "/synthetic/bin/" + program, arguments: arguments,
                binary: .init(scope: "observed_before_launch", bytes: 42, blake3: String(repeating: "a", count: 64), diagnostic: nil))
        }
        func observation(_ state: String, pid: UInt32 = 73, run: String? = nil, pipeline: String = "img", exit: Int32? = nil, signal: Int32? = nil) -> HistoryWorkerExecution {
            HistoryWorkerExecution(schema_version: 1, scope: "os_child_process", invocation_id: source.invocationID,
                run_id: run ?? source.runID, pipeline: pipeline, state: state, pid: pid, exit_code: exit, signal: signal)
        }
        func text(_ entry: HistoryEntry?, privacy: Bool = false, source key: FailureSourceKey? = nil) throws -> String {
            try Self.failureWorkerDetails(for: key ?? source, entry: entry, privacy: privacy).joined(separator: "\n")
        }
        var entry = original
        entry.invocationEvidenceInvalid = false
        entry.workerExecutionEvidenceInvalid = false
        entry.invocations = [invocation(source.invocationID), invocation("another-call", program: "synthetic-worker-two")]
        entry.workerExecutions = [source.invocationID: [observation("spawned"), observation("exited", exit: 7)]]
        let complete = try text(entry)
        guard complete.contains("synthetic-worker-one"), !complete.contains("synthetic-worker-two"),
              complete.contains("line\\n入力"), complete.contains(String(repeating: "a", count: 64)),
              complete.contains(localized("history.worker.execution.pid", "73")),
              complete.contains(localized("history.worker.execution.exited", "7")) else {
            throw HostError(message: "Failure worker details lost exact invocation, arguments, binary identity or process exit")
        }
        let anotherFailure = HistoryFileFailure(schema_version: failure.schema_version, pipeline: source.pipeline,
            source_path: failure.source_path, absolute_path: failure.absolute_path, message: failure.message,
            run_id: source.runID, invocation_id: "another-call", reason_code: failure.reason_code)
        let another = try text(entry, source: FailureSourceKey(anotherFailure))
        guard another.contains("synthetic-worker-two"), !another.contains("synthetic-worker-one"),
              another.contains(localized("history.worker.execution.unknown")) else {
            throw HostError(message: "Failure details borrowed the executable or process result of another invocation")
        }
        let hidden = try text(entry, privacy: true)
        guard hidden.contains(localized("history.file_failure.worker.private")),
              !hidden.contains(arguments[1]), !hidden.contains("line\\n入力"), !hidden.contains("/synthetic/bin/"),
              hidden.contains(localized("history.worker.execution.exited", "7")) else {
            throw HostError(message: "Failure worker privacy leaked routine arguments or lost the process result")
        }
        var diagnosticEntry = entry
        let diagnostic = "Cannot observe /synthetic/bin/private-worker while reading " + arguments[1]
        diagnosticEntry.invocations = [HistoryInvocation(schema_version: 1, invocation_id: source.invocationID,
            run_id: source.runID, pipeline: source.pipeline, state: "launch_requested", program_requested: "private-worker",
            program_path: "/synthetic/bin/private-worker", arguments: arguments,
            binary: .init(scope: "observed_before_launch", bytes: nil, blake3: nil, diagnostic: diagnostic))]
        let privateDiagnostic = try text(diagnosticEntry, privacy: true)
        guard !privateDiagnostic.contains(arguments[1]), !privateDiagnostic.contains("/synthetic/bin/private-worker"),
              privateDiagnostic.contains(localized("privacy.display.hidden")), try text(diagnosticEntry).contains(diagnostic) else {
            throw HostError(message: "Failure worker observation diagnostics leaked routine paths or lost retained details")
        }
        var invalidEntries: [HistoryEntry] = []
        for invocations in [[], [invocation(source.invocationID), invocation(source.invocationID)],
                            [invocation(source.invocationID, run: "wrong-run")],
                            [invocation(source.invocationID, pipeline: "vid")]] {
            var invalid = entry
            invalid.invocations = invocations
            invalidEntries.append(invalid)
        }
        var invalid = entry
        invalid.invocationEvidenceInvalid = true
        invalidEntries.append(invalid)
        for invalid in invalidEntries {
            let unavailable = try text(invalid)
            guard unavailable.contains(localized("history.file_failure.worker.unavailable")),
                  !unavailable.contains("synthetic-worker-one"), !unavailable.contains(localized("history.worker.execution.pid", "73")) else {
                throw HostError(message: "Failure details guessed an identity from missing, conflicting or mismatched invocation evidence")
            }
        }
        for observations in [[observation("exited", exit: 7)],
                             [observation("spawned"), observation("exited", pid: 74, exit: 7)],
                             [observation("spawned"), observation("exited", run: "wrong-run", exit: 7)],
                             [observation("spawned"), observation("exited", pipeline: "vid", exit: 7)]] {
            invalid = entry
            invalid.workerExecutions[source.invocationID] = observations
            let unavailable = try text(invalid)
            guard unavailable.contains(localized("history.worker.execution.invalid")),
                  !unavailable.contains(localized("history.worker.execution.exited", "7")) else {
                throw HostError(message: "Failure details displayed an unbound or conflicting process result")
            }
        }
        invalid = entry
        invalid.workerExecutionEvidenceInvalid = true
        guard try text(invalid).contains(localized("history.worker.execution.invalid")),
              try text(nil).contains(localized("history.file_failure.worker.unavailable")) else {
            throw HostError(message: "Failure details hid invalid execution evidence or invented a missing batch")
        }
        invalid.workerExecutionEvidenceInvalid = false
        invalid.workerExecutions[source.invocationID] = [observation("spawned"), observation("exited", signal: 15)]
        guard try text(invalid).contains(localized("history.worker.execution.signal", "15")) else {
            throw HostError(message: "Failure details lost the actual worker termination signal")
        }
        invalid.workerExecutions[source.invocationID] = [observation("spawned")]
        let started = try text(invalid)
        guard started.contains(localized("history.worker.execution.spawned")),
              !started.contains(localized("history.worker.execution.exited", "7")) else {
            throw HostError(message: "Failure details turned a start observation into a process exit")
        }
    }

    fileprivate func validateFailureComparisonForSelfTest(_ entry: HistoryEntry, results: [HistoryStore.BatchIssues]) throws {
        var other = HistoryEntry(id: "comparison-other", stamp: "comparison-other", audit: entry.audit, timestamp: entry.timestamp)
        other.context = entry.context
        entries = [entry, other]
        filter.selectedSegment = 0
        applyFilter()
        table.selectRowIndexes(IndexSet(integersIn: 0...1), byExtendingSelection: false)
        showSelected()
        comparisonIssues = results
        detailMode.selectedSegment = 2
        detailModeChanged()
        guard detailMode.isEnabled(forSegment: 2), failedContent.isHidden, !detailScroll.isHidden,
              !failedCopyButton.isEnabled, !failedRevealButton.isEnabled, !exportButton.isEnabled,
              detail.string.contains(localized("history.compare.failures.note")),
              detail.string.contains(entry.stamp), detail.string.contains(other.stamp),
              detail.string.contains(entry.context!.run_id!) else {
            throw HostError(message: "Native reason comparison retained single-file actions or lost its evidence scope")
        }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        showSelected()
        guard comparisonIssues == nil, issueScanControl == nil, detailMode.selectedSegment == 0 else {
            throw HostError(message: "Changing selection retained the previous reason comparison")
        }
    }

    func validatePrivacyForSelfTest() throws {
        let original = preferences.object(forKey: DisplayPrivacy.preferenceKey)
        defer {
            if let original { preferences.set(original, forKey: DisplayPrivacy.preferenceKey) }
            else { preferences.removeObject(forKey: DisplayPrivacy.preferenceKey) }
        }
        let normalPath = "/synthetic/private/error_failed_[WARN].jpg"
        let problemPath = "/synthetic/problem.jpg"
        var entry = HistoryEntry(id: "privacy:0", stamp: "privacy", audit: URL(fileURLWithPath: "/synthetic/history.sqlite3"),
            timestamp: "2026-01-01T00:00:00Z")
        entry.context = HistoryContext(schema_version: 1, inputs: [normalPath], output: "/synthetic/private/output",
            backup: nil, mode: "auto", dry_run: false, in_place: false, shortest_path: false, version: "synthetic",
            retry: false, resume: false, no_resume: false, output_sequence: 0, run_id: "run-privacy")
        let media = HistoryMedia(active: true, succeeded: 1, skipped: 0, ignored: 0, failed: 1, unprocessed: 0, exit_code: 1)
        let inactive = HistoryMedia(active: false, succeeded: nil, skipped: nil, ignored: nil, failed: nil, unprocessed: nil, exit_code: nil)
        entry.summary = HistorySummary(schema_version: 1, count_scope: "processor_outcomes", img: media, vid: inactive,
            integrity: .init(state: "CLEAN", issue_count: 0), failed_files: [problemPath], skipped_files: [normalPath])
        entry.finished = HistoryFinished(schema_version: 1, outcome: "failed", error: "decoder failed: " + problemPath, output_sequence: 2)
        entry.invocations = [HistoryInvocation(schema_version: 1, invocation_id: "invocation-privacy", run_id: "run-privacy",
            pipeline: "img", state: "launch_requested", program_requested: "img", program_path: "/synthetic/bin/img",
            arguments: ["--input", normalPath], binary: .init(scope: "observed_before_launch", bytes: 1,
                blake3: String(repeating: "a", count: 64), diagnostic: nil))]
        entry.configSnapshots = [HistoryConfigSnapshot(pipeline: "img", packageVersion: "synthetic",
            values: #"{"img":{"jxl_distance":0.8},"photos":{"album_name":"private-album"}}"#, sources: "{}")]
        entries = [entry]
        applyFilter()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        preferences.set(true, forKey: DisplayPrivacy.preferenceKey)
        displayPrivacyChanged()
        updateLocalization()
        let column = table.tableColumns.first { $0.identifier.rawValue == "source" }!
        let cell = tableView(table, viewFor: column, row: 0) as? NSTableCellView
        let privacyChecks: [(String, Bool)] = [
            ("source label", cell?.textField?.stringValue == localized("privacy.display.hidden")),
            ("source tooltip", cell?.textField?.toolTip == localized("privacy.display.hidden")),
            ("normal path hidden", !detail.string.contains(normalPath)),
            ("album hidden", !detail.string.contains("private-album")),
            ("problem preserved", detail.string.contains(problemPath)),
            ("numeric setting preserved", detail.string.contains("jxl_distance: 0.8")),
            ("visible copy", visibleCopyText == detail.string),
            ("visible copy label", copyButton.title == localized("privacy.display.copy_visible")),
            ("export disabled", !exportButton.isEnabled), ("reveal disabled", !revealButton.isEnabled),
            ("source evidence unchanged", entry.context?.inputs == [normalPath]),
            ("arguments unchanged", entry.invocations.first?.arguments == ["--input", normalPath]),
            ("comparison path hidden", !Self.renderComparison([entry, entry], privacy: true).contains(normalPath))]
        guard privacyChecks.allSatisfy({ $0.1 }) else {
            throw HostError(message: "History privacy invariant failed: " + privacyChecks.filter { !$0.1 }.map(\.0).joined(separator: ", ")
                + "; selected=\(selectedEntries.count), tab=\(detailMode.selectedSegment), detail=\(detail.string)")
        }
        outputPage = HistoryStore.Output(rows: [
            .init(sequence: 1, text: "[SUCCESS] " + normalPath, diagnostic: false, pipeline: "IMG", line: "[SUCCESS] " + normalPath),
            .init(sequence: 2, text: "[FAIL] " + problemPath + ": decoder failed", diagnostic: true,
                pipeline: "IMG", line: "[FAIL] " + problemPath + ": decoder failed")],
            total: 2, upper: 2, firstOrdinal: 1, issues: [])
        detailMode.selectedSegment = 1
        renderOutput()
        guard !detail.string.contains(normalPath), detail.string.contains(problemPath), visibleCopyText == detail.string else {
            throw HostError(message: "History output privacy hid a failure or copied an ordinary path")
        }
        let failure = HistoryFileFailure(schema_version: 1, pipeline: "img", source_path: problemPath, absolute_path: problemPath,
            message: "decoder failed", run_id: "run-privacy", invocation_id: "invocation-privacy", reason_code: nil)
        let failureRow = FailureDisplayRow(path: problemPath, absolutePath: problemPath, reason: "decoder failed",
            source: FailureSourceKey(failure), observations: [FailureObservation(failure: failure, repetitions: 1)])
        let privateFailureDetails = try failureDetailText(for: failureRow)
        guard privateFailureDetails.contains(problemPath), !privateFailureDetails.contains(normalPath),
              !privateFailureDetails.contains("/synthetic/bin/img"),
              privateFailureDetails.contains(localized("history.file_failure.worker.private")) else {
            throw HostError(message: "Failure detail privacy exposed the worker's routine inputs or hid the failed source")
        }
        failureDisplayRows = [FailureDisplayRow(path: problemPath, absolutePath: nil, reason: "decoder failed", source: nil, observations: [])]
        failedContent.isHidden = false
        guard visibleCopyText.contains(problemPath), !visibleCopyText.contains(normalPath) else {
            throw HostError(message: "Visible failure-list copy read a hidden summary")
        }
        preferences.set(false, forKey: DisplayPrivacy.preferenceKey)
        displayPrivacyChanged()
        let restored = tableView(table, viewFor: column, row: 0) as? NSTableCellView
        guard restored?.textField?.toolTip == normalPath, Self.render(entry).contains(normalPath),
              copyButton.title == localized("history.copy_sources"),
              try failureDetailText(for: failureRow).contains(normalPath) else {
            throw HostError(message: "History privacy changed or erased the retained original evidence")
        }
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
        guard !emptyState.isHidden, emptyTitle.stringValue == localized("history.empty"),
              !table.usesAlternatingRowBackgroundColors else {
            throw HostError(message: "History empty state is missing or renders empty striped rows")
        }
        loadIssues = ["Synthetic read failure"]
        applyFilter()
        guard emptyTitle.stringValue == localized("history.read_issues"),
              emptyHint.stringValue == localized("history.empty.read_hint") else {
            throw HostError(message: "History read failure was presented as an empty successful read")
        }
        loadIssues = []
        applyFilter()
        guard table.tableColumns.count == 5, search.delegate != nil, detail.isEditable == false,
              split.arrangedSubviews.count == 2, copyButton.isEnabled == false,
              logButton.isEnabled == false, revealButton.isEnabled == false, !cleanupButton.isEnabled, table.numberOfRows == 0,
              detailMode.segmentCount == 5, !detailMode.isEnabled(forSegment: 2), !detailMode.isEnabled(forSegment: 3),
              !detailMode.isEnabled(forSegment: 4), ignoredView.isHidden,
               failedContent.isHidden, !failedCopyButton.isEnabled, !failedRevealButton.isEnabled,
               table.allowsMultipleSelection, table.allowsEmptySelection,
               window?.styleMask.contains(.resizable) == true else {
            throw HostError(message: "History native controls or selection state regressed")
        }
        let synthetic = HistoryEntry(id: "synthetic:0", stamp: "synthetic", audit: URL(fileURLWithPath: "/synthetic/audit"),
                                     timestamp: "synthetic", legacy: true)
        entries = [synthetic]
        applyFilter()
        guard emptyState.isHidden else { throw HostError(message: "History empty state covered recorded batches") }
        search.stringValue = "synthetic-no-match"
        applyFilter()
        guard !emptyState.isHidden, emptyTitle.stringValue == localized("history.no_match") else {
            throw HostError(message: "History search with no matches lost its recovery hint")
        }
        search.stringValue = ""
        applyFilter()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        showSelected()
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
        guard selected == nil, selectedEntries.count == 2, !logButton.isEnabled, !revealButton.isEnabled, !cleanupButton.isEnabled,
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
        for button in [logButton, cleanupButton, exportButton, revealButton, copyButton, removeButton] {
            let rect = button.convert(button.bounds, to: content)
            guard button.imagePosition == .imageLeading, !button.title.isEmpty,
                  button.frame.width + 1 >= button.intrinsicContentSize.width,
                  rect.minX >= 0, rect.maxX <= content.bounds.maxX + 1,
                  rect.minY >= 0, rect.maxY <= content.bounds.maxY + 1 else {
                throw HostError(message: "History action label clipped at minimum size: \(button.title) \(rect)")
            }
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
        for control in [detailMode, failedSearch, failedDetailsButton, failedCopyButton, failedRevealButton, failedStatus, failedContent] as [NSView] {
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
        batchIssueResult = nil
        detailMode.selectedSegment = 2
        detailModeChanged()
        let reasonDeadline = Date().addingTimeInterval(5)
        while issueScanControl != nil, Date() < reasonDeadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        guard batchIssueResult != nil, status.stringValue == localized("history.shown", displayed.count, entries.count),
              !failedStatus.stringValue.contains(localized("history.file_failure.loading")) else {
            throw HostError(message: "Completed file-reason read retained a loading status")
        }
        detailMode.selectedSegment = 3
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

private func runHistoryRemovalSelfTests() throws {
    let directory = URL(fileURLWithPath: "/private/tmp/synthetic-history-removal")
    let database = directory.appendingPathComponent("history.sqlite3")
    func entry(_ run: String, index: Int) -> HistoryEntry {
        var value = HistoryEntry(id: "display-only:\(index)", stamp: "synthetic-session", audit: database, timestamp: "2026-10-25")
        value.context = HistoryContext(schema_version: 1, inputs: ["/synthetic/private-normal.jpeg"], output: nil,
            backup: nil, mode: "standard", dry_run: false, in_place: false, shortest_path: false,
            version: nil, retry: nil, resume: nil, no_resume: nil, output_sequence: 0, run_id: run)
        value.finished = HistoryFinished(schema_version: 1, outcome: "completed", error: nil, output_sequence: 4)
        return value
    }
    func require(_ value: Bool, _ message: String) throws { if !value { throw HostError(message: message) } }
    func reject(_ operation: () throws -> Void) throws {
        do { try operation() } catch { return }
        throw HostError(message: "History removal accepted unavailable or contradictory evidence")
    }
    let first = entry("run-one", index: 3)
    let second = entry("run-two", index: 7)
    let request = try HistoryRemovalRequest(entries: [first, second], directory: directory)
    try require(request.runs.map(\.run_id) == ["run-one", "run-two"] && request.runs.allSatisfy { $0.session_id == "synthetic-session" },
                "History removal used presentation row identity")
    let single = try HistoryRemovalRequest(entries: [first], directory: directory)
    try require(single.runs.count == 1, "History removal lost single selection")
    for fixtureRequest in [single, request] {
        let fixturePreview = try HistoryRemovalPreview.decode(historyRemovalFixtureOutput(fixtureRequest, directory: directory),
            request: fixtureRequest, directory: directory)
        var fixtureExecution = fixtureRequest
        fixtureExecution.snapshot_token = fixturePreview.snapshot_token
        let fixtureReceipt = try HistoryRemovalReceipt.decode(historyRemovalFixtureOutput(fixtureExecution, directory: directory),
            preview: fixturePreview)
        try require(fixtureReceipt.status == "completed" && fixtureReceipt.removed_runs == UInt64(fixtureRequest.runs.count),
            "History removal visual fixture did not encode a valid single/multiple-selection round trip")
    }
    try reject { _ = try HistoryRemovalRequest(entries: [first, first], directory: directory) }
    var legacy = first
    legacy.legacy = true
    try reject { _ = try HistoryRemovalRequest(entries: [legacy], directory: directory) }
    var unfinished = first
    unfinished.finished = nil
    try reject { _ = try HistoryRemovalRequest(entries: [unfinished], directory: directory) }
    let token = String(repeating: "a", count: 64)
    let artifact: [String: Any] = ["path": directory.appendingPathComponent("private-normal.log").path,
        "scope": "registered_file", "status": "delete", "reason": "registered session file"]
    let retained: [String: Any] = ["path": directory.path, "scope": "unregistered_external_logs", "status": "retained", "reason": "unowned logs remain"]
    let value: [String: Any] = ["schema_version": 1, "kind": "history_cleanup_preview", "source": database.path,
        "snapshot_token": token, "runs": [["session_id": "synthetic-session", "run_id": "run-one"],
            ["session_id": "synthetic-session", "run_id": "run-two"]], "event_rows": 4, "output_rows": 8,
        "event_bytes": 200, "output_bytes": 400, "artifacts": [artifact, retained], "can_execute": true,
        "external_log_scope": "registered_session_logs_only"]
    func preview(_ fields: [String: Any]) throws -> HistoryRemovalPreview {
        try HistoryRemovalPreview.decode(JSONSerialization.data(withJSONObject: fields), request: request, directory: directory)
    }
    let approved = try preview(value)
    try require(!approved.detail(privacy: true).contains("private-normal.log")
        && !approved.detail(privacy: true).contains(directory.path)
        && approved.detail(privacy: false).contains("private-normal.log"), "History removal preview leaked a normal or expected-retained log path")
    var problemPreview = value
    problemPreview["artifacts"] = [["path": directory.appendingPathComponent("owned-problem.log").path,
        "scope": "registered_file_problem", "status": "retained", "reason": "registered identity could not be verified"], retained]
    try require(try preview(problemPreview).detail(privacy: true).contains("owned-problem.log"),
        "History removal privacy hid an actual retained-log problem")
    var invalid = value
    invalid["source"] = "/different/history.sqlite3"
    try reject { _ = try preview(invalid) }
    invalid = value; invalid["snapshot_token"] = "stale"
    try reject { _ = try preview(invalid) }
    invalid = value; invalid["runs"] = [["session_id": "synthetic-session", "run_id": "other-run"]]
    try reject { _ = try preview(invalid) }
    var execution = request
    execution.snapshot_token = approved.snapshot_token
    let arguments = try execution.arguments(directory: directory)
    guard let encoded = arguments.firstIndex(of: "--history-cleanup").map({ arguments[$0 + 1] }),
          let body = try JSONSerialization.jsonObject(with: Data(encoded.utf8)) as? [String: Any] else {
        throw HostError(message: "History removal did not construct an explicit backend request")
    }
    try require(body["snapshot_token"] as? String == token && arguments.contains("--yes")
        && !encoded.contains("display-only"), "History removal lost the approved snapshot or run identities")
    let receipt: [String: Any] = ["schema_version": 1, "kind": "history_cleanup_receipt", "status": "completed",
        "database_committed": true, "removed_runs": 2, "event_rows": 4, "output_rows": 8,
        "removed_artifacts": 1, "retained_artifacts": [retained], "failed_artifacts": []]
    let completedReceipt = try HistoryRemovalReceipt.decode(JSONSerialization.data(withJSONObject: receipt), preview: approved)
    try require(!completedReceipt.detail(privacy: true).contains(directory.path)
        && completedReceipt.detail(privacy: false).contains(directory.path),
        "History removal receipt leaked an expected-retained normal log path")
    var badReceipt = receipt; badReceipt["removed_runs"] = 1
    try reject { _ = try HistoryRemovalReceipt.decode(JSONSerialization.data(withJSONObject: badReceipt), preview: approved) }
    badReceipt = receipt; badReceipt["removed_artifacts"] = 0
    try reject { _ = try HistoryRemovalReceipt.decode(JSONSerialization.data(withJSONObject: badReceipt), preview: approved) }
    var partial = receipt
    partial["status"] = "partial"; partial["removed_artifacts"] = 0
    partial["failed_artifacts"] = [["path": directory.appendingPathComponent("recovery.log").path,
        "scope": "registered_file", "status": "failed", "reason": "owned file retained at recovery path"]]
    let partialReceipt = try HistoryRemovalReceipt.decode(JSONSerialization.data(withJSONObject: partial), preview: approved)
    try require(partialReceipt.detail(privacy: true).contains("recovery.log"), "History removal hid an actual cleanup problem")
}

func runProcessingHistorySelfTests() throws {
    try runHistoryRemovalSelfTests()
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
    let deferredRun = "run-delivery-deferred"
    let deferredContext = try payload("MFB_HISTORY_CONTEXT", ["schema_version": 1, "run_id": deferredRun,
        "inputs": ["/synthetic/input"], "output": "/synthetic/output", "backup": NSNull(), "mode": "fast-img",
        "dry_run": false, "in_place": false, "shortest_path": true])
    let deferredReceipt: [String: Any] = ["schema_version": 1, "run_id": deferredRun,
        "source_directory": "/synthetic/input", "output_directory": "/synthetic/output",
        "succeeded": 0, "skipped": 0, "ignored": 0, "failed": 1, "unprocessed": 3, "encoded": 3,
        "photos_verified": 0, "source_retained": 4, "failed_sources_retained": 1, "failed_sources_verified": 1,
        "failed_source_verification_error": NSNull(), "retention_unknown_reason": NSNull(),
        "primary_cleanup_complete": false, "primary_verification_complete": true, "photos_delivery_deferred": true,
        "finalization_error": "synthetic Photos target unavailable"]
    var deferredSummary = summaryValue
    deferredSummary["run_id"] = deferredRun
    deferredSummary["img"] = ["active": true, "succeeded": 0, "skipped": 0, "ignored": 0,
        "failed": 1, "unprocessed": 3, "exit_code": 2]
    let deferredFinal = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "run_id": deferredRun,
        "outcome": "failed", "error": "fast-img exited with code 2"])
    let deliveryEvent = try payload("MFB_HISTORY_FAST_IMG_RESULT", deferredReceipt)
    let deliverySummary = try payload("MFB_HISTORY_SUMMARY", deferredSummary)
    let deferredEntry = try parsed([deferredContext, deliveryEvent, deliverySummary, deferredFinal]).entries[0]
    try require(deferredEntry.deliveryIsDeferred && deferredEntry.statusKey == "result.delivery_deferred"
        && deferredEntry.statusColor == .systemOrange && deferredEntry.succeededLabel == "0"
        && ProcessingHistoryPanel.render(deferredEntry).contains("/synthetic/output")
        && ProcessingHistoryPanel.render(deferredEntry).contains("synthetic Photos target unavailable"),
        "History lost deferred delivery evidence or claimed completed delivery")
    for (key, value) in [("run_id", "another-run" as Any), ("source_directory", "/synthetic/other" as Any),
                         ("output_directory", "/synthetic/other" as Any), ("failed_sources_verified", 0 as Any),
                         ("primary_verification_complete", false as Any), ("unprocessed", 2 as Any)] {
        var changed = deferredReceipt
        changed[key] = value
        let entry = try parsed([deferredContext, try payload("MFB_HISTORY_FAST_IMG_RESULT", changed),
            deliverySummary, deferredFinal]).entries[0]
        try require(!entry.deliveryIsDeferred, "History accepted contradictory deferred evidence: \(key)")
    }
    let duplicateDelivery = try parsed([deferredContext, deliveryEvent, deliveryEvent, deliverySummary, deferredFinal]).entries[0]
    try require(duplicateDelivery.fastDeliveryInvalid && !duplicateDelivery.deliveryIsDeferred,
                "Duplicate delivery observations inflated a deferred result")
    var cancelledDelivery: [String: Any] = ["schema_version": 1, "run_id": deferredRun,
        "outcome": "cancelled", "error": NSNull()]
    let cancelledEntry = try parsed([deferredContext, deliveryEvent, deliverySummary,
        try payload("MFB_HISTORY_FINISHED", cancelledDelivery)]).entries[0]
    try require(cancelledEntry.statusKey == "history.status.cancelled", "Deferred delivery concealed cancellation")
    cancelledDelivery["outcome"] = "failed"
    cancelledDelivery["error"] = "synthetic crash"
    let crashedDelivery = try parsed([deferredContext, deliveryEvent, deliverySummary,
        try payload("MFB_HISTORY_FINISHED", cancelledDelivery)]).entries[0]
    try require(!crashedDelivery.deliveryIsDeferred, "Deferred delivery concealed an unexpected terminal failure")
    let deferredPlan = try payload("MFB_HISTORY_INPUT_PLAN", ["schema_version": 1, "run_id": deferredRun,
        "inputs": [["source": "/synthetic/input", "output": "/synthetic/output",
            "in_place": false, "requires_verification": true]]])
    let plannedDelivery = try parsed([deferredContext, deferredPlan, deliveryEvent, deliverySummary, deferredFinal]).entries[0]
    try require(!plannedDelivery.deliveryIsDeferred && plannedDelivery.verification == nil
        && plannedDelivery.contradictoryFields.contains("input_plan"),
        "An unsupported single-input plan bypassed History input validation")
    var unsafeSummary = deferredSummary
    unsafeSummary["integrity"] = ["state": "WARNINGS", "issue_count": 1]
    let unsafeDelivery = try parsed([deferredContext, deliveryEvent, try payload("MFB_HISTORY_SUMMARY", unsafeSummary), deferredFinal]).entries[0]
    try require(unsafeDelivery.statusKey == "result.unconfirmed", "Deferred delivery hid an independent integrity issue")
    try require(ProcessingHistoryPanel.render(unsafeDelivery).contains(localized("result.delivery_record_title"))
        && !ProcessingHistoryPanel.render(unsafeDelivery).contains(localized("result.delivery_deferred")),
        "A historical delivery receipt overrode the unconfirmed primary verdict")
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
    let spawnValue: [String: Any] = ["schema_version": 1, "scope": "os_child_process", "invocation_id": "synthetic-call-1",
        "pipeline": "img", "state": "spawned", "pid": 1234, "exit_code": NSNull(), "signal": NSNull()]
    var exitValue = spawnValue
    exitValue["state"] = "exited"
    exitValue["exit_code"] = 7
    let spawn = try payload("MFB_HISTORY_WORKER_EXECUTION", spawnValue)
    let exited = try payload("MFB_HISTORY_WORKER_EXECUTION", exitValue)
    let executed = try parsed([context, launch, spawn, boundImage, exited, summary, failed]).entries[0]
    try require(executed.workerExecutions["synthetic-call-1"]?.count == 2 && executed.succeededLabel == "2"
                && ProcessingHistoryPanel.render(executed).contains(localized("history.worker.execution.exited", "7")),
                "Actual child observations were lost or replaced media counts")
    let unfinishedChild = try parsed([context, launch, spawn, summary, failed]).entries[0]
    try require(ProcessingHistoryPanel.render(unfinishedChild).contains(localized("history.worker.execution.spawned")),
                "Missing exit observation became a fabricated success or current liveness")
    var wrongPID = exitValue
    wrongPID["pid"] = 1235
    var signalValue = exitValue
    signalValue["exit_code"] = NSNull()
    signalValue["signal"] = 15
    let signalled = try parsed([context, launch, spawn, try payload("MFB_HISTORY_WORKER_EXECUTION", signalValue), summary, failed]).entries[0]
    try require(ProcessingHistoryPanel.render(signalled).contains(localized("history.worker.execution.signal", "15")),
                "Signal termination was interpreted as a normal exit")
    for observations in [[exited], [spawn, spawn], [spawn, exited, exited],
                         [spawn, try payload("MFB_HISTORY_WORKER_EXECUTION", wrongPID)]] {
        let invalid = try parsed([context, launch] + observations + [summary, failed]).entries[0]
        try require(invalid.workerExecutionEvidenceInvalid && invalid.workerExecutions.isEmpty && invalid.succeededLabel == "2",
                    "Conflicting process observations became authoritative or hid file outcomes")
    }
    try require(launchBatches.entries[0].invocations.map(\.invocation_id) == ["synthetic-call-1"]
                && launchBatches.entries[1].invocations.map(\.invocation_id) == ["synthetic-call-2"],
                "Worker launch evidence crossed selected-run boundaries")
    try require(success.entries[0].invocations.isEmpty
                && ProcessingHistoryPanel.render(success.entries[0]).contains(localized("history.worker.not_recorded")),
                "Old history fabricated a worker launch receipt")
    let storageValue: [String: Any] = ["schema_version": 1, "scope": "captured_worker_output", "run_id": NSNull(),
        "invocation_id": "synthetic-call-1", "pipeline": "img", "state": "committed", "observed_records": 3,
        "committed_records": 3, "committed_utf8_bytes": 27, "capture_complete": true, "diagnostic": NSNull()]
    let storage = try payload("MFB_HISTORY_OUTPUT_STORAGE", storageValue)
    let stored = try parsed([context, launch, storage, storage, summary, completed]).entries[0]
    try require(stored.outputStorage.count == 1 && stored.invalidOutputStorageInvocations.isEmpty
                && stored.outputStorageText(for: "synthetic-call-1").contains(localized("history.output_storage.committed"))
                && stored.outputStorageText(for: "synthetic-call-1").contains(localized("history.output_storage.counts", "3", "3", "27"))
                && stored.statusKey == "history.status.completed" && stored.succeededLabel == "2" && !stored.hasVerificationFailure,
                "Output storage duplicated observations, changed media outcomes, or lost exact record/byte counts")
    try require(launched.outputStorage.isEmpty && launched.outputStorageText(for: "synthetic-call-1") == localized("history.output_storage.not_recorded")
                && ProcessingHistoryPanel.render(success.entries[0]).contains(localized("history.output_storage.not_recorded")),
                "Old history invented a complete output storage receipt")
    var failedStorageValue = storageValue
    failedStorageValue["invocation_id"] = "synthetic-call-2"
    failedStorageValue["pipeline"] = "vid"
    failedStorageValue["state"] = "failed"
    failedStorageValue["committed_records"] = 2
    failedStorageValue["capture_complete"] = false
    failedStorageValue["diagnostic"] = "synthetic SQLite write failure"
    let failedStorage = try payload("MFB_HISTORY_OUTPUT_STORAGE", failedStorageValue)
    let mixedStorage = try parsed([context, launch, secondLaunch, storage, failedStorage, summary, completed]).entries[0]
    try require(mixedStorage.outputStorage.count == 2 && mixedStorage.issues.isEmpty
                && mixedStorage.statusKey == "history.status.completed" && mixedStorage.succeededLabel == "2"
                && mixedStorage.needsAttention && mixedStorage.statusLabel.contains(localized("history.output_storage.attention"))
                && mixedStorage.searchable.contains("synthetic SQLite write failure")
                && mixedStorage.outputStorageText(for: "synthetic-call-2").contains(localized("history.output_storage.failed"))
                && mixedStorage.outputStorageText(for: "synthetic-call-2").contains(localized("history.output_storage.capture_incomplete"))
                && ProcessingHistoryPanel.render(mixedStorage).contains("synthetic SQLite write failure"),
                "Per-invocation storage failure hid another receipt, its original diagnostic, or changed media verification")
    for observed in [NSNull() as Any, 2 as Any] {
        var recoveredStorageValue = failedStorageValue
        recoveredStorageValue["observed_records"] = observed
        let entry = try parsed([context, secondLaunch, try payload("MFB_HISTORY_OUTPUT_STORAGE", recoveredStorageValue), summary, completed]).entries[0]
        try require(entry.outputStorage["synthetic-call-2"]?.state == "failed" && entry.invalidOutputStorageInvocations.isEmpty,
                    "Counter overflow or later successful writes erased an actual storage failure")
    }
    for diagnostic in ["", String(repeating: "界", count: 16 * 1024 / 3 + 1), NSNull()] as [Any] {
        var invalidDiagnostic = failedStorageValue
        invalidDiagnostic["diagnostic"] = diagnostic
        let entry = try parsed([context, secondLaunch, try payload("MFB_HISTORY_OUTPUT_STORAGE", invalidDiagnostic), summary, completed]).entries[0]
        try require(entry.invalidOutputStorageInvocations.contains("synthetic-call-2"), "Failed storage accepted missing, empty, or oversized diagnostic")
    }
    var incompleteStorageValue = storageValue
    incompleteStorageValue["capture_complete"] = false
    let incompleteStorage = try parsed([context, launch, try payload("MFB_HISTORY_OUTPUT_STORAGE", incompleteStorageValue), summary, completed]).entries[0]
    try require(incompleteStorage.outputStorageText(for: "synthetic-call-1").contains(localized("history.output_storage.committed"))
                && incompleteStorage.needsAttention
                && incompleteStorage.outputStorageText(for: "synthetic-call-1").contains(localized("history.output_storage.capture_incomplete")),
                "Saving received output was confused with collecting every worker output")
    for (key, value) in [("schema_version", 2 as Any), ("scope", "all_runtime_logs"), ("run_id", "other-run"),
                          ("pipeline", "vid"), ("state", "unknown"), ("observed_records", -1), ("observed_records", NSNull()),
                          ("committed_records", 4), ("committed_records", true), ("committed_utf8_bytes", -1),
                          ("capture_complete", 1), ("diagnostic", "unexpected error"), ("unknown_field", true)] {
        var invalidStorage = storageValue
        invalidStorage[key] = value
        let entry = try parsed([context, launch, try payload("MFB_HISTORY_OUTPUT_STORAGE", invalidStorage), storage, summary, completed]).entries[0]
        try require(entry.invalidOutputStorageInvocations.contains("synthetic-call-1")
                    && entry.outputStorageText(for: "synthetic-call-1") == localized("history.output_storage.invalid")
                    && entry.issues.isEmpty && entry.statusKey == "history.status.completed" && entry.succeededLabel == "2",
                    "Invalid storage evidence recovered silently or changed independent media counts")
    }
    for key in ["run_id", "observed_records", "diagnostic"] {
        var missingField = storageValue
        missingField.removeValue(forKey: key)
        let entry = try parsed([context, launch, try payload("MFB_HISTORY_OUTPUT_STORAGE", missingField), summary, completed]).entries[0]
        try require(entry.invalidOutputStorageInvocations.contains("synthetic-call-1"), "Missing nullable receipt field was accepted as explicit evidence")
    }
    var conflictValue = storageValue
    conflictValue["committed_utf8_bytes"] = 28
    let conflictStorage = try parsed([context, launch, secondLaunch, storage,
        try payload("MFB_HISTORY_OUTPUT_STORAGE", conflictValue), storage, failedStorage, summary, completed]).entries[0]
    try require(conflictStorage.invalidOutputStorageInvocations == ["synthetic-call-1"]
                && conflictStorage.outputStorageText(for: "synthetic-call-1") == localized("history.output_storage.invalid")
                && conflictStorage.outputStorageText(for: "synthetic-call-2").contains("synthetic SQLite write failure"),
                "Storage conflict was erased by a later receipt or invalidated an independent invocation")
    let unboundStorage = try parsed([context, storage, launch, summary, completed]).entries[0]
    try require(unboundStorage.unboundOutputStorageInvalid && unboundStorage.outputStorage.isEmpty
                && unboundStorage.statusKey == "history.status.completed"
                && ProcessingHistoryPanel.render(unboundStorage).contains(localized("history.output_storage.unbound")),
                "Unbound storage receipt was attached to a later invocation or changed media status")
    let storageBatches = try parsed([context, launch, storage, summary, completed, context, launch, summary, completed])
    try require(storageBatches.entries[0].outputStorage.count == 1 && storageBatches.entries[1].outputStorage.isEmpty,
                "Output storage receipts crossed run boundaries")
    func withStorageRun(_ event: String) throws -> String {
        let separator = event.firstIndex(of: "=")!
        var object = try JSONSerialization.jsonObject(with: Data(event[event.index(after: separator)...].utf8)) as! [String: Any]
        object["run_id"] = "synthetic-storage-run"
        return try payload(String(event[..<separator]), object)
    }
    let identifiedStorage = try parsed([context, launch, storage, summary, completed].map { try withStorageRun($0) }).entries[0]
    try require(identifiedStorage.outputStorage["synthetic-call-1"]?.run_id == "synthetic-storage-run"
                && identifiedStorage.invalidOutputStorageInvocations.isEmpty && identifiedStorage.issues.isEmpty,
                "An exact run/invocation/pipeline storage binding was rejected")
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
    let fastContext = try payload("MFB_HISTORY_CONTEXT", ["schema_version": 1, "inputs": ["/synthetic/input"],
        "output": "/synthetic/output", "mode": "fast-img", "dry_run": false, "in_place": false, "shortest_path": false])
    let fileFailureFinish = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "failed", "error": "fast-img exited with code 1"])
    let preservedReceipt: [String: Any] = ["schema_version": 1, "has_warnings": false, "issue_count": 0,
        "source_count": 4, "optimized_count": 2, "skipped_count": 1, "failed_count": 1,
        "source_remaining_count": 1, "verified_deleted_count": 2, "count_status": "MATCH",
        "source_path": "/synthetic/input", "output_path": "/synthetic/output",
        "failed_sources_verified": 1, "primary_cleanup_complete": true]
    let preservedEvent = try payload("MFB_HISTORY_VERIFICATION", preservedReceipt)
    do {
        func bound(_ name: String, _ fields: [String: Any]) throws -> String {
            var fields = fields
            fields["run_id"] = "synthetic-cleanup-run"
            return try payload(name, fields)
        }
        let cleanupContext = try bound("MFB_HISTORY_CONTEXT", ["schema_version": 1, "inputs": ["/synthetic/input"],
            "output": "/synthetic/output", "mode": "fast-img", "dry_run": false, "in_place": false, "shortest_path": true])
        let cleanup: [String: Any] = ["schema_version": 1, "output_path": "/synthetic/output", "outcome": "residual",
            "removed_files": 2, "already_absent_files": 1, "pruned_directories": 0, "error": NSNull()]
        let event = try bound("MFB_HISTORY_OUTPUT_CLEANUP", cleanup)
        let end = try bound("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "completed", "error": NSNull()])
        let events = [cleanupContext, event, event, try bound("MFB_HISTORY_VERIFICATION", preservedReceipt),
            try bound("MFB_HISTORY_SUMMARY", mixedSummary), end]
        let entry = try parsed(events).entries[0]
        try MainActor.assumeIsolated { try ProcessingHistoryPanel(directory: fixtureURL.deletingLastPathComponent()).validateCleanupForSelfTest(entry) }
        try require(entry.outputCleanups.count == 1 && !entry.outputCleanupInvalid && entry.summary?.img.succeeded == 2
                    && entry.outputCleanups[0].removed_files == 2
                    && ProcessingHistoryPanel.render(entry).contains(localized("cleanup.outcome.residual")),
                    "Output cleanup observations inflated counts or lost their separate historical status")
        let noFailures = try parsed([cleanupContext, event, try bound("MFB_HISTORY_SUMMARY", summaryValue), end]).entries[0]
        try require(noFailures.statusKey == "history.status.completed" && noFailures.needsAttention
                    && noFailures.succeededLabel == "2" && noFailures.statusLabel.contains(localized("cleanup.attention.short"))
                    && noFailures.searchable.contains(localized("cleanup.outcome.residual")),
                    "Cleanup attention disappeared from the filter, search or label, or changed media completion")
        let old = try parsed([fastContext, mixedEvent, preservedEvent, fileFailureFinish]).entries[0]
        try require(ProcessingHistoryPanel.render(old).contains(localized("cleanup.not_recorded")),
                    "Primary source cleanup was used to infer output folder removal")
        var wrongOutput = preservedReceipt
        wrongOutput["output_path"] = "/synthetic/other"
        var wrongCleanup = cleanup
        wrongCleanup["output_path"] = "/synthetic/other"
        let inconsistentPaths = try parsed([cleanupContext, try bound("MFB_HISTORY_VERIFICATION", wrongOutput),
            try bound("MFB_HISTORY_OUTPUT_CLEANUP", wrongCleanup), end]).entries[0]
        try require(inconsistentPaths.outputCleanupInvalid && inconsistentPaths.outputCleanups.isEmpty,
                    "Verification and cleanup paths overrode a conflicting recorded output target")
        for (key, value) in [("schema_version", 2 as Any), ("outcome", "guessed"), ("removed_files", -1),
                              ("removed_files", true), ("output_path", "/synthetic/other"), ("run_id", "different-run")] {
            var changed = cleanup
            changed["run_id"] = "synthetic-cleanup-run"
            changed[key] = value
            let bad = try payload("MFB_HISTORY_OUTPUT_CLEANUP", changed)
            let invalid = try parsed([cleanupContext, event, bad, event, end]).entries[0]
            try require(invalid.outputCleanupInvalid && invalid.outputCleanups.isEmpty,
                        "Invalid, conflicting or unbound output cleanup evidence was accepted: \(key)")
        }
        for outcome in ["not_requested", "verification_blocked", "already_absent"] {
            var unmeasured = cleanup
            unmeasured["outcome"] = outcome
            for key in ["removed_files", "already_absent_files", "pruned_directories"] { unmeasured[key] = NSNull() }
            let noAttempt = try bound("MFB_HISTORY_OUTPUT_CLEANUP", unmeasured)
            let operationContext = outcome == "not_requested"
                ? try bound("MFB_HISTORY_CONTEXT", ["schema_version": 1, "inputs": ["/synthetic/input"],
                    "output": "/synthetic/output", "mode": "fast-img", "dry_run": false, "in_place": false, "shortest_path": false])
                : cleanupContext
            let recorded = try parsed([operationContext, noAttempt, end]).entries[0]
            try require(recorded.outputCleanups.first?.removed_files == nil
                        && recorded.outputCleanups.first?.detailText.contains(localized("cleanup.not_measured")) == true,
                        "An unmeasured cleanup invented zero counts")
            let contradictory = try parsed([outcome == "not_requested" ? cleanupContext
                : try bound("MFB_HISTORY_CONTEXT", ["schema_version": 1, "inputs": ["/synthetic/input"],
                    "output": "/synthetic/output", "mode": "fast-img", "dry_run": false, "in_place": false, "shortest_path": false]),
                noAttempt, end]).entries[0]
            try require(contradictory.outputCleanupInvalid && contradictory.outputCleanups.isEmpty,
                        "Cleanup outcome contradicted whether folder removal was requested")
        }
        var failedCleanup = cleanup
        failedCleanup["outcome"] = "failed"
        failedCleanup["pruned_directories"] = NSNull()
        failedCleanup["error"] = "synthetic permission denied after two removals"
        let partial = try parsed([cleanupContext, try bound("MFB_HISTORY_OUTPUT_CLEANUP", failedCleanup), end]).entries[0]
        try require(partial.outputCleanups.first?.removed_files == 2
                    && partial.outputCleanups.first?.pruned_directories == nil
                    && partial.outputCleanups.first?.error == failedCleanup["error"] as? String,
                    "Partial cleanup erased completed work or invented an unmeasured directory count")
    }
    for finish in [completed, failed] {
        let mixed = try parsed([context, mixedEvent, finish]).entries[0]
        try require(!mixed.hasPartialSuccess && mixed.needsAttention
                    && mixed.summary?.img.exit_code == 1 && mixed.summary?.failed_files == ["/synthetic/failed.jpg"]
                    && mixed.statusColor == .systemRed
                    && mixed.succeededLabel == "2" && mixed.finished?.outcome == (finish == completed ? "completed" : "failed"),
                    "Counts without preservation proof became a confirmed partial result")
    }
    for finish in [completed, fileFailureFinish] {
        let mixed = try parsed([fastContext, mixedEvent, preservedEvent, finish]).entries[0]
        try require(mixed.hasPartialSuccess && mixed.statusKey == "history.status.partial"
                    && mixed.statusColor == .systemOrange && mixed.succeededLabel == "2",
                    "Verified file failures lost their safe partial result")
    }
    for (key, value) in [("failed_sources_verified", NSNull() as Any), ("failed_sources_verified", 0),
                          ("primary_cleanup_complete", false), ("source_path", "/synthetic/other"),
                          ("output_path", "/synthetic/other"), ("count_status", "UNKNOWN")] {
        var incomplete = preservedReceipt
        incomplete[key] = value
        let entry = try parsed([fastContext, mixedEvent, try payload("MFB_HISTORY_VERIFICATION", incomplete), fileFailureFinish]).entries[0]
        try require(!entry.hasPartialSuccess && entry.succeededLabel == "2",
                    "Missing or unbound preservation evidence became a safe partial result")
    }
    let infrastructureFailure = try parsed([fastContext, mixedEvent, preservedEvent, failed]).entries[0]
    try require(!infrastructureFailure.hasPartialSuccess && infrastructureFailure.statusColor == .systemRed,
                "A later session failure was hidden behind earlier file preservation proof")
    var integritySummary = mixedSummary
    integritySummary["integrity"] = ["state": "WARNINGS", "issue_count": 1]
    let integrityFailure = try parsed([context, try payload("MFB_HISTORY_SUMMARY", integritySummary), failed]).entries[0]
    try require(!integrityFailure.hasPartialSuccess && integrityFailure.statusKey == "result.unconfirmed"
                && integrityFailure.statusColor == .systemRed && integrityFailure.succeededLabel == "2",
                "Preserved successful items hid a known integrity issue in a partial batch")
    for (issueCount, countStatus) in [(0, "MATCH"), (1, "MATCH"), (0, "MISMATCH (1 invariant issue)")] {
        let receipt = HistoryVerification(schema_version: 1, has_warnings: true, issue_count: issueCount,
            source_count: 4, optimized_count: 2, skipped_count: 1, failed_count: 1,
            source_remaining_count: 1, verified_deleted_count: 2, count_status: countStatus,
            source_path: "/synthetic/input", output_path: "/synthetic/output",
            failed_sources_verified: 1, primary_cleanup_complete: true)
        let serious = issueCount > 0 || countStatus.hasPrefix("MISMATCH")
        for perInput in [false, true] {
            var entry = try parsed([fastContext, mixedEvent, preservedEvent, fileFailureFinish]).entries[0]
            if perInput { entry.inputVerifications["/synthetic/input"] = receipt }
            else { entry.verification = receipt }
            try require(entry.hasPartialSuccess != serious && entry.hasVerificationFailure == serious
                        && entry.statusKey == (serious ? "result.unconfirmed" : "history.status.partial")
                        && entry.statusColor == (serious ? .systemRed : .systemOrange)
                        && entry.succeededLabel == "2",
                        "History confused a warning-only partial result with failed verification evidence")
        }
    }
    let cancelledMixed = try parsed([context, mixedEvent, cancelled]).entries[0]
    let unfinishedMixed = try parsed([context, mixedEvent]).entries[0]
    let contradictoryMixed = try parsed([context, mixedEvent, summary, completed]).entries[0]
    try require(cancelledMixed.statusKey == "history.status.cancelled" && !cancelledMixed.hasPartialSuccess
                && unfinishedMixed.statusKey != "history.status.partial" && !unfinishedMixed.hasPartialSuccess
                && contradictoryMixed.statusKey == "history.status.incomplete" && !contradictoryMixed.hasPartialSuccess,
                "Cancelled, unfinished, or contradictory history became an ordinary partial result")
    let unsafeCancelled = try parsed([context, try payload("MFB_HISTORY_SUMMARY", integritySummary), cancelled]).entries[0]
    try require(unsafeCancelled.statusKey == "result.unconfirmed" && unsafeCancelled.statusColor == .systemRed
                && unsafeCancelled.finished?.outcome == "cancelled" && unsafeCancelled.succeededLabel == "2",
                "Cancellation hid a known integrity failure or erased independent processing facts")
    for (issueCount, countStatus) in [(0, "MATCH"), (1, "MATCH"), (0, "MISMATCH (1 invariant issue)")] {
        var receipt = preservedReceipt
        receipt["has_warnings"] = true
        receipt["issue_count"] = issueCount
        receipt["count_status"] = countStatus
        let entry = try parsed([fastContext, mixedEvent,
            try payload("MFB_HISTORY_VERIFICATION", receipt), cancelled]).entries[0]
        let serious = issueCount > 0 || countStatus.hasPrefix("MISMATCH")
        try require(entry.statusKey == (serious ? "result.unconfirmed" : "history.status.cancelled")
                    && (!serious || entry.statusColor == .systemRed)
                    && entry.finished?.outcome == "cancelled" && entry.succeededLabel == "2"
                    && entry.verification?.count_status == countStatus && !entry.hasPartialSuccess,
                    "Cancellation confused an independent verification failure with a warning-only receipt")
    }
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
    try require(!combined.hasPartialSuccess && combined.summary?.img.succeeded == 2 && combined.summary?.vid.failed == 1,
                "Unverified video failures borrowed another pipeline's preservation proof")
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
        try require(warningEntry.statusKey == "result.unconfirmed" && warningEntry.statusColor == .systemRed
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
                && mixedWarning.statusKey == "result.unconfirmed" && mixedWarning.statusColor == .systemRed,
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
    func writeDatabase(_ sessions: [(String, [String])], schema: Int = 1, destination: URL? = nil) throws {
        var database: OpaquePointer?
        guard sqlite3_open_v2((destination ?? databaseURL).path, &database, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, nil) == SQLITE_OK,
              let database else { throw HostError(message: "Unable to create synthetic history database") }
        defer { sqlite3_close(database) }
        let setup = """
        DROP TABLE IF EXISTS history_removed_sessions_v1;
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
    do {
    let coldDirectory = directory.appendingPathComponent("cold-history")
    try FileManager.default.createDirectory(at: coldDirectory, withIntermediateDirectories: false)
    let coldDatabase = coldDirectory.appendingPathComponent("history.sqlite3")
    try writeDatabase([("cold", [context, summary, completed])], schema: 2, destination: coldDatabase)
    func nativeReadAccess(_ persist: Int32, createWAL: Bool) throws {
        var connection: OpaquePointer?
        guard sqlite3_open_v2(try HistoryStore.databasePath(coldDatabase), &connection, SQLITE_OPEN_READWRITE | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK,
              let connection else { throw HostError(message: "Open cold native fixture failed") }
        defer { sqlite3_close(connection) }
        var value = persist
        try require(sqlite3_file_control(connection, "main", SQLITE_FCNTL_PERSIST_WAL, &value) == SQLITE_OK,
                    "Set fixture WAL persistence failed")
        try require(sqlite3_exec(connection, createWAL ? "PRAGMA journal_mode=WAL" : "PRAGMA query_only=ON; PRAGMA user_version", nil, nil, nil) == SQLITE_OK,
                    "Read cold native fixture failed")
    }
    try nativeReadAccess(0, createWAL: true)
    let coldBytes = try Data(contentsOf: coldDatabase)
    try require(try HistoryStore.needsReadOnlyPreparation(directory: coldDirectory),
                "Cold WAL history must request preparation rather than appear empty")
    var preparations = 0
    let coldLoaded = try HistoryStore.loadForDisplay(directory: coldDirectory) {
        preparations += 1
        try nativeReadAccess(1, createWAL: false)
    }
    try require(preparations == 1 && coldLoaded.entries.count == 1
                && coldLoaded.entries.first?.succeededLabel == "2"
                && (try Data(contentsOf: coldDatabase)) == coldBytes,
                "Cold history preparation lost or rewrote records")
    _ = try HistoryStore.loadForDisplay(directory: coldDirectory) {
        throw HostError(message: "Readable history must not be reopened writable")
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
    try require(try !HistoryStore.needsReadOnlyPreparation(directory: directory),
                "A readable database must not request writable preparation")
    let readReceipt: [String: Any] = ["schema_version": 1, "kind": "history_readonly_access",
        "source": try HistoryStore.databasePath(databaseURL), "history_schema_version": 1]
    try HistoryReadOnlyAccessReceipt.validate(JSONSerialization.data(withJSONObject: readReceipt), directory: directory)
    for override in [["source": databaseURL.path + ".other"], ["history_schema_version": 99]] as [[String: Any]] {
        var invalid = readReceipt
        invalid.merge(override) { _, new in new }
        do {
            try HistoryReadOnlyAccessReceipt.validate(JSONSerialization.data(withJSONObject: invalid), directory: directory)
            throw HostError(message: "Unrelated or unsupported history preparation was accepted")
        } catch let error as HostError {
            try require(error.message != "Unrelated or unsupported history preparation was accepted",
                        "Unrelated or unsupported history preparation was accepted")
        }
    }
    try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: databaseURL.path)
    let databaseLoaded = try HistoryStore.load(directory: directory)
    let expectedStatuses = ["history.status.completed", "history.status.failed", "history.status.cancelled",
        "history.status.preview", "history.status.unfinished", "history.status.file_failures",
        "result.unconfirmed", "history.status.incomplete", "history.status.incomplete"]
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
    let retainedAuditBytes = try Data(contentsOf: rootAudit)
    do {
        var removedDB: OpaquePointer?
        guard sqlite3_open_v2(databaseURL.path, &removedDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let removedDB else {
            throw HostError(message: "Synthetic removal registry open failed")
        }
        defer { sqlite3_close(removedDB) }
        guard sqlite3_exec(removedDB, """
            CREATE TABLE history_removed_sessions_v1(session_id TEXT PRIMARY KEY NOT NULL);
            INSERT INTO history_removed_sessions_v1 VALUES ('test');
            DELETE FROM history_events WHERE session_id='test';
            DELETE FROM history_sessions WHERE session_id='test';
            """, nil, nil, nil) == SQLITE_OK else { throw HostError(message: "Synthetic removal registry setup failed") }
    }
    let removedHistory = try HistoryStore.load(directory: directory)
    try require(!removedHistory.entries.contains { $0.stamp == "test" }
        && removedHistory.entries.contains { $0.stamp == "legacy" }
        && (try Data(contentsOf: rootAudit)) == retainedAuditBytes,
        "Retained legacy audit resurrected a removed database session or was altered")
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
    try writeDatabase([("export", [exportContext, launch, spawn, exportConfig, exited, storage, unknownSummary, exportFinished, adjacentContext, secondLaunch])], schema: 2)
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
    try require(document["schema_version"] as? Int == 2 && document["authoritative_state"] as? Bool == false
                && document["run_id"] is NSNull && document["history_entry_id"] as? String == exportEntries[0].id
                && document["full_run_manifest"] as? Bool == false && scope["total_output_rows"] as? Int == 4
                && scope["retained_output_rows"] as? Int == 3 && scope["diagnostic_log_rows"] as? Int == 2
                && scope["phase_control_rows"] as? Int == 1 && scope["omitted_non_diagnostic_output_rows"] as? Int == 1
                && scope["end_source"] as? String == "terminal_boundary"
                && exportedImage["succeeded"] is NSNull && snapshots.count == 1
                && workerLaunches.count == 1 && workerLaunches[0]["invocation_id"] as? String == "synthetic-call-1"
                && workerLaunches[0]["arguments"] as? [String] == launchValue["arguments"] as? [String]
                && (workerLaunches[0]["process_observations"] as? [[String: Any]])?.count == 2
                && (workerLaunches[0]["process_observations"] as? [[String: Any]])?.last?["exit_code"] as? Int == 7
                && workerLaunches[0]["output_storage_state"] as? String == "committed"
                && (workerLaunches[0]["output_storage"] as? [String: Any])?["committed_records"] as? Int == 3
                && (workerLaunches[0]["output_storage"] as? [String: Any])?["committed_utf8_bytes"] as? Int == 27
                && document["output_storage_unbound_invalid"] as? Bool == false
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
                && ongoingLaunch?["process_observations"] is NSNull
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
    let coldDirectory = directory.appendingPathComponent("cold-wal", isDirectory: true)
    try FileManager.default.createDirectory(at: coldDirectory, withIntermediateDirectories: true)
    let coldDatabase = coldDirectory.appendingPathComponent("history.sqlite3")
    try writeDatabase([("cold-wal", [context, summary, completed])], schema: 2, destination: coldDatabase)
    var coldHandle: OpaquePointer?
    guard sqlite3_open_v2(coldDatabase.path, &coldHandle, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let coldHandle else {
        throw HostError(message: "Cold WAL fixture writer open failed")
    }
    var persist: Int32 = 1
    let preserveResult = sqlite3_file_control(coldHandle, "main", SQLITE_FCNTL_PERSIST_WAL, &persist)
    let coldWriteResult = sqlite3_exec(coldHandle, "PRAGMA journal_mode=WAL; UPDATE history_sessions SET updated_at='2026-10-12T09:00:00Z'", nil, nil, nil)
    let coldCloseResult = sqlite3_close(coldHandle)
    try require(preserveResult == SQLITE_OK && persist == 1 && coldWriteResult == SQLITE_OK && coldCloseResult == SQLITE_OK,
                "Cold WAL fixture did not retain writer-managed sidecars")
    let coldWAL = coldDatabase.path + "-wal"
    try require(FileManager.default.fileExists(atPath: coldWAL) && FileManager.default.fileExists(atPath: coldDatabase.path + "-shm"),
                "Closed history lost WAL sidecars required by read-only system SQLite")
    let coldBytes = try Data(contentsOf: coldDatabase)
    let coldWALBytes = try Data(contentsOf: URL(fileURLWithPath: coldWAL))
    let coldHistory = try HistoryStore.load(directory: coldDirectory)
    let coldAfter = try Data(contentsOf: coldDatabase)
    let coldWALAfter = try Data(contentsOf: URL(fileURLWithPath: coldWAL))
    try require(coldHistory.entries.count == 1 && coldHistory.entries[0].statusKey == "history.status.completed"
                && coldAfter == coldBytes && coldWALAfter == coldWALBytes,
                "Cold WAL read failed, changed outcomes, or wrote database/committed WAL bytes")
    var identityContext = outputContext
    identityContext["output_sequence"] = 0
    identityContext["run_id"] = "run-synthetic-1"
    let identityContextLine = try payload("MFB_HISTORY_CONTEXT", identityContext)
    var identityLaunch = launchValue
    identityLaunch["run_id"] = "run-synthetic-1"
    let identityLaunchLine = try payload("MFB_HISTORY_INVOCATION", identityLaunch)
    var identitySummary = summaryValue
    identitySummary["run_id"] = "run-synthetic-1"
    var identityMedia = media
    identityMedia["failed"] = 1
    identitySummary["img"] = identityMedia
    let exactPath = "relative/映像 🚀\nfile's.jpg"
    identitySummary["failed_files"] = [exactPath, "/synthetic/no-reason.jpg"]
    let identitySummaryLine = try payload("MFB_HISTORY_SUMMARY", identitySummary)
    let identityFinished = try payload("MFB_HISTORY_FINISHED", ["schema_version": 1, "outcome": "failed", "error": "synthetic",
        "run_id": "run-synthetic-1", "output_sequence": 3])
    let identityEvents = [identityContextLine, identityLaunchLine, identitySummaryLine, identityFinished]
    let identityAudit = try parsed(identityEvents)
    try require(identityAudit.entries[0].issues.isEmpty && identityAudit.entries[0].context?.run_id == "run-synthetic-1",
                "Valid run identity did not bind the context, invocation, summary, and finished event")
    for rejectedSummary in [summary, try payload("MFB_HISTORY_SUMMARY", identitySummary.merging(["run_id": "other-run"]) { _, new in new })] {
        let invalid = try parsed([identityContextLine, rejectedSummary, identityFinished]).entries[0]
        try require(invalid.summary == nil && invalid.needsAttention, "Missing or mismatched run identity was accepted")
    }
    try require(try parsed(identityEvents + [identityContextLine]).entries.contains(where: { !$0.issues.isEmpty }),
                "A reused run identity silently opened another batch")
    var markerValue: [String: Any] = ["schema_version": 1, "pipeline": "img", "source_path": exactPath,
        "absolute_path": "/synthetic/absent/映像 🚀\nfile's.jpg", "message": "synthetic decoder failed\nno guessed root cause",
        "reason_code": "decoder_failed", "run_id": "run-synthetic-1", "invocation_id": "synthetic-call-1"]
    let markerLine = try payload("MFB_FILE_FAILURE", markerValue)
    markerValue["message"] = "synthetic metadata failure"
    markerValue["reason_code"] = nil
    let secondMarkerLine = try payload("MFB_FILE_FAILURE", markerValue)
    let identityEntry = identityAudit.entries[0]
    try require(try HistoryFileFailure.parse(markerLine, context: identityEntry.context!, invocations: identityEntry.invocations, pipeline: "IMG")?.source_path == exactPath,
                "File marker parser lost Unicode, newline, quotes, or the explicit invocation relation")
    for invalidMarker in [markerValue.merging(["run_id": "other-run"]) { _, new in new },
                          markerValue.merging(["invocation_id": "unknown-invocation"]) { _, new in new },
                          markerValue.merging(["pipeline": "vid"]) { _, new in new },
                          markerValue.merging(["fingerprint": "guessed"]) { _, new in new },
                          markerValue.merging(["message": String(repeating: "x", count: 64 * 1024)]) { _, new in new }] {
        try refuseExport({ _ = try HistoryFileFailure.parse(payload("MFB_FILE_FAILURE", invalidMarker), context: identityEntry.context!, invocations: identityEntry.invocations, pipeline: "IMG") },
            "Malformed or unbound file failure evidence was accepted")
    }
    try writeDatabase([("file-reasons", identityEvents)], schema: 2)
    var reasonDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &reasonDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let reasonDB else {
        throw HostError(message: "Synthetic file failure database open failed")
    }
    func insertFailureLines(_ lines: [String]) throws {
        var query: OpaquePointer?
        guard sqlite3_prepare_v2(reasonDB, "INSERT INTO history_output(session_id,ts,pipeline,line) VALUES ('file-reasons','synthetic','IMG',?)", -1, &query, nil) == SQLITE_OK, let query else {
            throw HostError(message: "Synthetic file failure insert prepare failed")
        }
        defer { sqlite3_finalize(query) }
        for line in lines {
            sqlite3_reset(query)
            guard line.withCString({ sqlite3_bind_text(query, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
                  sqlite3_step(query) == SQLITE_DONE else { throw HostError(message: "Synthetic file failure insert failed") }
        }
    }
    try insertFailureLines([markerLine, markerLine, secondMarkerLine])
    let reasonEntry = try HistoryStore.load(directory: directory).entries.first(where: { $0.stamp == "file-reasons" })!
    let reasonSnapshot = try Data(contentsOf: databaseURL)
    let reasonIssues = try HistoryStore.batchIssues(for: reasonEntry)
    try require(reasonIssues.fileFailures.count == 3 && reasonIssues.scannedRows == 3 && reasonEntry.summary?.img.failed == 1,
                "File failure log observations changed authoritative media outcome counts")
    var otherMarker = markerValue
    otherMarker["run_id"] = "run-synthetic-2"
    otherMarker["invocation_id"] = "synthetic-call-2"
    otherMarker["message"] = "synthetic decoder failed\nno guessed root cause"
    otherMarker["reason_code"] = "decoder_failed"
    let sharedFailure = try HistoryFileFailure.decode(payload("MFB_FILE_FAILURE", otherMarker))!
    otherMarker["message"] = "synthetic metadata failure"
    otherMarker["reason_code"] = "different-code"
    let changedCode = try HistoryFileFailure.decode(payload("MFB_FILE_FAILURE", otherMarker))!
    otherMarker["reason_code"] = nil
    otherMarker["absolute_path"] = "/other-root/file.jpg"
    let changedAbsolute = try HistoryFileFailure.decode(payload("MFB_FILE_FAILURE", otherMarker))!
    let comparisonIssues = HistoryStore.BatchIssues(groups: [], occurrences: 0,
        fileFailures: [sharedFailure, sharedFailure, changedCode, changedAbsolute], scannedRows: 4, start: 0, upper: 4,
        endSource: "snapshot_only")
    let failureComparison = try ProcessingHistoryPanel.renderFailureComparison([reasonIssues, comparisonIssues])
    try require(failureComparison.contains(localized("history.compare.failures.totals", 1, 1, 2))
                && failureComparison.contains(localized("history.compare.failures.observations", 2, 2))
                && failureComparison.contains(exactPath) && failureComparison.contains(localized("history.compare.failures.note"))
                && failureComparison.contains(localized("history.batch_issues.scope_snapshot", Int64(4), Int64(0), Int64(4)))
                && reasonEntry.summary?.img.failed == 1,
                "Reason comparison merged codes/absolute paths, compared run IDs, or invented failed-file counts")
    let oversizedReasons = (0...1_000).map { index in
        HistoryFileFailure(schema_version: 1, pipeline: "img", source_path: "/synthetic/\(index).jpg", absolute_path: nil,
                           message: "synthetic", run_id: "run", invocation_id: "invocation", reason_code: nil)
    }
    let oversizedComparison = HistoryStore.BatchIssues(groups: [], occurrences: 0, fileFailures: oversizedReasons,
        scannedRows: 1_001, start: 0, upper: 1_001, endSource: "terminal_boundary")
    try refuseExport({ _ = try ProcessingHistoryPanel.renderFailureComparison([reasonIssues, oversizedComparison]) },
                     "Oversized comparison displayed a misleading partial result")
    let largeReasons = (0..<40).map { index in
        HistoryFileFailure(schema_version: 1, pipeline: "img", source_path: "/synthetic/large-\(index).jpg", absolute_path: nil,
                           message: String(repeating: "x", count: 60 * 1_024), run_id: "run", invocation_id: "invocation", reason_code: nil)
    }
    let largeComparison = HistoryStore.BatchIssues(groups: [], occurrences: 0, fileFailures: largeReasons,
        scannedRows: 40, start: 0, upper: 40, endSource: "terminal_boundary")
    try refuseExport({ _ = try ProcessingHistoryPanel.renderFailureComparison([reasonIssues, largeComparison]) },
                     "Comparison text overflow displayed a misleading partial result")
    try MainActor.assumeIsolated {
        try ProcessingHistoryPanel(directory: directory).validateFailureComparisonForSelfTest(reasonEntry,
            results: [reasonIssues, comparisonIssues])
    }
    let reasonDocument = try JSONSerialization.jsonObject(with: HistoryStore.diagnosticArtifact(for: reasonEntry)) as? [String: Any]
    try require(reasonDocument?["run_id"] as? String == "run-synthetic-1"
                && (reasonDocument?["scope"] as? [String: Any])?["file_failure_observation_rows"] as? Int == 3
                && (reasonDocument?["records"] as? [[String: Any]])?.allSatisfy({ $0["kind"] as? String == "file_failure_observation" }) == true,
                "Diagnostic export omitted exact failure observations or confused run and entry identity")
    try MainActor.assumeIsolated { try ProcessingHistoryPanel(directory: directory).validateFileReasonsForSelfTest(reasonEntry, issues: reasonIssues) }
    try require(try Data(contentsOf: databaseURL) == reasonSnapshot, "File failure scan, export, or GUI mutated history")
    guard sqlite3_exec(reasonDB, "UPDATE history_output SET line='MFB_FILE_FAILURE={invalid}' WHERE sequence=1", nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic file failure corruption failed")
    }
    try refuseExport({ _ = try HistoryStore.batchIssues(for: reasonEntry) }, "Malformed file failure evidence appeared as zero observations")
    try refuseExport({ _ = try HistoryStore.diagnosticArtifact(for: reasonEntry) }, "Malformed file failure evidence was silently excluded from export")
    sqlite3_close(reasonDB)
    var ignoredSummary = identitySummary
    var ignoredMedia = identityMedia
    ignoredMedia["ignored"] = 2
    ignoredSummary["img"] = ignoredMedia
    let ignoredFinished: [String: Any] = ["schema_version": 1, "outcome": "failed", "error": "synthetic",
        "run_id": "run-synthetic-1", "output_sequence": 4]
    let ignoredEvents = [identityContextLine, identityLaunchLine,
        try payload("MFB_HISTORY_SUMMARY", ignoredSummary), try payload("MFB_HISTORY_FINISHED", ignoredFinished)]
    var ignoredValue: [String: Any] = ["schema_version": 1, "pipeline": "img", "run_id": "run-synthetic-1",
        "invocation_id": "synthetic-call-1", "source_path": "ignored/映像\nfile's.png",
        "absolute_path": "/synthetic/absent/映像\nfile's.png", "reason_code": "outside_current_policy_scope",
        "message": "Synthetic policy scope observation", "ordinal": 1, "total": 2]
    let firstIgnoredLine = try payload("MFB_FAST_IMG_IGNORED", ignoredValue)
    let firstIgnored = try HistoryIgnoredSource.decode(firstIgnoredLine)!
    ignoredValue["source_path"] = "ignored/live.heic"
    ignoredValue["absolute_path"] = "/synthetic/absent/live.heic"
    ignoredValue["reason_code"] = "live_photo_pair_member"
    ignoredValue["message"] = "Synthetic Live Photo policy observation"
    ignoredValue["ordinal"] = 2
    let secondIgnoredLine = try payload("MFB_FAST_IMG_IGNORED", ignoredValue)
    let secondIgnored = try HistoryIgnoredSource.decode(secondIgnoredLine)!
    let ignoredRecords = try HistoryIgnoredSource.reconcile([firstIgnored, secondIgnored, firstIgnored], expected: 2)
    try require(ignoredRecords.count == 2 && ignoredRecords[0].source_path.utf8.elementsEqual(firstIgnored.source_path.utf8),
                "Repeated ignored observations inflated counts or changed the exact source path")
    for (values, expected) in [([firstIgnored], 2), ([firstIgnored, secondIgnored], 3), ([], 2)] {
        try refuseExport({ _ = try HistoryIgnoredSource.reconcile(values, expected: expected) }, "Missing ignored evidence appeared as a matched inventory")
    }
    try refuseExport({ _ = try HistoryIgnoredSource.reconcile([], expected: nil) }, "Missing ignored count became zero")
    try require(try HistoryIgnoredSource.reconcile([], expected: 0).isEmpty, "An explicit zero ignored count required invented file records")
    var conflictingIgnored = ignoredValue
    conflictingIgnored["ordinal"] = 1
    let conflictingIgnoredRecord = try HistoryIgnoredSource.decode(payload("MFB_FAST_IMG_IGNORED", conflictingIgnored))!
    try refuseExport({ _ = try HistoryIgnoredSource.reconcile([firstIgnored, secondIgnored, conflictingIgnoredRecord], expected: 2) }, "A conflicting ordinal replaced recorded ignored evidence")
    conflictingIgnored["ordinal"] = 2
    conflictingIgnored["source_path"] = firstIgnored.source_path
    let duplicatePath = try HistoryIgnoredSource.decode(payload("MFB_FAST_IMG_IGNORED", conflictingIgnored))!
    try refuseExport({ _ = try HistoryIgnoredSource.reconcile([firstIgnored, duplicatePath], expected: 2) }, "Duplicate ignored paths inflated the classified inventory")
    for invalid in [ignoredValue.merging(["source_path": "../escape.png"]) { _, new in new },
                    ignoredValue.merging(["total": 1]) { _, new in new },
                    ignoredValue.merging(["schema_version": 2]) { _, new in new },
                    ignoredValue.merging(["guessed_stage": "safe"]) { _, new in new }] {
        try refuseExport({ _ = try HistoryIgnoredSource.decode(payload("MFB_FAST_IMG_IGNORED", invalid)) }, "Invalid ignored marker was accepted")
    }
    try writeDatabase([("ignored", ignoredEvents)], schema: 2)
    var ignoredDB: OpaquePointer?
    guard sqlite3_open_v2(databaseURL.path, &ignoredDB, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK, let ignoredDB else {
        throw HostError(message: "Synthetic ignored database open failed")
    }
    func insertIgnoredOutput(_ line: String) throws {
        var query: OpaquePointer?
        guard sqlite3_prepare_v2(ignoredDB, "INSERT INTO history_output(session_id,ts,pipeline,line) VALUES ('ignored','synthetic','IMG',?)", -1, &query, nil) == SQLITE_OK,
              let query else { throw HostError(message: "Synthetic ignored output prepare failed") }
        defer { sqlite3_finalize(query) }
        guard line.withCString({ sqlite3_bind_text(query, 1, $0, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self)) }) == SQLITE_OK,
              sqlite3_step(query) == SQLITE_DONE else { throw HostError(message: "Synthetic ignored output insert failed") }
    }
    for line in [firstIgnoredLine, secondIgnoredLine, firstIgnoredLine, "ordinary output with embedded MFB_FAST_IMG_IGNORED= text"] { try insertIgnoredOutput(line) }
    guard let ignoredEntry = try HistoryStore.load(directory: directory).entries.first(where: { $0.stamp == "ignored" }) else {
        throw HostError(message: "The synthetic ignored batch is absent")
    }
    let ignoredSnapshot = try Data(contentsOf: databaseURL)
    let ignoredIssues: HistoryStore.BatchIssues
    do { ignoredIssues = try HistoryStore.batchIssues(for: ignoredEntry, ignoredOnly: true) }
    catch { throw HostError(message: "Initial ignored inventory read failed: \(error.localizedDescription); audit issues: \(ignoredEntry.issues.joined(separator: "; "))") }
    try require(ignoredIssues.scannedRows == 3 && ignoredIssues.ignoredSources.count == 2 && ignoredIssues.fileFailures.isEmpty,
                "Ignored-only scan included unrelated text, inflated repeats or mixed failure records")
    let ignoredDocument = try JSONSerialization.jsonObject(with: HistoryStore.diagnosticArtifact(for: ignoredEntry)) as? [String: Any]
    try require((ignoredDocument?["ignored_inventory"] as? [String: Any])?["state"] as? String == "matches_reported_total"
                && (ignoredDocument?["scope"] as? [String: Any])?["ignored_source_observation_rows"] as? Int == 3
                && (ignoredDocument?["records"] as? [[String: Any]])?.count == 3,
                "Ignored export lost original observations or substituted observation count for item count")
    try MainActor.assumeIsolated { try ProcessingHistoryPanel(directory: directory).validateIgnoredForSelfTest(ignoredEntry, records: ignoredIssues.ignoredSources) }
    try require(try Data(contentsOf: databaseURL) == ignoredSnapshot, "Ignored history inspection changed the stored database")
    guard sqlite3_exec(ignoredDB, "UPDATE history_output SET line=zeroblob(2097153) WHERE sequence=4", nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic unrelated large output update failed")
    }
    try require(try HistoryStore.batchIssues(for: ignoredEntry, ignoredOnly: true).ignoredSources.count == 2,
                "Ignored inventory unnecessarily read unrelated oversized output")
    guard sqlite3_exec(ignoredDB, "UPDATE history_output SET line='MFB_FAST_IMG_IGNORED={invalid}' WHERE sequence=1", nil, nil, nil) == SQLITE_OK else {
        throw HostError(message: "Synthetic ignored corruption failed")
    }
    try refuseExport({ _ = try HistoryStore.batchIssues(for: ignoredEntry, ignoredOnly: true) }, "Malformed ignored record became a complete list")
    sqlite3_close(ignoredDB)
    do {
        var cleanupContext = identityContext
        cleanupContext["mode"] = "fast-img"
        cleanupContext["output"] = "/synthetic/output"
        cleanupContext["shortest_path"] = true
        var finish = ignoredFinished
        finish["output_sequence"] = 0
        let receipt: [String: Any] = ["schema_version": 1, "run_id": "run-synthetic-1", "output_path": "/synthetic/output",
            "outcome": "failed", "removed_files": 2, "already_absent_files": 0, "pruned_directories": NSNull(),
            "error": "synthetic directory permission denied"]
        let context = try payload("MFB_HISTORY_CONTEXT", cleanupContext)
        let end = try payload("MFB_HISTORY_FINISHED", finish)
        let cleanup = try payload("MFB_HISTORY_OUTPUT_CLEANUP", receipt)
        try writeDatabase([("cleanup", [context, identitySummaryLine, cleanup, end])], schema: 2)
        let entry = try HistoryStore.load(directory: directory).entries.first(where: { $0.stamp == "cleanup" })!
        let before = try Data(contentsOf: databaseURL)
        let document = try JSONSerialization.jsonObject(with: HistoryStore.diagnosticArtifact(for: entry)) as? [String: Any]
        let exported = document?["output_cleanup"] as? [String: Any]
        let rows = exported?["receipts"] as? [[String: Any]]
        try require(exported?["state"] as? String == "recorded" && rows?.count == 1
                    && rows?.first?["removed_files"] as? Int == 2 && rows?.first?["pruned_directories"] is NSNull
                    && rows?.first?["error"] as? String == receipt["error"] as? String
                    && Data(contentsOf: databaseURL) == before,
                    "Read-only diagnostic export lost partial cleanup evidence or changed its database")
        var conflicting = receipt
        conflicting["removed_files"] = 3
        try writeDatabase([("cleanup", [context, identitySummaryLine, cleanup,
            try payload("MFB_HISTORY_OUTPUT_CLEANUP", conflicting), end])], schema: 2)
        try refuseExport({ _ = try HistoryStore.diagnosticArtifact(for: entry) },
                         "Conflicting output cleanup evidence exported a successful diagnostic")
    }
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
