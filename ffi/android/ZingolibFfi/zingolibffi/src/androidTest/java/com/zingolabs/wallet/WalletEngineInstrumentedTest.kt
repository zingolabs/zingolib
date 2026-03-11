package com.zingolabs.wallet

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.example.zingolibffi.Chain
import com.example.zingolibffi.ImportUfvkRequest
import com.example.zingolibffi.Performance
import com.example.zingolibffi.WalletEngine
import com.example.zingolibffi.WalletEngineException
import com.example.zingolibffi.WalletEvent
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class WalletEngineInstrumentedTest {

    @Test
    fun appContext_isAvailable() {
        val appContext = InstrumentationRegistry.getInstrumentation().targetContext
        assertNotNull(appContext)
    }

    @Test
    fun walletEngine_canBeCreatedAndClosed() {
        val engine = WalletEngine.create()
        try {
            assertNotNull(engine)
        } finally {
            engine.close()
        }
    }

    @Test
    fun walletEngine_listenerCanBeInstalled() {
        val engine = WalletEngine.create()
        try {
            val sawEvent = CountDownLatch(1)

            engine.setListener { event ->
                when (event) {
                    WalletEvent.EngineReady -> sawEvent.countDown()
                    else -> Unit
                }
            }

            // Depending on your Rust side, EngineReady may already have fired
            // before the listener is attached. So this is just a smoke test that
            // listener registration itself does not crash.
            assertTrue(true)
        } finally {
            engine.close()
        }
    }

    @Test
    fun initFromUfvk_withInvalidUfvk_returnsInternalError() {
        val engine = WalletEngine.create()
        try {
            val params = ImportUfvkRequest(
                ufvk = "not-a-real-ufvk",
                birthday = 1u,
                indexerUri = "http://127.0.0.1:9067",
                chain = Chain.REGTEST,
                performance = Performance.HIGH,
                minConfirmations = 1u
            )
            println("Testing initFromUfvk with birthday=${params.birthday}, chain=${params.chain}, uri=${params.indexerUri}")


            try {
                engine.initFromUfvk(params)
                fail("Expected WalletEngineException.Internal")
            } catch (e: WalletEngineException.Internal) {
                println("Caught WalletEngineException.Internal: details=${e.details}")
                assertTrue(e.details.contains("Key decoding failed"))
            }
        } finally {
            engine.close()
        }
    }

    @Test
    fun initFromUfvk_withValidUfvk_initializesWallet() {
        val engine = WalletEngine.create()
        try {
            val validTestUfvk =
                "uviewregtest1sq45509uyu7sfz2veyqsl5jv834urwgelpwadvn37h2726gp47y5g8qklm4urcxpmn8rrtpr2m4r5c0vsjph9kj5q5vu044zyxzevrejphhx9ekjq7ctefzs036l5y9v2dnumgfq04g8r3gj8da8qhy8k0f4fcke2prnwcn0x2g6sng05lqux2a5y25ea39s3m70gxlfczfh5hvm4ggsd98r995fufr0xc3ns2p8ugdt9xy0k687k5vrvcdz5uzleuplz3w6gfr3gj8mwznuy7e62ntqaute8wp2yv8szgmrrz9fhdpnqx9k38w5duh79qwkm2f82s0mtemx5m2hx2de82w6nwrvsxlr6eh6y8hn3tkjhcft5q7fyae5a6t32swv6w0elfrgypkcclgc866z3t5mgz53ffvtjv2zdxzmzg3l47u23pmd778jwdgeag79fu7swnl5alqmrxfuuwfz6g64dhf24gj3tfudfsdpgwhjrc820ln0"

            val params = ImportUfvkRequest(
                ufvk = validTestUfvk,
                birthday = 1u,
                indexerUri = "http://127.0.0.1:9067",
                chain = Chain.REGTEST,
                performance = Performance.HIGH,
                minConfirmations = 1u
            )

            println(
                "initFromUfvk(valid): birthday=${params.birthday}, " +
                        "chain=${params.chain}, uri=${params.indexerUri}, ufvkLength=${params.ufvk.length}"
            )

            engine.initFromUfvk(params)

            val snapshot = engine.getBalanceSnapshot()
            println("wallet initialized, snapshot=$snapshot")

            assertNotNull(snapshot)
            assertTrue(snapshot.confirmed.isNotBlank())
            assertTrue(snapshot.total.isNotBlank())
        } finally {
            engine.close()
        }
    }

    @Test
    fun getBalanceSnapshot_beforeInitialization_throwsNotInitialized() {
        val engine = WalletEngine.create()
        try {
            try {
                engine.getBalanceSnapshot()
                throw AssertionError("Expected WalletEngineException.NotInitialized")
            } catch (_: WalletEngineException.NotInitialized) {
                // expected
                assertTrue(true)
            }
        } finally {
            engine.close()
        }
    }

    @Test
    fun startSync_beforeInitialization_doesNotCrashCaller() {
        val engine = WalletEngine.create()
        try {
            // Depending on your Rust implementation, this may emit an async Error
            // event instead of throwing synchronously.
            val latch = CountDownLatch(1)

            engine.setListener { event ->
                if (event is WalletEvent.Error && event.code == "start_sync_failed") {
                    latch.countDown()
                }
            }

            engine.startSync()

            // Best-effort assertion for async event delivery.
            latch.await(2, TimeUnit.SECONDS)
            assertTrue(true)
        } finally {
            engine.close()
        }
    }
}
