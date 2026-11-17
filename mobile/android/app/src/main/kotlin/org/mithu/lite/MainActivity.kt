package org.mithu.lite

import android.annotation.SuppressLint
import android.os.Bundle
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.appcompat.app.AppCompatActivity

/**
 * System web view for Mithu Lite, and the same shell for the Servo flavor
 * until a Servo Android embedding is linked. The first screen is the shared
 * new-tab document in assets, produced by the mithu-mobile crate.
 */
class MainActivity : AppCompatActivity() {
    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val web = WebView(this)
        web.settings.javaScriptEnabled = true
        web.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                return false
            }
        }
        setContentView(web)
        web.loadUrl("file:///android_asset/newtab.html")
    }
}
