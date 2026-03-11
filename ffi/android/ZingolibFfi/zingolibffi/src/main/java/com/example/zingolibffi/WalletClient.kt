package com.example.zingolibffi

import ZingolibFfi.BalanceSnapshot as FfiBalanceSnapshot
import ZingolibFfi.Chain as FfiChain
import ZingolibFfi.Performance as FfiPerformance
import ZingolibFfi.RestoreParams as FfiRestoreParams
import ZingolibFfi.SeedPhrase as FfiSeedPhrase
import ZingolibFfi.UfvkImportParams as FfiUfvkImportParams
import ZingolibFfi.WalletEngine as FfiWalletEngine
import ZingolibFfi.WalletEvent as FfiWalletEvent
import ZingolibFfi.WalletException as FfiWalletException
import ZingolibFfi.WalletListener as FfiWalletListener
import ZingolibFfi.uniffiEnsureInitialized
import java.io.Closeable

/**
 * Consumer-facing Kotlin wrapper over the UniFFI-generated WalletEngine API.
 *
 * Goals:
 * - Hide generated/backticked names
 * - Expose Kotlin-friendly request/response/event types
 * - Preserve the underlying lifecycle and error semantics
 * - Keep usage simple for app developers
 */
class WalletEngine private constructor(
    private val delegate: FfiWalletEngine
) : Closeable {

    companion object {
        /**
         * Creates a new wallet engine and ensures UniFFI/native bindings are initialized.
         */
        fun create(): WalletEngine {
            uniffiEnsureInitialized()
            return WalletEngine(FfiWalletEngine())
        }
    }

    /**
     * Initializes a new wallet.
     */
    @Throws(WalletEngineException::class)
    fun initNew(
        indexerUri: String,
        chain: Chain,
        performance: Performance,
        minConfirmations: UInt = 1u,
    ) {
        require(indexerUri.isNotBlank()) { "indexerUri cannot be blank" }
        require(minConfirmations >= 1u) { "minConfirmations must be >= 1" }

        runCatchingFfi {
            delegate.initNew(
                indexerUri,
                chain.toFfi(),
                performance.toFfi(),
                minConfirmations,
            )
        }
    }

    /**
     * Restores a wallet from seed phrase.
     */
    @Throws(WalletEngineException::class)
    fun initFromSeed(params: RestoreFromSeedRequest) {
        require(params.seedPhrase.isNotBlank()) { "seedPhrase cannot be blank" }
        require(params.indexerUri.isNotBlank()) { "indexerUri cannot be blank" }
        require(params.minConfirmations >= 1u) { "minConfirmations must be >= 1" }

        runCatchingFfi {
            delegate.initFromSeed(
                FfiRestoreParams(
                    seedPhrase = FfiSeedPhrase(params.seedPhrase),
                    birthday = params.birthday,
                    indexerUri = params.indexerUri,
                    chain = params.chain.toFfi(),
                    perf = params.performance.toFfi(),
                    minconf = params.minConfirmations,
                )
            )
        }
    }

    /**
     * Imports a wallet from UFVK.
     */
    @Throws(WalletEngineException::class)
    fun initFromUfvk(params: ImportUfvkRequest) {
        require(params.ufvk.isNotBlank()) { "ufvk cannot be blank" }
        require(params.indexerUri.isNotBlank()) { "indexerUri cannot be blank" }
        require(params.minConfirmations >= 1u) { "minConfirmations must be >= 1" }

        runCatchingFfi {
            delegate.initFromUfvk(
                FfiUfvkImportParams(
                    ufvk = params.ufvk,
                    birthday = params.birthday,
                    indexerUri = params.indexerUri,
                    chain = params.chain.toFfi(),
                    perf = params.performance.toFfi(),
                    minconf = params.minConfirmations,
                    walletDir = "./"
                )
            )
        }
    }

    /**
     * Returns the latest known balance snapshot.
     */
    @Throws(WalletEngineException::class)
    fun getBalanceSnapshot(): BalanceSnapshot =
        runCatchingFfi {
            delegate.getBalanceSnapshot().toDomain()
        }

    /**
     * Returns the latest known network height.
     */
    @Throws(WalletEngineException::class)
    fun getNetworkHeight(): UInt =
        runCatchingFfi {
            delegate.getNetworkHeight()
        }

    /**
     * Starts one manual sync round.
     */
    @Throws(WalletEngineException::class)
    fun startSync() {
        runCatchingFfi {
            delegate.startSync()
        }
    }

    /**
     * Requests a best-effort pause of the current sync.
     */
    @Throws(WalletEngineException::class)
    fun pauseSync() {
        runCatchingFfi {
            delegate.pauseSync()
        }
    }

    /**
     * Installs a listener for async engine events.
     *
     * The callback may be invoked from a non-main thread.
     */
    @Throws(WalletEngineException::class)
    fun setListener(listener: (WalletEvent) -> Unit) {
        runCatchingFfi {
            delegate.setListener(
                object : FfiWalletListener {
                    override fun onEvent(event: FfiWalletEvent) {
                        listener(event.toDomain())
                    }
                }
            )
        }
    }

    /**
     * Clears the installed listener, if any.
     */
    @Throws(WalletEngineException::class)
    fun clearListener() {
        runCatchingFfi {
            delegate.clearListener()
        }
    }

    /**
     * Shuts down the engine thread.
     */
    @Throws(WalletEngineException::class)
    fun shutdown() {
        runCatchingFfi {
            delegate.shutdown()
        }
    }

    /**
     * Shuts down the engine and unloads the wallet from memory.
     */
    @Throws(WalletEngineException::class)
    fun unloadWallet() {
        runCatchingFfi {
            delegate.unloadWallet()
        }
    }

    /**
     * Releases the underlying UniFFI object.
     *
     * This does not implicitly call shutdown/unloadWallet.
     * If you want graceful shutdown semantics, call those explicitly first.
     */
    override fun close() {
        delegate.close()
    }
}

