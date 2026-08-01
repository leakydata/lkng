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
            // Never in release. Left explicit rather than defaulted so the
            // decision is visible in review.
            WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)

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
        const val UI_CONTRACT = "REPLACE_WITH_PUBLISHED_UI_CONTRACT_ID"
    }
}
