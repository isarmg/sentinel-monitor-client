package org.sarmg.xcoc

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Only ciphertext is persisted; the key remains in Android Keystore. */
class SecurePairing(context: Context) {
    private val prefs = context.getSharedPreferences("xcoc", Context.MODE_PRIVATE)
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("xcoc-pairing", null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder("xcoc-pairing", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
            generateKey()
        }
    }
    fun installationId(): String {
        prefs.getString("installation", null)?.let { return it }
        val id = UUID.randomUUID().toString()
        check(prefs.edit().putString("installation", id).commit())
        return id
    }
    fun save(pairing: String) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        val bytes = cipher.iv + cipher.doFinal(pairing.toByteArray(Charsets.UTF_8))
        check(prefs.edit().putString("pairing", Base64.encodeToString(bytes, Base64.NO_WRAP)).commit())
    }
    fun load(): String? {
        val encoded = prefs.getString("pairing", null) ?: return null
        val bytes = Base64.decode(encoded, Base64.NO_WRAP)
        check(bytes.size >= 28)
        return Cipher.getInstance("AES/GCM/NoPadding").run {
            init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
            String(doFinal(bytes.copyOfRange(12, bytes.size)), Charsets.UTF_8)
        }
    }
}
