package org.lkng.app

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationManager
import android.webkit.JavascriptInterface
import androidx.core.content.ContextCompat

/**
 * Coarse location for the UI.
 *
 * ## Why this exists rather than the WebView's own geolocation
 *
 * The browser Geolocation API would hand the page raw latitude and
 * longitude at whatever precision the OS offers. There is no reason for
 * the web layer to ever hold that: it converts position to a ~5 km cell
 * and throws the rest away, so the precision would exist only as something
 * that could leak — through a bug, an XSS, or a future careless feature.
 *
 * So the *native* side asks for **coarse** location only, and the bridge
 * exposes exactly one thing: a jittered, cell-resolution answer. The
 * grid literally cannot render a distance, because nothing in the app ever
 * knows one.
 *
 * ## Why `PRIORITY_BALANCED`-style coarse, and no background access
 *
 * `ACCESS_COARSE_LOCATION` alone is requested — never `FINE`. Android
 * fuzzes coarse location to roughly a 1–2 km grid before the app sees it,
 * which composes with the app's own stable jitter rather than replacing
 * it. Background location is never requested: presence is published while
 * the user has the app open, and an app in this category asking to track
 * you when closed would deserve the suspicion it got.
 */
class Locator(private val context: Context) {

    /** True when the user has granted coarse location. */
    @JavascriptInterface
    fun hasPermission(): Boolean =
        ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_COARSE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED

    /**
     * Last known coarse position as `"lat,lon"`, or null.
     *
     * Deliberately uses *last known* rather than requesting a fresh fix:
     * a live fix costs battery and buys precision the app immediately
     * discards. A position from some minutes ago is indistinguishable
     * after quantisation to a 5 km cell.
     */
    @JavascriptInterface
    fun lastKnown(): String? {
        if (!hasPermission()) return null
        val lm = context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager ?: return null

        val best: Location? = try {
            // NETWORK first: it is the coarse provider, and GPS would give
            // precision that is thrown away anyway.
            listOf(LocationManager.NETWORK_PROVIDER, LocationManager.PASSIVE_PROVIDER)
                .mapNotNull { p -> runCatching { lm.getLastKnownLocation(p) }.getOrNull() }
                .maxByOrNull { it.time }
        } catch (e: SecurityException) {
            null
        }

        return best?.let { "${it.latitude},${it.longitude}" }
    }
}
