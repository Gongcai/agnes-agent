package com.agnes.agent.secrets

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AndroidKeystoreVaultTest {
  private lateinit var vault: AndroidKeystoreVault

  @Before
  fun setUp() {
    val suffix = UUID.randomUUID().toString()
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    vault = AndroidKeystoreVault(
      context,
      preferencesName = "agnes_secure_secrets_test_$suffix",
      masterKeyAlias = "com.agnes.agent.secrets.test.$suffix",
    )
  }

  @After
  fun tearDown() {
    vault.clearForTesting()
  }

  @Test
  fun roundTripsOverwritesAndDeletesIndependentSecrets() {
    assertNull(vault.get("provider:first:api_key"))

    vault.set("provider:first:api_key", "first-secret")
    vault.set("provider:second:api_key", "second-secret")
    assertEquals("first-secret", vault.get("provider:first:api_key"))
    assertEquals("second-secret", vault.get("provider:second:api_key"))

    vault.set("provider:first:api_key", "updated-secret\nwith-unicode-密钥")
    assertEquals("updated-secret\nwith-unicode-密钥", vault.get("provider:first:api_key"))

    vault.delete("provider:first:api_key")
    assertNull(vault.get("provider:first:api_key"))
    assertEquals("second-secret", vault.get("provider:second:api_key"))
  }
}
