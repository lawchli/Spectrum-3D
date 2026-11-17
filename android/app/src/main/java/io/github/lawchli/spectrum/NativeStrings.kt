package io.github.lawchli.spectrum

import android.content.Context
import org.json.JSONObject

/** Native-only strings are added by the shared exporter without modifying the web UI. */
class NativeStrings(context: Context) {
    private val strings = JSONObject(context.assets.open("spectrum/strings.json").bufferedReader().use { it.readText() })
    private val language = context.getSharedPreferences("spectrum", Context.MODE_PRIVATE).getString(
        "lang", if (context.resources.configuration.locales[0].language == "zh") "zh" else "en",
    ) ?: "en"
    fun text(key: String): String = strings.optJSONObject(language)?.optString(key, key) ?: key
}
