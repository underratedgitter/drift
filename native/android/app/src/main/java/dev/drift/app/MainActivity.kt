package dev.drift.app

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import org.json.JSONObject
import java.io.File
import java.net.Inet4Address

class MainActivity : Activity() {
    private lateinit var view: WebView
    private lateinit var local: Uri
    private var chooser: ValueCallback<Array<Uri>>? = null
    private var pendingInvite: String? = null
    private var exportFile: File? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var engineStarted = false
    private var pageReady = false

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        try {
            val response = JSONObject(NativeBridge.start(File(filesDir, "engine").absolutePath, Build.MODEL.take(60), localAddress()))
            check(response.getBoolean("ok")) { response.optString("error", "Engine failed") }
            engineStarted = true
            val url = response.getJSONObject("result").getString("url")
            local = Uri.parse(url)
            check(local.scheme == "http" && local.host == "127.0.0.1" && local.port > 0)
            pendingInvite = intent.dataString
            view = WebView(this)
            view.setBackgroundColor(android.graphics.Color.BLACK)
            view.settings.javaScriptEnabled = true
            view.settings.domStorageEnabled = true
            view.settings.allowFileAccess = false
            view.settings.allowContentAccess = true
            view.settings.setSupportMultipleWindows(false)
            view.addJavascriptInterface(SaveBridge(), "DriftNative")
            view.webViewClient = object : WebViewClient() {
                override fun onPageStarted(webView: WebView, url: String, icon: android.graphics.Bitmap?) { pageReady = false }
                override fun shouldOverrideUrlLoading(webView: WebView, request: WebResourceRequest): Boolean {
                    val uri = request.url
                    return uri.scheme != "http" || uri.host != "127.0.0.1" || uri.port != local.port
                }
                override fun onPageFinished(webView: WebView, url: String) {
                    if (Uri.parse(url).host != "127.0.0.1") return
                    pageReady = true
                    webView.evaluateJavascript("window.driftMobile={saveReceived:(id,index)=>DriftNative.saveReceived(id,index)};", null)
                    deliverInvite()
                }
            }
            view.webChromeClient = object : WebChromeClient() {
                override fun onShowFileChooser(webView: WebView, callback: ValueCallback<Array<Uri>>, params: FileChooserParams): Boolean {
                    chooser?.onReceiveValue(null); chooser = callback
                    return try { startActivityForResult(params.createIntent().apply { addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION) }, 10); true }
                    catch (_: Exception) { chooser = null; callback.onReceiveValue(null); false }
                }
            }
            setContentView(view)
            view.loadUrl(url)
        } catch (error: Throwable) { AlertDialog.Builder(this).setTitle("Drift could not start").setMessage(error.message).setPositiveButton("Close") { _, _ -> finish() }.show() }
    }
    override fun onResume() {
        super.onResume()
        try { val wifi = applicationContext.getSystemService(WIFI_SERVICE) as WifiManager; multicast = wifi.createMulticastLock("drift-discovery").apply { setReferenceCounted(false); acquire() } } catch (_: Exception) { /* Manual IP/code pairing still works if multicast is unavailable. */ }
    }
    // Android can restrict libc interface enumeration for ordinary apps. Query
    // LinkProperties using the existing network-state permission instead; never
    // substitute a cellular or VPN address for a local transfer link.
    private fun localAddress(): String = runCatching {
        val manager = getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
        manager.allNetworks.asSequence().filter { network ->
            val capabilities = manager.getNetworkCapabilities(network)
            capabilities != null && !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
                (capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) || capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))
        }.flatMap { manager.getLinkProperties(it)?.linkAddresses.orEmpty().asSequence() }
            .map { it.address }.filterIsInstance<Inet4Address>()
            .firstOrNull { it.isSiteLocalAddress || it.isLinkLocalAddress }?.hostAddress.orEmpty()
    }.getOrDefault("")
    override fun onPause() { multicast?.let { if (it.isHeld) it.release() }; multicast = null; super.onPause() }
    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); setIntent(intent); pendingInvite = intent.dataString; deliverInvite() }
    private fun deliverInvite() {
        if (!pageReady) return
        val value = pendingInvite ?: return
        pendingInvite = null
        if (!value.startsWith("drift://pair?") || value.length > 8192 || !::view.isInitialized) return
        view.evaluateJavascript("window.__driftInvitation=${JSONObject.quote(value)};window.dispatchEvent(new Event('drift-invitation'));", null)
    }
    private fun result(ok: Boolean, error: String = "") {
        val detail = JSONObject().put("ok", ok).put("error", error)
        runOnUiThread { if (!isDestroyed && ::view.isInitialized) view.evaluateJavascript("window.dispatchEvent(new CustomEvent('drift-save-result',{detail:$detail}));", null) }
    }
    inner class SaveBridge {
        @JavascriptInterface fun saveReceived(id: String, index: Int) {
            runOnUiThread {
                try {
                    check(exportFile == null) { "Finish the current save operation first" }
                    val reply = JSONObject(NativeBridge.exportPath(id, index)); check(reply.getBoolean("ok")) { reply.optString("error") }
                    val data = reply.getJSONObject("result"); exportFile = File(data.getString("path"))
                    startActivityForResult(Intent(Intent.ACTION_CREATE_DOCUMENT).apply { addCategory(Intent.CATEGORY_OPENABLE); type = "application/octet-stream"; putExtra(Intent.EXTRA_TITLE, data.getString("name")) }, 11)
                } catch (error: Exception) { exportFile = null; result(false, error.message ?: "Could not save file") }
            }
        }
    }
    @Deprecated("Platform Activity result API retained to avoid a large UI dependency")
    override fun onActivityResult(request: Int, code: Int, intent: Intent?) {
        super.onActivityResult(request, code, intent)
        if (request == 10) { chooser?.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(code, intent)); chooser = null }
        if (request == 11) {
            val source = exportFile; exportFile = null
            val destination = intent?.data
            if (code != RESULT_OK || source == null || destination == null) return
            Thread {
                try { source.inputStream().use { input -> contentResolver.openOutputStream(destination, "w").use { output -> checkNotNull(output); input.copyTo(output, 64 * 1024) } }; result(true) }
                catch (error: Exception) { result(false, error.message ?: "File save failed") }
            }.start()
        }
    }
    override fun onDestroy() { chooser?.onReceiveValue(null); if (::view.isInitialized) { view.removeJavascriptInterface("DriftNative"); view.destroy() }; if (isFinishing && engineStarted) NativeBridge.stop(); super.onDestroy() }
}
