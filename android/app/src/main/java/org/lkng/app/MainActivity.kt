package org.lkng.app

import android.content.Intent
import android.os.Bundle
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.appcompat.app.AppCompatActivity

/**
 * Hosts the LKNG web UI and starts the node service.
 *
 * The UI is the same Dioxus/WASM app that runs in a desktop browser — one
 * implementation of the crypto and the grid rules, not two. It talks to
 * the node over loopback exactly as it would when served by the node.
 */
class MainActivity : AppCompatActivity() {

    private lateinit var web: WebView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        startForegroundService(Intent(this, NodeService::class.java))

        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            // No file or content access: the UI needs neither, and both
            // are routes out of the WebView sandbox if the page is ever
            // tricked into loading something hostile.
            settings.allowFileAccess = false
            settings.allowContentAccess = false

            // The UI ships over Freenet at a URL that never changes while
            // its content does, so HTTP caching pins users to whatever
            // version they first loaded — including past a security fix.
            // The content comes from a node on loopback, so there is
            // nothing to save by caching it.
            settings.cacheMode = android.webkit.WebSettings.LOAD_NO_CACHE
            // Never in release. Left explicit rather than defaulted so the
            // decision is visible in review.
            WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)

            // Keystore-sealed secret storage for the UI. Only reachable
            // from our own local content — see shouldOverrideUrlLoading,
            // which pins navigation to the node's loopback origin.
            addJavascriptInterface(KeyVault(this@MainActivity), "LkngVault")

            webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(
                    view: WebView?, request: WebResourceRequest?
                ): Boolean {
                    // Keep the WebView pinned to local content. Anything
                    // else — including a link in a stranger's headline —
                    // must not navigate the app's own surface.
                    val host = request?.url?.host ?: return true
                    return !(host == "127.0.0.1" || host == "localhost" || host == "appassets.androidplatform.net")
                }
            }
        }
        // Self-test: seal and unseal a known value from Kotlin, with no
        // JavaScript involved. This separates "the Keystore code is
        // broken" from "the JS bridge never ran", which otherwise look
        // identical from outside (no vault file either way).
        if (BuildConfig.DEBUG) {
            val vault = KeyVault(this)
            val probe = android.util.Base64.encodeToString(ByteArray(32) { 0x5A }, android.util.Base64.NO_WRAP)
            val stored = vault.put("selftest.probe", probe)
            val read = vault.get("selftest.probe")
            android.util.Log.i(
                "lkng.vault",
                "selftest stored=$stored roundtrip=${read == probe} sealed=${vault.isSealed()}"
            )
        }

        web.clearCache(true)
        setContentView(web)
        web.loadUrl("http://127.0.0.1:${NodeService.WS_PORT}/v1/contract/web/$UI_CONTRACT/")
    }

    override fun onDestroy() {
        web.destroy()
        super.onDestroy()
    }

    companion object {
        /**
         * The published web-container contract holding the UI.
         *
         * Serving the interface from Freenet rather than from the APK is
         * what lets the UI update without a store review — and makes it
         * un-takedownable, which for this audience is a safety property
         * rather than a convenience.
         */
        const val UI_CONTRACT = "H477C5kQMNhXDS3H7rfDujjf3fVUghTNm7VHiyFh5ewn"
    }
}
