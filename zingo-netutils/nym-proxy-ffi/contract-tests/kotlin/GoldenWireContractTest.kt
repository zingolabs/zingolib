// Golden wire-format contract tests, Kotlin side.
//
// Asserts the generated Kotlin converters against the SAME pinned hex files
// the Rust suite (tests/golden_wire.rs) and the Swift suite verify, so all
// three languages enforce one wire contract at test time. The canonical
// values below are shared verbatim across the three suites; the pins under
// test-data/golden/ are never-regenerate contract artifacts.
//
// Execution: this file compiles against the generated bindings
// (uniffi.zingo_nym_proxy_ffi) and JUnit 4; zingo-mobile's test suite wires
// it in during step-4/step-3 packaging (#2513/#2505). Point
// -Dzingo.golden.dir at the crate's test-data/golden directory if the
// working directory is not the crate root.

package uniffi.zingo_nym_proxy_ffi.contract

import java.io.File
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zingo_nym_proxy_ffi.FfiConverterTypeProxyDeathReason
import uniffi.zingo_nym_proxy_ffi.FfiConverterTypeProxyFfiError
import uniffi.zingo_nym_proxy_ffi.FfiConverterTypeSocks5Endpoint
import uniffi.zingo_nym_proxy_ffi.ProxyDeathReason
import uniffi.zingo_nym_proxy_ffi.ProxyFfiException
import uniffi.zingo_nym_proxy_ffi.Socks5Endpoint

class GoldenWireContractTest {
    private val goldenDir = File(System.getProperty("zingo.golden.dir") ?: "test-data/golden")

    private fun golden(name: String): ByteArray {
        val hex = File(goldenDir, "$name.hex").readText().trim()
        require(hex.length % 2 == 0) { "golden hex has odd length" }
        return ByteArray(hex.length / 2) { i ->
            hex.substring(2 * i, 2 * i + 2).toInt(16).toByte()
        }
    }

    private fun toHex(bytes: ByteArray): String =
        bytes.joinToString("") { "%02x".format(it) }

    private fun readAll(bytes: ByteArray, read: (ByteBuffer) -> Any): Any {
        val buf = ByteBuffer.wrap(bytes)
        val value = read(buf)
        assertEquals("lifting left trailing bytes unread", 0, buf.remaining())
        return value
    }

    private fun endpointAt(port: UShort) = Socks5Endpoint("127.0.0.1", port)

    private fun endpointWire(value: Socks5Endpoint): ByteArray {
        val buf = ByteBuffer.allocate(FfiConverterTypeSocks5Endpoint.allocationSize(value).toInt())
        FfiConverterTypeSocks5Endpoint.write(value, buf)
        buf.flip()
        return ByteArray(buf.remaining()).also { buf.get(it) }
    }

    private fun assertEndpointPin(name: String, value: Socks5Endpoint) {
        assertEquals(
            "$name: lowering drifted from the pinned wire bytes",
            toHex(golden(name)),
            toHex(endpointWire(value)),
        )
        assertEquals(
            "$name: lifting the pin changed the value",
            value,
            readAll(golden(name)) { FfiConverterTypeSocks5Endpoint.read(it) },
        )
    }

    @Test
    fun socks5EndpointWireEncodingMatchesThePins() {
        assertEndpointPin("socks5_endpoint", endpointAt(43210u))
        assertEndpointPin("socks5_endpoint_port_min", endpointAt(0u))
        assertEndpointPin("socks5_endpoint_port_max", endpointAt(65535u))
    }

    @Test
    fun everyDeathReasonWireEncodingMatchesItsPin() {
        val cases: List<Pair<String, ProxyDeathReason>> = listOf(
            "proxy_death_reason_listener_refused" to
                ProxyDeathReason.ListenerRefused("gateway went away — упал — 途絶"),
            "proxy_death_reason_greeting_unwritable" to
                ProxyDeathReason.GreetingUnwritable("broken pipe"),
            "proxy_death_reason_method_selection_unreadable" to
                ProxyDeathReason.MethodSelectionUnreadable("connection reset"),
            "proxy_death_reason_method_selection_refused" to
                ProxyDeathReason.MethodSelectionRefused(0x05.toUByte(), 0xff.toUByte()),
            "proxy_death_reason_check_timed_out" to
                ProxyDeathReason.CheckTimedOut(250.toULong()),
        )
        for ((name, expected) in cases) {
            val buf = ByteBuffer.allocate(
                FfiConverterTypeProxyDeathReason.allocationSize(expected).toInt(),
            )
            FfiConverterTypeProxyDeathReason.write(expected, buf)
            buf.flip()
            val wire = ByteArray(buf.remaining()).also { buf.get(it) }
            assertEquals("$name: lowering drifted", toHex(golden(name)), toHex(wire))
            assertEquals(
                "$name: lifting the pin changed the value",
                expected,
                readAll(golden(name)) { FfiConverterTypeProxyDeathReason.read(it) },
            )
        }
    }

    @Test
    fun everyErrorVariantWireEncodingMatchesItsPin() {
        // The generated exception classes are not data classes, so equality is
        // variant class plus the reason payload.
        val cases: List<Triple<String, Class<out ProxyFfiException>, String>> = listOf(
            Triple("proxy_ffi_error_runtime", ProxyFfiException.Runtime::class.java, "no threads"),
            Triple("proxy_ffi_error_connect", ProxyFfiException.Connect::class.java, "gateway refused"),
        )
        for ((name, variantClass, reason) in cases) {
            val lifted = readAll(golden(name)) { FfiConverterTypeProxyFfiError.read(it) }
            assertEquals("$name: variant drifted", variantClass, lifted.javaClass)
            val liftedReason = when (lifted) {
                is ProxyFfiException.Runtime -> lifted.reason
                is ProxyFfiException.Connect -> lifted.reason
                else -> error("unreachable: variant class already asserted")
            }
            assertEquals("$name: reason payload drifted", reason, liftedReason)
            val roundTrip = ByteBuffer.allocate(
                FfiConverterTypeProxyFfiError.allocationSize(lifted as ProxyFfiException).toInt(),
            )
            FfiConverterTypeProxyFfiError.write(lifted, roundTrip)
            roundTrip.flip()
            val wire = ByteArray(roundTrip.remaining()).also { roundTrip.get(it) }
            assertEquals("$name: lowering drifted", toHex(golden(name)), toHex(wire))
            assertTrue("$name: reason must be non-empty", liftedReason.isNotEmpty())
        }
    }
}
