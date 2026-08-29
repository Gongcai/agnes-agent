package com.agnes.agent.secrets

import android.app.Activity
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.nio.ByteBuffer
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
private const val MASTER_KEY_ALIAS = "com.agnes.agent.secrets.master.v1"
private const val PREFERENCES_NAME = "agnes_secure_secrets"
private const val BLOB_VERSION: Byte = 1
private const val GCM_TAG_BITS = 128
private const val MAX_SECRET_ID_BYTES = 1024
private const val MAX_SECRET_VALUE_BYTES = 1024 * 1024
private const val TAG = "AgnesSecrets"

@InvokeArg
class SecretIdArgs {
  lateinit var secretId: String
}

@InvokeArg
class SetSecretArgs {
  lateinit var secretId: String
  lateinit var value: String
}

internal class AndroidKeystoreVault(
  context: Context,
  private val preferencesName: String = PREFERENCES_NAME,
  private val masterKeyAlias: String = MASTER_KEY_ALIAS,
) {
  private val preferences = context.getSharedPreferences(preferencesName, Context.MODE_PRIVATE)
  private val lock = Any()

  fun get(secretId: String): String? = synchronized(lock) {
    validateSecretId(secretId)
    val key = getOrCreateMasterKey()
    val encoded = preferences.getString(preferenceKey(secretId), null) ?: return@synchronized null
    val blob = try {
      Base64.decode(encoded, Base64.NO_WRAP)
    } catch (exception: IllegalArgumentException) {
      throw IllegalStateException("Stored secret has invalid encoding", exception)
    }
    decrypt(key, secretId, blob)
  }

  fun set(secretId: String, value: String) = synchronized(lock) {
    validateSecretId(secretId)
    val plaintext = value.toByteArray(Charsets.UTF_8)
    require(plaintext.size <= MAX_SECRET_VALUE_BYTES) { "Secret value is too large" }
    val encoded = Base64.encodeToString(
      encrypt(getOrCreateMasterKey(), secretId, plaintext),
      Base64.NO_WRAP,
    )
    check(preferences.edit().putString(preferenceKey(secretId), encoded).commit()) {
      "Unable to commit encrypted secret"
    }
  }

  fun delete(secretId: String) = synchronized(lock) {
    validateSecretId(secretId)
    check(preferences.edit().remove(preferenceKey(secretId)).commit()) {
      "Unable to commit secret deletion"
    }
  }

  internal fun clearForTesting() = synchronized(lock) {
    preferences.edit().clear().commit()
    val keyStore = loadKeyStore()
    if (keyStore.containsAlias(masterKeyAlias)) keyStore.deleteEntry(masterKeyAlias)
  }

  private fun getOrCreateMasterKey(): SecretKey {
    val keyStore = loadKeyStore()
    val existing = keyStore.getKey(masterKeyAlias, null)
    if (existing != null) {
      return existing as? SecretKey
        ?: throw IllegalStateException("Keystore alias does not contain a secret key")
    }
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE_PROVIDER)
    generator.init(
      KeyGenParameterSpec.Builder(
        masterKeyAlias,
        KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
      )
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .setRandomizedEncryptionRequired(true)
        .build()
    )
    return generator.generateKey()
  }

  private fun loadKeyStore(): KeyStore {
    return KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
  }

  private fun encrypt(key: SecretKey, secretId: String, plaintext: ByteArray): ByteArray {
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.ENCRYPT_MODE, key)
    cipher.updateAAD(aad(secretId))
    val ciphertext = cipher.doFinal(plaintext)
    require(cipher.iv.size in 12..16) { "Unexpected GCM nonce size" }
    return ByteBuffer.allocate(2 + cipher.iv.size + ciphertext.size)
      .put(BLOB_VERSION)
      .put(cipher.iv.size.toByte())
      .put(cipher.iv)
      .put(ciphertext)
      .array()
  }

  private fun decrypt(key: SecretKey, secretId: String, blob: ByteArray): String {
    require(blob.size >= 2 + 12 + 16) { "Stored secret is truncated" }
    val buffer = ByteBuffer.wrap(blob)
    require(buffer.get() == BLOB_VERSION) { "Stored secret version is unsupported" }
    val nonceSize = buffer.get().toInt() and 0xff
    require(nonceSize in 12..16 && buffer.remaining() >= nonceSize + 16) {
      "Stored secret has an invalid nonce"
    }
    val nonce = ByteArray(nonceSize)
    buffer.get(nonce)
    val ciphertext = ByteArray(buffer.remaining())
    buffer.get(ciphertext)
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(GCM_TAG_BITS, nonce))
    cipher.updateAAD(aad(secretId))
    return cipher.doFinal(ciphertext).toString(Charsets.UTF_8)
  }

  private fun preferenceKey(secretId: String): String {
    val digest = MessageDigest.getInstance("SHA-256").digest(secretId.toByteArray(Charsets.UTF_8))
    return Base64.encodeToString(digest, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
  }

  private fun aad(secretId: String): ByteArray {
    return "com.agnes.agent\u0000$secretId".toByteArray(Charsets.UTF_8)
  }

  private fun validateSecretId(secretId: String) {
    val bytes = secretId.toByteArray(Charsets.UTF_8)
    require(secretId.isNotBlank()) { "Secret ID is empty" }
    require(bytes.size <= MAX_SECRET_ID_BYTES) { "Secret ID is too large" }
  }
}

@TauriPlugin
class SecretStorePlugin(activity: Activity) : Plugin(activity) {
  private val vault = AndroidKeystoreVault(activity.applicationContext)

  @Command
  fun getSecret(invoke: Invoke) {
    runSecureCommand(invoke, "read") {
      val args = invoke.parseArgs(SecretIdArgs::class.java)
      val result = JSObject()
      vault.get(args.secretId)?.let { result.put("value", it) }
      invoke.resolve(result)
    }
  }

  @Command
  fun setSecret(invoke: Invoke) {
    runSecureCommand(invoke, "write") {
      val args = invoke.parseArgs(SetSecretArgs::class.java)
      vault.set(args.secretId, args.value)
      resolveSuccess(invoke)
    }
  }

  @Command
  fun deleteSecret(invoke: Invoke) {
    runSecureCommand(invoke, "delete") {
      val args = invoke.parseArgs(SecretIdArgs::class.java)
      vault.delete(args.secretId)
      resolveSuccess(invoke)
    }
  }

  private fun resolveSuccess(invoke: Invoke) {
    val result = JSObject()
    result.put("success", true)
    invoke.resolve(result)
  }

  private inline fun runSecureCommand(invoke: Invoke, operation: String, block: () -> Unit) {
    try {
      block()
    } catch (exception: Exception) {
      Log.e(TAG, "Android Keystore $operation failed", exception)
      invoke.reject("Android Keystore $operation failed: ${exception.javaClass.simpleName}")
    }
  }
}
