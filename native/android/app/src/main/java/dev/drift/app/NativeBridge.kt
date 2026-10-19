package dev.drift.app

object NativeBridge {
    init { System.loadLibrary("drift_native") }
    @JvmStatic external fun start(directory: String, name: String, localHost: String): String
    @JvmStatic external fun exportPath(id: String, index: Int): String
    @JvmStatic external fun stop()
}
