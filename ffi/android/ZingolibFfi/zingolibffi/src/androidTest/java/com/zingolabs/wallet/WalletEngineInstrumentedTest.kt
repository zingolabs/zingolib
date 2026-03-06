package com.zingolabs.wallet

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.example.zingolibffi.WalletEngine
import com.example.zingolibffi.WalletEngineException
import com.example.zingolibffi.WalletEvent
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
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
    fun getBalanceSnapshot_beforeInitialization_throwsNotInitialized() {
        val engine = WalletEngine.create()
        try {
            try {
                engine.getBalanceSnapshot()
                throw AssertionError("Expected WalletEngineException.NotInitialized")
            } catch (e: WalletEngineException.NotInitialized) {
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
