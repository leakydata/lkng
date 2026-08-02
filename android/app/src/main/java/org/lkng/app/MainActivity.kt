package org.lkng.app

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
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

    /** The chooser callback currently awaiting a result, if any. */
    private var pendingFile: ValueCallback<Array<Uri>>? = null

    /**
     * Result launcher for the system file picker.
     *
     * Registered as a field so it is created during `onCreate`, which the
     * Activity Result API requires — registering it lazily from inside the
     * chooser callback throws at runtime, and only when a user first taps
     * "add a photo".
     */
    private val filePicker = registerForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val cb = pendingFile
        pendingFile = null
        if (cb == null) return@registerForActivityResult

        // Cancelling must still answer the callback, with null. Returning
        // nothing at all is what wedges the input permanently.
        val uris: Array<Uri>? = if (result.resultCode == RESULT_OK) {
            val data = result.data
            val clip = data?.clipData
            when {
                clip != null -> Array(clip.itemCount) { i -> clip.getItemAt(i).uri }
                data?.data != null -> arrayOf(data.data!!)
                else -> null
            }
        } else {
            null
        }
        cb.onReceiveValue(uris)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        startForegroundService(Intent(this, NodeService::class.java))

        // Coarse location only, and only in the foreground. Asked for here
        // rather than at first use so the grid is not empty on first run
        // while a dialog waits behind it.
        if (androidx.core.content.ContextCompat.checkSelfPermission(
                this, android.Manifest.permission.ACCESS_COARSE_LOCATION
            ) != android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            androidx.core.app.ActivityCompat.requestPermissions(
                this, arrayOf(android.Manifest.permission.ACCESS_COARSE_LOCATION), REQ_LOCATION
            )
        }

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
            addJavascriptInterface(Locator(this@MainActivity), "LkngLocation")

            // Without this, `<input type="file">` does **nothing**. No
            // picker, no error, no console message — the tap is simply
            // swallowed, and every photo feature in the app is unreachable
            // on Android while working perfectly in a desktop browser.
            //
            // That is exactly what shipped: profile photos, multiple photos,
            // album photos and backup restore were all built, tested and
            // published, and none of them could be triggered on a phone.
            webChromeClient = object : WebChromeClient() {
                override fun onShowFileChooser(
                    view: WebView?,
                    callback: ValueCallback<Array<Uri>>?,
                    params: FileChooserParams?,
                ): Boolean {
                    // Any previous request must be answered before a new one
                    // starts. A ValueCallback left un-called leaves the
                    // WebView believing a chooser is still open, and every
                    // later tap on a file input is ignored for the life of
                    // the page — the failure looks identical to having no
                    // chooser at all, which makes it hard to tell apart from
                    // the bug this whole block fixes.
                    pendingFile?.onReceiveValue(null)
                    pendingFile = callback

                    return try {
                        val intent = params?.createIntent()
                            ?: Intent(Intent.ACTION_GET_CONTENT).apply {
                                type = "image/*"
                                addCategory(Intent.CATEGORY_OPENABLE)
                            }
                        // ACTION_GET_CONTENT via the system picker: it grants
                        // read access to exactly the file chosen and nothing
                        // else, so the app never needs a storage permission.
                        // Asking for READ_MEDIA_IMAGES to upload one photo
                        // would be requesting the whole gallery to read one
                        // file, from an app whose argument is that it takes
                        // only what it needs.
                        filePicker.launch(intent)
                        true
                    } catch (e: Exception) {
                        android.util.Log.e("lkng.ui", "no file picker available", e)
                        pendingFile?.onReceiveValue(null)
                        pendingFile = null
                        false
                    }
                }
            }

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
        // Startup assertion: prove the Keystore can seal and unseal before
        // the UI relies on it. Cheap, and it turns a silent security
        // regression (keys quietly falling back to web storage) into a
        // visible one.
        if (BuildConfig.DEBUG) {
            val vault = KeyVault(this)
            val probe = android.util.Base64.encodeToString(
                ByteArray(32) { 0x5A }, android.util.Base64.NO_WRAP
            )
            val ok = vault.put("selftest.probe", probe) && vault.get("selftest.probe") == probe
            android.util.Log.i("lkng.vault", "keystore selftest passed=$ok")
        }

        web.clearCache(true)
        setContentView(web)
        loadWhenNodeReady()
    }

    /**
     * Wait for the node's API to answer before loading the UI.
     *
     * The node takes tens of seconds to start and bind its port, but the
     * Activity used to call `loadUrl` immediately — so a cold start always
     * hit connection-refused and the user got `chrome-error://`. It
     * appeared to work only when a node happened to survive from a
     * previous run, which is exactly the case a developer sees and a new
     * user never does.
     *
     * Polls loopback rather than waiting a fixed time: a phone that is
     * slow, cold, or busy takes longer than any constant you would pick.
     */
    private fun loadWhenNodeReady(attempt: Int = 0) {
        val url = "http://127.0.0.1:${NodeService.WS_PORT}/v1/contract/web/$UI_CONTRACT/"
        Thread {
            val ready = try {
                (java.net.URL(url).openConnection() as java.net.HttpURLConnection).run {
                    connectTimeout = 3000
                    readTimeout = 5000
                    requestMethod = "HEAD"
                    val ok = responseCode in 200..399
                    disconnect()
                    ok
                }
            } catch (e: Exception) {
                false
            }

            runOnUiThread {
                when {
                    ready -> web.loadUrl(url)
                    attempt < MAX_STARTUP_ATTEMPTS -> {
                        web.loadDataWithBaseURL(
                            null, startingHtml(attempt), "text/html", "utf-8", null
                        )
                        web.postDelayed({ loadWhenNodeReady(attempt + 1) }, 2000)
                    }
                    else -> web.loadDataWithBaseURL(
                        null, failedHtml(), "text/html", "utf-8", null
                    )
                }
            }
        }.start()
    }

    /** Honest progress, not a spinner that says nothing. */
    private fun startingHtml(attempt: Int) = """
        <html><body style="background:#0d0e12;color:#eceef2;font:15px system-ui;
        display:flex;align-items:center;justify-content:center;height:100vh;margin:0">
        <div style="text-align:center;padding:24px">
          <div style="font-weight:700;letter-spacing:.14em;margin-bottom:14px">LKNG</div>
          <div style="color:#8b909c">Starting your node and joining the network…</div>
          <div style="color:#8b909c;font-size:12px;margin-top:10px">
            first run takes about a minute (${attempt * 2}s)
          </div>
        </div></body></html>
    """.trimIndent()

    private fun failedHtml() = """
        <html><body style="background:#0d0e12;color:#eceef2;font:15px system-ui;
        display:flex;align-items:center;justify-content:center;height:100vh;margin:0">
        <div style="text-align:center;padding:24px;max-width:32em">
          <div style="font-weight:700;letter-spacing:.14em;margin-bottom:14px">LKNG</div>
          <div style="color:#8b909c">Your node did not start. This is pre-alpha software —
          please reopen the app, and report it if it keeps happening.</div>
        </div></body></html>
    """.trimIndent()

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

        /** ~2 minutes of 2s polls. A cold node on a slow phone is slow. */
        const val MAX_STARTUP_ATTEMPTS = 60
        private const val REQ_LOCATION = 1001
    }
}
