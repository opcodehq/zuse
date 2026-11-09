package sh.zuse.android

import android.app.Application

class ZuseApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        ru.noties.jlatexmath.JLatexMathAndroid.init(this)
    }

    val uiParser by lazy { NativeUiParser(this) }
    val vault by lazy { Vault(this) }
    val account by lazy { Account(vault) }
}
