package com.zingolabs.wallet

import ZingolibFfi.WalletEngine
import ZingolibFfi.WalletException
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertNotNull
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class WalletEngineLoadTest {

    @Test
    fun testRunner_works() {
        assert(true)
    }

    @Test
    fun walletEngine_classLoads() {
        val clazz = WalletEngine::class.java
        assertNotNull(clazz)
    }

    @Test
    fun walletEngine_canBeConstructed() {
        val engine = WalletEngine()
        try {
            assertNotNull(engine)
        } finally {
            engine.close()
        }
    }

    @Test
    fun getBalanceSnapshot_beforeInit_throwsExpectedError() {
        val engine = WalletEngine()
        try {
            try {
                engine.getBalanceSnapshot()
                fail("Expected WalletException.NotInitialized")
            } catch (_: WalletException.NotInitialized) {
                // expected
            }
        } finally {
            engine.close()
        }
    }

    @Test
    fun getNetworkHeight_beforeInit_throwsExpectedError() {
        val engine = WalletEngine()
        try {
            try {
                engine.getNetworkHeight()
                fail("Expected WalletException.NotInitialized")
            } catch (_: WalletException.NotInitialized) {
                // expected
            }
        } finally {
            engine.close()
        }
    }

    @Test
    fun startSync_beforeInit_doesNotCrashTestProcess() {
        val engine = WalletEngine()
        try {
            engine.startSync()
        } finally {
            engine.close()
        }
    }
}
