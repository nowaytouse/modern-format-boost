import Darwin
import Dispatch
import Foundation

/// A process keeps one role for its lifetime. Child panels do not acquire another lease.
final class ApplicationSession {
    enum Role: String {
        case production
        case debug
        case backgroundSelfTest

        var activationNotificationName: Notification.Name {
            Notification.Name("com.modernformatboost.gui.activate.\(rawValue)")
        }
    }

    enum BootstrapResult {
        case acquired(ApplicationSession)
        case alreadyRunning(Role)
    }

    private(set) static var current: ApplicationSession?

    /// Read only after bootstrap in production; the fallback preserves existing callers and hidden tests.
    static var activePreferences: UserDefaults { current?.preferences ?? .standard }

    let role: Role
    let debugRoot: URL?
    let preferences: UserDefaults
    var isDebug: Bool { role == .debug }
    var readOnlyPreviewOnly: Bool { isDebug }
    private let lease: WindowLease?

    private init(role: Role, debugRoot: URL?, preferences: UserDefaults, lease: WindowLease?) {
        self.role = role
        self.debugRoot = debugRoot
        self.preferences = preferences
        self.lease = lease
    }

    static func requestedRole(arguments: [String]) -> Role {
        if arguments.contains("--self-test") { return .backgroundSelfTest }
        if arguments.contains("--debug-window")
            || arguments.contains(where: { $0.hasPrefix("--") && $0.hasSuffix("-ui-fixture") }) {
            return .debug
        }
        return .production
    }

    static func bootstrap(arguments: [String] = CommandLine.arguments) throws -> BootstrapResult {
        guard current == nil else { throw SessionError("Application session was already initialized") }
        let role = requestedRole(arguments: arguments)
        let lease: WindowLease?
        if role == .backgroundSelfTest {
            lease = nil
        } else {
            let coordination = try coordinationDirectory()
            guard let acquired = try WindowLease.acquire(directory: coordination, role: role) else {
                return .alreadyRunning(role)
            }
            lease = acquired
        }
        let preferences: UserDefaults
        let debugRoot: URL?
        if role == .debug {
            guard let isolated = UserDefaults(suiteName: "com.modernformatboost.debug-window") else {
                throw SessionError("Cannot open isolated debug preferences")
            }
            preferences = isolated
            let root = FileManager.default.temporaryDirectory
                .appendingPathComponent("mfb-debug-window-\(UUID().uuidString)", isDirectory: true)
            try ensureDirectory(root, privateMode: true, allowExisting: false)
            try ensureDirectory(root.appendingPathComponent("logs"), privateMode: true)
            try ensureDirectory(root.appendingPathComponent("tmp"), privateMode: true)
            debugRoot = root
        } else {
            preferences = .standard
            debugRoot = nil
        }
        let session = ApplicationSession(role: role, debugRoot: debugRoot, preferences: preferences, lease: lease)
        current = session
        return .acquired(session)
    }

    /// GUI queries and children share this context; terminal handoff needs explicit environment arguments too.
    func childEnvironment(base: [String: String] = ProcessInfo.processInfo.environment) -> [String: String] {
        guard let debugRoot else { return base }
        var environment = base
        environment["MFB_HOME_ROOT"] = debugRoot.path
        environment["MFB_LOG_DIR"] = debugRoot.appendingPathComponent("logs").path
        environment["MFB_LOG_DIRECTORY"] = debugRoot.appendingPathComponent("logs").path
        environment["TMPDIR"] = debugRoot.appendingPathComponent("tmp").path
        return environment
    }

    private static func coordinationDirectory() throws -> URL {
        // The account database, rather than HOME/MFB_HOME_ROOT, determines the shared per-user namespace.
        guard let account = getpwuid(geteuid()), let rawHome = account.pointee.pw_dir,
              let home = String(validatingUTF8: rawHome), home.hasPrefix("/") else {
            throw SessionError("Cannot determine the current account's home directory")
        }
        let support = URL(fileURLWithPath: home, isDirectory: true).resolvingSymlinksInPath()
            .appendingPathComponent("Library/Application Support", isDirectory: true)
        try FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        let root = support.appendingPathComponent("Modern Format Boost", isDirectory: true)
        try ensureDirectory(root, privateMode: true)
        let coordination = root.appendingPathComponent("locks", isDirectory: true)
        try ensureDirectory(coordination, privateMode: true)
        return coordination
    }

    private static func ensureDirectory(_ directory: URL, privateMode: Bool, allowExisting: Bool = true) throws {
        if Darwin.mkdir(directory.path, 0o700) != 0 {
            guard allowExisting, errno == EEXIST else {
                throw systemError("Cannot create application session directory")
            }
        }
        var metadata = stat()
        guard Darwin.lstat(directory.path, &metadata) == 0 else {
            throw systemError("Cannot inspect application session directory")
        }
        guard metadata.st_mode & S_IFMT == S_IFDIR, metadata.st_uid == geteuid(),
              metadata.st_mode & (privateMode ? 0o077 : 0o022) == 0 else {
            throw SessionError("Application session directory is not a safe owned directory")
        }
    }

    private static func systemError(_ context: String) -> SessionError {
        SessionError("\(context): \(String(cString: strerror(errno)))")
    }

    private struct SessionError: LocalizedError {
        let message: String
        init(_ message: String) { self.message = message }
        var errorDescription: String? { message }
    }

    private final class WindowLease {
        let descriptor: Int32
        private init(_ descriptor: Int32) { self.descriptor = descriptor }
        deinit { Darwin.close(descriptor) }

