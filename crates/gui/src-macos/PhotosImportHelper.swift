// Versioned, file-backed PhotoKit worker. One writer; stdout is JSONL only.
import Foundation
import Photos
import Darwin

private struct ImportResource: Codable {
    let path: String
    let kind: String
    let originalFilename: String
    let blake3: String

    var photoType: PHAssetResourceType {
        get throws {
            switch kind {
            case "photo": return .photo
            case "video": return .video
            case "pairedVideo": return .pairedVideo
            case "alternatePhoto": return .alternatePhoto
            default: throw failure("Unsupported resource composition: \(kind)")
            }
        }
    }
}

private struct ImportAsset: Codable {
    let entryID: String
    let resources: [ImportResource]
    let albumIdentifier: String?
}

private struct ImportRequest: Codable {
    let version: Int
    let operation: String
    let batchID: String
    let witnessIdentifiers: [String]?
    let assets: [ImportAsset]?
    let journalPath: String?
    let identifiers: [String]?
}

private func failure(_ message: String) -> NSError {
    NSError(domain: "MFB.PhotoKit", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
}

// LaunchServices gives the helper its own PhotoKit/TCC identity. The parent
// speaks the same JSONL protocol over a private local socket, not app stdio.
private func connectParent(socketPath: String, lockPath: String) throws -> Int32 {
    for path in [socketPath, lockPath] {
        let parent = URL(fileURLWithPath: path).deletingLastPathComponent().path
        let attrs = try FileManager.default.attributesOfItem(atPath: parent)
        guard path.hasPrefix("/"), attrs[.type] as? FileAttributeType == .typeDirectory,
              (attrs[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(),
              let permissions = attrs[.posixPermissions] as? NSNumber,
              permissions.intValue & 0o077 == 0 else { throw failure("IPC requires a private owned directory") }
    }
    let lock = Darwin.open(lockPath, O_CREAT | O_RDWR | O_NOFOLLOW, 0o600)
    guard lock >= 0 else { throw failure("Cannot open native writer lock") }
    var writer = flock()
    writer.l_type = Int16(F_WRLCK)
    writer.l_whence = Int16(SEEK_SET)
    guard Darwin.fcntl(lock, F_SETLK, &writer) == 0 else {
        Darwin.close(lock)
        throw failure("Another native PhotoKit writer is still active; reconcile before retry")
    }
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(socketPath.utf8) + [0]
    guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else {
        Darwin.close(lock)
        throw failure("IPC socket path too long")
    }
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    withUnsafeMutableBytes(of: &address.sun_path) { raw in raw.copyBytes(from: bytes) }
    let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { Darwin.close(lock); throw failure("Cannot create IPC socket") }
    let result = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard result == 0, dup2(fd, STDIN_FILENO) >= 0, dup2(fd, STDOUT_FILENO) >= 0 else {
        Darwin.close(fd); Darwin.close(lock); throw failure("Cannot connect to import parent")
    }
    Darwin.close(fd)
    return lock // Kept until process exit, even if the parent dies during a commit.
}

private func emit(_ value: [String: Any]) throws {
    var data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    data.append(10)
    try FileHandle.standardOutput.write(contentsOf: data)
}

private func durableJournal(_ value: [String: Any], at path: String, create: Bool) throws {
    let url = URL(fileURLWithPath: path)
    let parent = url.deletingLastPathComponent()
    let attributes = try FileManager.default.attributesOfItem(atPath: parent.path)
    guard path.hasPrefix("/"), attributes[.type] as? FileAttributeType == .typeDirectory,
          let permissions = attributes[.posixPermissions] as? NSNumber,
          permissions.intValue & 0o077 == 0 else {
        throw failure("Journal requires an absolute path in a private directory")
    }
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    if create {
        let fd = Darwin.open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
        guard fd >= 0 else { throw failure("Journal already exists or cannot be created: errno=\(errno)") }
        let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        try file.write(contentsOf: data)
        try file.synchronize()
        try file.close()
    } else {
        let attrs = try FileManager.default.attributesOfItem(atPath: path)
        guard attrs[.type] as? FileAttributeType == .typeRegular else {
            throw failure("Journal is not a regular file")
        }
        try data.write(to: url, options: .atomic)
        let file = try FileHandle(forWritingTo: url)
        try file.synchronize()
        try file.close()
    }
    let directory = Darwin.open(parent.path, O_RDONLY)
    guard directory >= 0 else { throw failure("Cannot open journal directory for sync") }
    defer { Darwin.close(directory) }
    guard Darwin.fsync(directory) == 0 else { throw failure("Cannot sync journal directory") }
}

private func authorize() throws {
    let done = DispatchSemaphore(value: 0)
    var status = PHAuthorizationStatus.notDetermined
    PHPhotoLibrary.requestAuthorization(for: .readWrite) { value in
        status = value
        done.signal()
    }
    guard done.wait(timeout: .now() + 120) == .success else {
        throw failure("PhotoKit authorization timed out; no import submitted")
    }
    guard status == .authorized else {
        throw failure("Full PhotoKit read/write authorization required (status \(status.rawValue)); no import submitted")
    }
}

private func visibleIdentifiers(_ identifiers: [String]) -> [String] {
    var result: [String] = []
    PHAsset.fetchAssets(withLocalIdentifiers: identifiers, options: nil).enumerateObjects { asset, _, _ in
        result.append(asset.localIdentifier)
    }
    return result.sorted()
}

private func verifyWitness(_ identifiers: [String]) throws {
    guard !identifiers.isEmpty, Set(identifiers).count == identifiers.count else {
        throw failure("Explicit target-library witness identifiers required")
    }
    let actual = visibleIdentifiers(identifiers)
    guard Set(actual) == Set(identifiers) else {
        throw failure("PhotoKit target does not contain the selected library witnesses; no import submitted")
    }
}

private func importAssets(_ request: ImportRequest) throws -> [String: Any] {
    guard let assets = request.assets, !assets.isEmpty, assets.count <= 1000,
          Set(assets.map(\.entryID)).count == assets.count,
          assets.allSatisfy({ !$0.entryID.isEmpty }), let journal = request.journalPath else {
        throw failure("Import requires 1...1000 unique task entries and a durable journal")
    }
    // Validate all local inputs before creating any PhotoKit change request.
    for asset in assets {
        guard !asset.resources.isEmpty, asset.resources.count <= 4 else {
            throw failure("Asset requires 1...4 explicit original resources")
        }
        let kinds = Set(asset.resources.map(\.kind))
        guard kinds.count == asset.resources.count,
              kinds == ["video"] || (kinds.contains("photo") && !kinds.contains("video")) else {
            throw failure("Ambiguous or incomplete original resource composition")
        }
        for resource in asset.resources {
            _ = try resource.photoType
            guard resource.path.hasPrefix("/"), !resource.originalFilename.isEmpty,
                  resource.blake3.utf8.count == 64,
                  resource.blake3.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
                  try FileManager.default.attributesOfItem(atPath: resource.path)[.type] as? FileAttributeType == .typeRegular else {
                throw failure("Invalid file-backed original resource")
            }
        }
    }
    try verifyWitness(request.witnessIdentifiers ?? [])
    var albums: [String: PHAssetCollection] = [:]
    for id in Set(assets.compactMap(\.albumIdentifier)) {
        guard let album = PHAssetCollection.fetchAssetCollections(withLocalIdentifiers: [id], options: nil).firstObject else {
            throw failure("Requested album is not visible in the selected PhotoKit library")
        }
        albums[id] = album
    }
    let encoded = try JSONEncoder().encode(request)
    var record: [String: Any] = ["version": 1, "batchID": request.batchID, "state": "submitted",
        "request": try JSONSerialization.jsonObject(with: encoded)]
    try durableJournal(record, at: journal, create: true)
    let started = ProcessInfo.processInfo.systemUptime
    var identities: [[String: String]] = []
    var transactionError: Error?
    var committed = false
    let done = DispatchSemaphore(value: 0)
    PHPhotoLibrary.shared().performChanges({
        autoreleasepool {
            var albumAssets: [String: [PHObjectPlaceholder]] = [:]
            for asset in assets {
                let creation = PHAssetCreationRequest.forAsset()
                for resource in asset.resources {
                    let options = PHAssetResourceCreationOptions()
                    options.shouldMoveFile = false
                    options.originalFilename = resource.originalFilename
                    do {
                        creation.addResource(with: try resource.photoType, fileURL: URL(fileURLWithPath: resource.path), options: options)
                    } catch {
                        // Intent already durable. A worker exit is uncertain, never a rollback claim.
                        Darwin._exit(74)
                    }
                }
                guard let placeholder = creation.placeholderForCreatedAsset else { Darwin._exit(74) }
                identities.append(["entryID": asset.entryID, "localIdentifier": placeholder.localIdentifier])
                if let id = asset.albumIdentifier {
                    albumAssets[id, default: []].append(placeholder)
                }
            }
            for (id, placeholders) in albumAssets {
                guard let album = albums[id],
                      let change = PHAssetCollectionChangeRequest(for: album) else { Darwin._exit(74) }
                change.addAssets(placeholders as NSArray)
            }
            record["identities"] = identities
            record["state"] = "identifier-known"
            do { try durableJournal(record, at: journal, create: false) }
            catch { Darwin._exit(74) }
        }
    }, completionHandler: { success, error in
        committed = success
        transactionError = error
        done.signal()
    })
    guard done.wait(timeout: .now() + 600) == .success else {
        // Do not accept another write while the prior transaction might still commit.
        try emit(["version": 1, "batchID": request.batchID, "state": "uncertain", "error": "PhotoKit transaction deadline exceeded; reconcile journal before retry"])
        Darwin._exit(75)
    }
    record["state"] = committed ? "committed" : "uncertain"
    record["transactionSeconds"] = ProcessInfo.processInfo.systemUptime - started
    var usage = rusage()
    if getrusage(RUSAGE_SELF, &usage) == 0 {
        record["peakRSSBytes"] = usage.ru_maxrss
        record["userCPUSeconds"] = Double(usage.ru_utime.tv_sec) + Double(usage.ru_utime.tv_usec) / 1_000_000
        record["systemCPUSeconds"] = Double(usage.ru_stime.tv_sec) + Double(usage.ru_stime.tv_usec) / 1_000_000
    }
    if let error = transactionError { record["error"] = error.localizedDescription }
    try durableJournal(record, at: journal, create: false)
    // Success is not custody proof. Rust must re-query original resources and hash them.
    return record
}

@main
private struct PhotosImportHelper {
    static func main() {
        let args = CommandLine.arguments
        var writerLock: Int32 = -1
        if args.count == 5, args[1] == "--socket", args[3] == "--lock" {
            do { writerLock = try connectParent(socketPath: args[2], lockPath: args[4]) }
            catch { fputs("\(error.localizedDescription)\n", stderr); Darwin.exit(74) }
        } else if args.count != 1 {
            fputs("Usage: mfb-photos-import [--socket path --lock path]\n", stderr)
            Darwin.exit(64)
        }
        defer { if writerLock >= 0 { Darwin.close(writerLock) } }
        DispatchQueue.global(qos: .userInitiated).async {
            while let line = readLine() {
                autoreleasepool {
                    var batch = ""
                    do {
                        guard line.utf8.count <= 4 * 1024 * 1024 else { throw failure("Oversized request") }
                        let request = try JSONDecoder().decode(ImportRequest.self, from: Data(line.utf8))
                        batch = request.batchID
                        guard request.version == 1 else { throw failure("Unsupported PhotoKit protocol version") }
                        switch request.operation {
                        case "probe":
                            try authorize()
                            try verifyWitness(request.witnessIdentifiers ?? [])
                            try emit(["version": 1, "batchID": batch, "state": "ready", "writerConcurrency": 1])
                        case "import":
                            guard PHPhotoLibrary.authorizationStatus(for: .readWrite) == .authorized else {
                                throw failure("Probe/authorization must succeed before import")
                            }
                            let result = try importAssets(request)
                            try emit(result)
                            if result["state"] as? String != "committed" { Darwin._exit(75) }
                        case "reconcile":
                            try verifyWitness(request.witnessIdentifiers ?? [])
                            try emit(["version": 1, "batchID": batch, "state": "reconciled", "visibleIdentifiers": visibleIdentifiers(request.identifiers ?? [])])
                        default: throw failure("Unknown PhotoKit operation")
                        }
                    } catch {
                        do { try emit(["version": 1, "batchID": batch, "state": "error", "error": error.localizedDescription]) }
                        catch { Darwin._exit(74) }
                        // Stop this session. A journal may already contain an intent
                        // or committed identifiers; later requests must not run blindly.
                        Darwin._exit(75)
                    }
                }
            }
            Darwin.exit(0)
        }
        RunLoop.main.run()
    }
}
