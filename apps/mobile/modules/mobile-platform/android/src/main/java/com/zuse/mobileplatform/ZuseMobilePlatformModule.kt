package com.zuse.mobileplatform

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File

class ZuseMobilePlatformModule : Module() {
  private fun requireContext(): Context = appContext.reactContext
    ?: throw LocalFileException("ERR_CONTEXT_UNAVAILABLE", "The app is not ready yet.")

  override fun definition() = ModuleDefinition {
    Name("ZuseMobilePlatform")

    // Android has no counterpart to iOS's short background-execution grant;
    // 0 is the shared "no task" identifier the JavaScript layer ignores.
    AsyncFunction<Int>("beginBackgroundTask") { 0 }

    AsyncFunction("endBackgroundTask") { _: Int -> }

    Function("presentQuickLook") { uri: String -> presentFile(uri) }

    AsyncFunction("saveImageToPhotos") { uri: String -> saveImageToPictures(uri) }

    AsyncFunction("shareFile") { uri: String, title: String? -> shareFile(uri, title) }
  }

  /** Opens the file in whichever installed app handles its type. */
  private fun presentFile(rawUri: String): Boolean {
    val activity = appContext.currentActivity ?: return false
    val source = try {
      LocalFileSource.resolve(activity, rawUri)
    } catch (_: Exception) {
      return false
    }
    return try {
      val intent = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(source.contentUri(activity), source.mimeType)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }
      activity.startActivity(intent)
      true
    } catch (_: ActivityNotFoundException) {
      false
    } catch (_: SecurityException) {
      false
    } catch (_: IllegalArgumentException) {
      false
    }
  }

  private fun shareFile(rawUri: String, title: String?) {
    val activity = appContext.currentActivity
      ?: throw LocalFileException("ERR_NO_ACTIVITY", "Sharing is unavailable right now.")
    val source = LocalFileSource.resolve(activity, rawUri)
    val contentUri = source.contentUri(activity)
    val send = Intent(Intent.ACTION_SEND).apply {
      type = source.mimeType
      putExtra(Intent.EXTRA_STREAM, contentUri)
      clipData = ClipData.newRawUri(title ?: source.displayName, contentUri)
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    val chooser = Intent.createChooser(send, title).apply {
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    activity.runOnUiThread {
      try {
        activity.startActivity(chooser)
      } catch (_: ActivityNotFoundException) {
        // The system chooser always exists; nothing useful to report.
      }
    }
  }

  /**
   * Scoped-storage insert into Pictures/Zuse. Android 10+ needs no runtime
   * permission for this; older versions would need WRITE_EXTERNAL_STORAGE, so
   * `false` there means "not supported on this device".
   */
  private fun saveImageToPictures(rawUri: String): Boolean {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false
    val context = requireContext()
    val source = LocalFileSource.resolve(context, rawUri)
    if (!source.mimeType.startsWith("image/")) {
      throw LocalFileException("ERR_NOT_AN_IMAGE", "Only images can be saved.")
    }
    val resolver = context.contentResolver
    val collection = MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
    val values = ContentValues().apply {
      put(MediaStore.Images.Media.DISPLAY_NAME, pictureName(source))
      put(MediaStore.Images.Media.MIME_TYPE, source.mimeType)
      put(
        MediaStore.Images.Media.RELATIVE_PATH,
        Environment.DIRECTORY_PICTURES + File.separator + "Zuse"
      )
      put(MediaStore.Images.Media.IS_PENDING, 1)
    }
    val item = resolver.insert(collection, values)
      ?: throw LocalFileException("ERR_SAVE_FAILED", "The image could not be saved.")
    try {
      val output = resolver.openOutputStream(item)
        ?: throw LocalFileException("ERR_SAVE_FAILED", "The image could not be saved.")
      output.use { out -> source.openStream(context).use { input -> input.copyTo(out) } }
      resolver.update(
        item,
        ContentValues().apply { put(MediaStore.Images.Media.IS_PENDING, 0) },
        null,
        null
      )
    } catch (cause: Exception) {
      runCatching { resolver.delete(item, null, null) }
      throw cause
    }
    return true
  }

  private fun pictureName(source: LocalFileSource): String {
    val name = source.displayName
    return if (name.contains('.')) {
      name
    } else {
      "$name.${LocalFileSource.extensionForMimeType(source.mimeType)}"
    }
  }
}
