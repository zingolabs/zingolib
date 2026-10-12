// Golden wire-format contract tests, Swift side.
//
// Asserts the generated Swift converters against the SAME pinned hex files
// the Rust suite (tests/golden_wire.rs) and the Kotlin suite verify, so all
// three languages enforce one wire contract at test time. The canonical
// values below are shared verbatim across the three suites; the pins under
// test-data/golden/ are never-regenerate contract artifacts.
//
// Execution: compiles against the generated Swift bindings in an XCTest
// bundle; the Mac-gated step-7 packaging (#2504) wires it in. The golden
// directory resolves relative to this file, or set ZINGO_GOLDEN_DIR.

import Foundation
import XCTest

final class GoldenWireContractTests: XCTestCase {
    private func goldenDir() -> URL {
        if let env = ProcessInfo.processInfo.environment["ZINGO_GOLDEN_DIR"] {
            return URL(fileURLWithPath: env)
        }
        return URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // swift/
            .deletingLastPathComponent() // contract-tests/
            .appendingPathComponent("test-data/golden")
    }

    private func golden(_ name: String) throws -> [UInt8] {
        let url = goldenDir().appendingPathComponent("\(name).hex")
        let hex = try String(contentsOf: url, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        XCTAssertEqual(hex.count % 2, 0, "golden hex has odd length")
        var bytes = [UInt8]()
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            guard let byte = UInt8(hex[index..<next], radix: 16) else {
                XCTFail("golden must be pure hex")
                return []
            }
            bytes.append(byte)
            index = next
        }
        return bytes
    }

    private func toHex(_ bytes: [UInt8]) -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }

    /// Lift the whole pin, asserting nothing trails.
    private func readAll<T>(
        _ bytes: [UInt8],
        _ read: (inout (data: Data, offset: Data.Index)) throws -> T
    ) throws -> T {
        var reader: (data: Data, offset: Data.Index) = (data: Data(bytes), offset: 0)
        let value = try read(&reader)
        XCTAssertEqual(reader.offset, reader.data.count, "lifting left trailing bytes unread")
        return value
    }

    private func assertEndpointPin(_ name: String, port: UInt16) throws {
        let value = Socks5Endpoint(host: "127.0.0.1", port: port)
        var writer = [UInt8]()
        FfiConverterTypeSocks5Endpoint.write(value, into: &writer)
        XCTAssertEqual(
            toHex(writer), toHex(try golden(name)),
            "\(name): lowering drifted from the pinned wire bytes"
        )
        let lifted = try readAll(try golden(name)) {
            try FfiConverterTypeSocks5Endpoint.read(from: &$0)
        }
        XCTAssertEqual(lifted, value, "\(name): lifting the pin changed the value")
    }

    func testSocks5EndpointWireEncodingMatchesThePins() throws {
        try assertEndpointPin("socks5_endpoint", port: 43210)
        try assertEndpointPin("socks5_endpoint_port_min", port: 0)
        try assertEndpointPin("socks5_endpoint_port_max", port: 65535)
    }

    func testEveryDeathReasonWireEncodingMatchesItsPin() throws {
        let cases: [(String, ProxyDeathReason)] = [
            ("proxy_death_reason_listener_refused",
             .listenerRefused(detail: "gateway went away — упал — 途絶")),
            ("proxy_death_reason_greeting_unwritable",
             .greetingUnwritable(detail: "broken pipe")),
            ("proxy_death_reason_method_selection_unreadable",
             .methodSelectionUnreadable(detail: "connection reset")),
            ("proxy_death_reason_method_selection_refused",
             .methodSelectionRefused(version: 0x05, method: 0xff)),
            ("proxy_death_reason_check_timed_out",
             .checkTimedOut(budgetMillis: 250)),
        ]
        for (name, value) in cases {
            var writer = [UInt8]()
            FfiConverterTypeProxyDeathReason.write(value, into: &writer)
            XCTAssertEqual(toHex(writer), toHex(try golden(name)), "\(name): lowering drifted")
            let lifted = try readAll(try golden(name)) {
                try FfiConverterTypeProxyDeathReason.read(from: &$0)
            }
            XCTAssertEqual(lifted, value, "\(name): lifting the pin changed the value")
        }
    }

    func testEveryErrorVariantWireEncodingMatchesItsPin() throws {
        let cases: [(String, ProxyFfiError)] = [
            ("proxy_ffi_error_runtime", .Runtime(reason: "no threads")),
            ("proxy_ffi_error_connect", .Connect(reason: "gateway refused")),
        ]
        for (name, value) in cases {
            var writer = [UInt8]()
            FfiConverterTypeProxyFfiError.write(value, into: &writer)
            XCTAssertEqual(toHex(writer), toHex(try golden(name)), "\(name): lowering drifted")
            let lifted = try readAll(try golden(name)) {
                try FfiConverterTypeProxyFfiError.read(from: &$0)
            }
            XCTAssertEqual(lifted, value, "\(name): lifting the pin changed the value")
        }
    }
}
