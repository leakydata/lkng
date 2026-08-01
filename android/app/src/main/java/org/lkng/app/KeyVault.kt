package org.lkng.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.webkit.JavascriptInterface
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Keystore-sealed storage for the identity seed.
 *
 * ## Why this exists
 *
 * The web UI previously kept the 32-byte identity seed in `localStorage`.
 * That seed *is* the account: it derives the signing key, every per-epoch
 * subkey, the encryption key, and the recovery bundle. Anything that can
 * run script in the WebView could read it, and for this app's users a
 * stolen identity key is not an inconvenience — it is someone else able to
 * be them, permanently, with no server that can revoke anything.
 *
 * ## What this actually protects against
 *
 * The seed is sealed with an AES-GCM key held in the **Android Keystore**
 * and never exported. On most modern devices that key is hardware-backed,
 * but we do not claim so — see [`isSealed`]. The ciphertext is what gets
 * written to disk, so:
 *
 * * an attacker who copies app storage (a backup, a rooted pull, a stolen
 *   unlocked device's files) gets ciphertext they cannot open elsewhere;
 * * the key cannot be extracted even by this app — it can only ask the
 *   Keystore to use it.
 *
 * What it does **not** protect against, stated plainly: script running
 * inside our own WebView can still call `unseal()` and get the seed, since
 * the app has to hand it to the crypto that lives in WASM. Closing that
 * would mean moving all signing behind the bridge so the seed never
 * crosses it — the right end state, and a much larger change. This is a
 * real improvement over `localStorage`, not a complete answer, and the
 * difference matters enough to write down.
 */
class KeyVault(private val context: Context) {

    companion object {
        private const val KEYSTORE = "AndroidKeyStore"
        private const val KEY_ALIAS = "lkng.identity.wrapping.v1"
        private const val PREFS = "lkng.vault"
        private const val TRANSFORM = "AES/GCM/NoPadding"
        private const val GCM_TAG_BITS = 128
        private const val IV_BYTES = 12
    }

    private fun wrappingKey(): SecretKey {
        val ks = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (ks.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }

        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        gen.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                // Deliberately NOT requiring user authentication: the node
                // must be able to sign presence while the screen is off, and
                // a key that needs a fingerprint every epoch would mean the
                // app silently stops working in the user's pocket.
                .setUserAuthenticationRequired(false)
                .build()
        )
        return gen.generateKey()
    }

    /** Seal arbitrary bytes. Returns base64 of `iv || ciphertext`. */
    private fun seal(plain: ByteArray): String {
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, wrappingKey())
        val out = cipher.iv + cipher.doFinal(plain)
        return Base64.encodeToString(out, Base64.NO_WRAP)
    }

    private fun unseal(blob: String): ByteArray? = try {
        val raw = Base64.decode(blob, Base64.NO_WRAP)
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(
            Cipher.DECRYPT_MODE,
            wrappingKey(),
            GCMParameterSpec(GCM_TAG_BITS, raw, 0, IV_BYTES)
        )
        cipher.doFinal(raw, IV_BYTES, raw.size - IV_BYTES)
    } catch (e: Exception) {
        // A failure here means the Keystore key is gone — the user cleared
        // app data, restored to a new device, or the hardware key was
        // invalidated. The seed is unrecoverable by design; recovery is the
        // passphrase bundle's job, not this one's.
        null
    }

    // -- JavaScript bridge -------------------------------------------------
    //
    // Exposed to the WebView under `LkngVault`. The surface is deliberately
    // three methods wide: fetch a secret, store one, report whether the
    // vault exists. Nothing here takes a path, a URL, or anything else
    // that could be turned into a general capability.

    /** Existing secret for `name`, or null. Base64 of raw bytes. */
    @JavascriptInterface
    fun get(name: String): String? {
        if (!isSafeName(name)) return null
        val stored = context
            .getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString(name, null) ?: return null
        return unseal(stored)?.let { Base64.encodeToString(it, Base64.NO_WRAP) }
    }

    /** Store `valueB64` under `name`, sealed by the Keystore. */
    @JavascriptInterface
    fun put(name: String, valueB64: String): Boolean {
        if (!isSafeName(name)) return false
        return try {
            val plain = Base64.decode(valueB64, Base64.NO_WRAP)
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                .edit()
                .putString(name, seal(plain))
                .apply()
            true
        } catch (e: Exception) {
            false
        }
    }

    /**
     * Whether the wrapping key exists in the Keystore at all.
     *
     * Deliberately **not** claiming hardware backing: `KeyInfo`
     * introspection varies across API levels and vendors, and reporting
     * "hardware-backed: true" on a device where it is not would be worse
     * than reporting nothing. The UI should say "protected by this device's
     * keystore", which is verifiable, rather than a security claim we
     * cannot check.
     */
    @JavascriptInterface
    fun isSealed(): Boolean = try {
        val ks = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        ks.containsAlias(KEY_ALIAS)
    } catch (e: Exception) {
        false
    }

    /** Reject anything that is not a plain identifier. */
    private fun isSafeName(name: String): Boolean =
        name.isNotEmpty() && name.length <= 64 &&
            name.all { it.isLetterOrDigit() || it == '.' || it == '_' || it == '-' }
}
