package dev.tunnelrat

import android.app.Application
import android.content.Context

class TuraApp : Application() {
    lateinit var tura: Tura
        private set

    override fun onCreate() {
        super.onCreate()
        tura = Tura(this)
        TuraService.sync(this)
    }
}

val Context.tura: Tura get() = (applicationContext as TuraApp).tura
