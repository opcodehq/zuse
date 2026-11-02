package com.zuse.mobileplatform

import androidx.core.content.FileProvider

/**
 * A dedicated subclass so this provider's manifest entry never merges with
 * another library's `androidx.core.content.FileProvider` declaration.
 */
class ZuseMobilePlatformFileProvider : FileProvider()