        static func acquire(directory: URL, role: Role) throws -> WindowLease? {
            guard role != .backgroundSelfTest else { throw SessionError("Background validation has no window lease") }
            try ensureDirectory(directory, privateMode: true)
            let path = directory.appendingPathComponent("\(role.rawValue).lock").path
            let descriptor = Darwin.open(path, O_CREAT | O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC, 0o600)
            guard descriptor >= 0 else { throw systemError("Cannot open application window lease") }
            var keepDescriptor = false
            defer { if !keepDescriptor { Darwin.close(descriptor) } }
            var metadata = stat()
            guard Darwin.fstat(descriptor, &metadata) == 0 else {
                throw systemError("Cannot inspect application window lease")
            }
            guard metadata.st_mode & S_IFMT == S_IFREG, metadata.st_uid == geteuid(),
                  metadata.st_mode & 0o077 == 0, metadata.st_nlink == 1, metadata.st_size == 0 else {
                throw SessionError("Application window lease is not a private empty regular file")
            }
            var lock = flock()
            lock.l_type = Int16(F_WRLCK)
            lock.l_whence = Int16(SEEK_SET)
            guard Darwin.fcntl(descriptor, F_SETLK, &lock) == 0 else {
                if errno == EACCES || errno == EAGAIN { return nil }
                throw systemError("Cannot acquire application window lease")
            }
            var published = stat()
            guard Darwin.lstat(path, &published) == 0 else {
                throw systemError("Cannot confirm application window lease identity")
            }
            guard published.st_dev == metadata.st_dev, published.st_ino == metadata.st_ino else {
                throw SessionError("Application window lease changed while acquiring it")
            }
            keepDescriptor = true
            return WindowLease(descriptor)
        }
    }

    private static let leaseTestFlag = "--application-session-lease-test-child"
    private static let leaseTestPrefix = ".mfb-session-lease-test-"

    /// Call before bootstrap/NSApplication construction. Only the isolated self-test directory is accepted.
    static func runLeaseTestChildIfRequested(arguments: [String] = CommandLine.arguments) -> Int32? {
        guard arguments.contains(leaseTestFlag) else { return nil }
        do {
            guard arguments.count == 4, arguments[1] == leaseTestFlag,
                  let role = Role(rawValue: arguments[3]), role != .backgroundSelfTest else {
                throw SessionError("Invalid application lease self-test arguments")
            }
            let directory = URL(fileURLWithPath: arguments[2], isDirectory: true)
            let temporary = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
            guard directory.path.hasPrefix("/"), directory.lastPathComponent.hasPrefix(leaseTestPrefix),
                  directory.deletingLastPathComponent().resolvingSymlinksInPath() == temporary else {
                throw SessionError("Application lease self-test requires its private temporary directory")
            }
            try ensureDirectory(directory, privateMode: true)
            guard let lease = try WindowLease.acquire(directory: directory, role: role) else { return 75 }
            return withExtendedLifetime(lease) {
                print("acquired")
                fflush(stdout)
                _ = readLine()
                return 0
            }
        } catch {
            fputs("Application lease self-test failed: \(error.localizedDescription)\n", stderr)
            return 70
        }
    }

    /// Real separate processes prove both role limits and kernel release after SIGKILL.
    static func selfTest(executable: URL) throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("\(leaseTestPrefix)\(UUID().uuidString)", isDirectory: true)
        try ensureDirectory(directory, privateMode: true, allowExisting: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        var children: [Process] = []
        var inputs: [Pipe] = []
        defer {
            for input in inputs { try? input.fileHandleForWriting.close() }
            for child in children {
                if child.isRunning { child.terminate() }
                child.waitUntilExit()
            }
        }
        func start(_ role: Role, acquired: Bool) throws -> Process {
            let child = Process()
            child.executableURL = executable
            child.arguments = [leaseTestFlag, directory.path, role.rawValue]
            let input = Pipe()
            let output = Pipe()
            child.standardInput = input
            child.standardOutput = output
            try child.run()
            children.append(child)
            inputs.append(input)
            // A broken child-entry dispatch must fail this synthetic test instead of leaving a GUI process behind.
            let watchdog = DispatchWorkItem { if child.isRunning { child.terminate() } }
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 30, execute: watchdog)
            defer { watchdog.cancel() }
            if acquired {
                let expected = Data("acquired\n".utf8)
                var message = Data()
                while message.count < expected.count {
                    guard let chunk = try output.fileHandleForReading.read(upToCount: expected.count - message.count),
                          !chunk.isEmpty else { break }
                    message.append(chunk)
                }
                guard message == expected, child.isRunning else {
                    throw SessionError("Application role lease was not acquired by its child process")
                }
            } else {
                child.waitUntilExit()
                guard child.terminationReason == .exit, child.terminationStatus == 75 else {
                    throw SessionError("An occupied application role was not rejected")
                }
            }
            return child
        }
        let production = try start(.production, acquired: true)
        _ = try start(.production, acquired: false)
        _ = try start(.debug, acquired: true)
        _ = try start(.debug, acquired: false)
        _ = try start(.production, acquired: false)
        guard Darwin.kill(production.processIdentifier, SIGKILL) == 0 else {
            throw systemError("Cannot terminate the isolated lease owner")
        }
        production.waitUntilExit()
        guard production.terminationReason == .uncaughtSignal, production.terminationStatus == SIGKILL else {
            throw SessionError("Application lease owner did not exit through SIGKILL")
        }
        _ = try start(.production, acquired: true)
        _ = try start(.debug, acquired: false)
    }
}