/* =========================
 * Public domain models
 * ========================= */

data class RestoreFromSeedRequest(
    val seedPhrase: String,
    val birthday: UInt,
    val indexerUri: String,
    val chain: Chain,
    val performance: Performance,
    val minConfirmations: UInt = 1u,
)

data class ImportUfvkRequest(
    val ufvk: String,
    val birthday: UInt,
    val indexerUri: String,
    val chain: Chain,
    val performance: Performance,
    val minConfirmations: UInt = 1u,
)

data class BalanceSnapshot(
    val confirmed: String,
    val total: String,
)

enum class Chain {
    MAINNET,
    TESTNET,
    REGTEST,
}

enum class Performance {
    MAXIMUM,
    HIGH,
    MEDIUM,
    LOW,
}

sealed interface WalletEvent {
    data object EngineReady : WalletEvent
    data object SyncStarted : WalletEvent
    data class SyncProgress(
        val walletHeight: UInt,
        val networkHeight: UInt,
        val percent: Float,
    ) : WalletEvent
    data object SyncPaused : WalletEvent
    data object SyncFinished : WalletEvent
    data class BalanceChanged(val snapshot: BalanceSnapshot) : WalletEvent
    data class Error(val code: String, val message: String) : WalletEvent
    data object Unloaded : WalletEvent
}

sealed class WalletEngineException(message: String? = null, cause: Throwable? = null) :
    RuntimeException(message, cause) {

    data object CommandQueueClosed : WalletEngineException("Command queue is closed")
    data object ListenerLockPoisoned : WalletEngineException("Listener lock poisoned")
    data object NotInitialized : WalletEngineException("Wallet engine is not initialized")
    data class Internal(val details: String) : WalletEngineException(details)
    data class Unexpected(val original: Throwable) :
        WalletEngineException(original.message ?: "Unexpected wallet engine error", original)
}

/* =========================
 * Mapping helpers
 * ========================= */

private fun Chain.toFfi(): FfiChain =
    when (this) {
        Chain.MAINNET -> FfiChain.MAINNET
        Chain.TESTNET -> FfiChain.TESTNET
        Chain.REGTEST -> FfiChain.REGTEST
    }

private fun Performance.toFfi(): FfiPerformance =
    when (this) {
        Performance.MAXIMUM -> FfiPerformance.MAXIMUM
        Performance.HIGH -> FfiPerformance.HIGH
        Performance.MEDIUM -> FfiPerformance.MEDIUM
        Performance.LOW -> FfiPerformance.LOW
    }

private fun FfiBalanceSnapshot.toDomain(): BalanceSnapshot =
    BalanceSnapshot(
        confirmed = confirmed,
        total = total,
    )

private fun FfiWalletEvent.toDomain(): WalletEvent =
    when (this) {
        is FfiWalletEvent.EngineReady -> WalletEvent.EngineReady
        is FfiWalletEvent.SyncStarted -> WalletEvent.SyncStarted
        is FfiWalletEvent.SyncProgress -> WalletEvent.SyncProgress(
            walletHeight = walletHeight,
            networkHeight = networkHeight,
            percent = percent,
        )
        is FfiWalletEvent.SyncPaused -> WalletEvent.SyncPaused
        is FfiWalletEvent.SyncFinished -> WalletEvent.SyncFinished
        is FfiWalletEvent.BalanceChanged -> WalletEvent.BalanceChanged(
            snapshot = v1.toDomain()
        )
        is FfiWalletEvent.Error -> WalletEvent.Error(
            code = code,
            message = message,
        )
        is FfiWalletEvent.Unloaded -> WalletEvent.Unloaded
    }

private fun Throwable.toDomainException(): WalletEngineException =
    when (this) {
        is WalletEngineException -> this
        is FfiWalletException.CommandQueueClosed -> WalletEngineException.CommandQueueClosed
        is FfiWalletException.ListenerLockPoisoned -> WalletEngineException.ListenerLockPoisoned
        is FfiWalletException.NotInitialized -> WalletEngineException.NotInitialized
        is FfiWalletException.Internal -> WalletEngineException.Internal(v1)
        else -> WalletEngineException.Unexpected(this)
    }

private inline fun <T> runCatchingFfi(block: () -> T): T {
    try {
        return block()
    } catch (t: Throwable) {
        throw t.toDomainException()
    }
}

/* =========================
 * Optional convenience APIs
 * ========================= */

/**
 * Convenience helper for scoped engine usage.
 */
inline fun <R> withWalletEngine(block: (WalletEngine) -> R): R {
    WalletEngine.create().use { engine ->
        return block(engine)
    }
}
