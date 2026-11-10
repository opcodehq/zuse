package com.zuse.mobileplatform

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import expo.modules.kotlin.exception.CodedException
import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileInputStream
import java.io.InputStream
import java.util.Locale

private const val FALLBACK_MIME_TYPE = "application/octet-stream"
private const val SHARED_DIRECTORY = "zuse-mobile-platform-shared"
private const val SHARED_FILE_MAX_AGE_MS = 60 * 60 * 1000L

internal class LocalFileException(code: String, message: String, cause: Throwable? = null) :
  CodedException(code, message, cause)

/**
 * One local file handed over from JavaScript. The app passes `file://` cache
 * URIs, `content://` URIs and (for tool output images) `data:` URLs, and all
 * three must be shareable, viewable and savable the same way.
 */
internal sealed class LocalFileSource(val mimeType: String, val displayName: String) {
  class OnDisk(val file: File, mimeType: String) : LocalFileSource(mimeType, file.name)

  class Content(val uri: Uri, mimeType: String, displayName: String) :
    LocalFileSource(mimeType, displayName)

  class Inline(val bytes: ByteArray, mimeType: String, displayName: String) :
    LocalFileSource(mimeType, displayName)

  fun openStream(context: Context): InputStream = when (this) {
    is OnDisk -> FileInputStream(file)
    is Content -> context.contentResolver.openInputStream(uri)
      ?: throw LocalFileException("ERR_FILE_UNREADABLE", "The file could not be opened.")
    is Inline -> ByteArrayInputStream(bytes)
  }

  /**
   * A `content://` URI another app can read once it is granted
   * FLAG_GRANT_READ_URI_PERMISSION. Files outside the provider roots and
   * inline data are copied into a short-lived cache directory first.
   */
  fun contentUri(context: Context): Uri = when (this) {
    is Content -> uri
    is OnDisk -> try {
      FileProvider.getUriForFile(context, providerAuthority(context), file)
    } catch (_: IllegalArgumentException) {
      providerUriForCopy(context, displayName) { file.inputStream() }
    }
    is Inline -> providerUriForCopy(context, displayName) { ByteArrayInputStream(bytes) }
  }

  companion object {
    fun resolve(context: Context, rawUri: String): LocalFileSource {
      if (rawUri.startsWith("data:", ignoreCase = true)) return parseDataUrl(rawUri)
      val uri = Uri.parse(rawUri)
      return when (uri.scheme?.lowercase(Locale.ROOT)) {
        ContentResolver.SCHEME_CONTENT -> Content(
          uri,
          context.contentResolver.getType(uri) ?: FALLBACK_MIME_TYPE,
          contentDisplayName(context, uri)
        )
        ContentResolver.SCHEME_FILE, null -> {
          val path = (if (uri.scheme == null) rawUri else uri.path)
            ?: throw LocalFileException("ERR_FILE_NOT_FOUND", "The file could not be found.")
          val file = File(path)
          if (!file.isFile) {
            throw LocalFileException("ERR_FILE_NOT_FOUND", "The file could not be found.")
          }
          OnDisk(file, mimeTypeForName(file.name))
        }
        else -> throw LocalFileException(
          "ERR_UNSUPPORTED_URI",
          "Only local files can be opened."
        )
      }
    }

    fun extensionForMimeType(mimeType: String): String =
      MimeTypeMap.getSingleton().getExtensionFromMimeType(mimeType) ?: "bin"

    private fun mimeTypeForName(name: String): String {
      val extension = name.substringAfterLast('.', "").lowercase(Locale.ROOT)
      if (extension.isEmpty()) return FALLBACK_MIME_TYPE
      return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) ?: FALLBACK_MIME_TYPE
    }

    private fun contentDisplayName(context: Context, uri: Uri): String {
      val queried = runCatching {
        context.contentResolver.query(
          uri,
          arrayOf(OpenableColumns.DISPLAY_NAME),
          null,
          null,
          null
        )?.use { cursor ->
          if (cursor.moveToFirst()) cursor.getString(0) else null
        }
      }.getOrNull()
      return queried ?: uri.lastPathSegment ?: "file"
    }

    private fun parseDataUrl(rawUri: String): Inline {
      val comma = rawUri.indexOf(',')
      if (comma < 0) throw LocalFileException("ERR_INVALID_DATA_URL", "The file data is invalid.")
      val header = rawUri.substring("data:".length, comma)
      val payload = rawUri.substring(comma + 1)
      val parameters = header.split(';')
      val mimeType = parameters.first().trim().ifEmpty { "text/plain" }.lowercase(Locale.ROOT)
      val isBase64 = parameters.drop(1).any { it.trim().equals("base64", ignoreCase = true) }
      val bytes = try {
        if (isBase64) Base64.decode(payload, Base64.DEFAULT) else Uri.decode(payload).toByteArray()
      } catch (cause: IllegalArgumentException) {
        throw LocalFileException("ERR_INVALID_DATA_URL", "The file data is invalid.", cause)
      }
      val name = "zuse-${System.currentTimeMillis()}.${extensionForMimeType(mimeType)}"
      return Inline(bytes, mimeType, name)
    }

    private fun providerAuthority(context: Context): String =
      "${context.packageName}.zuse.mobileplatform.files"

    private fun providerUriForCopy(
      context: Context,
      displayName: String,
      open: () -> InputStream
    ): Uri {
      val directory = File(context.cacheDir, SHARED_DIRECTORY)
      if (!directory.isDirectory && !directory.mkdirs()) {
        throw LocalFileException("ERR_FILE_UNWRITABLE", "The file could not be prepared.")
      }
      pruneSharedCopies(directory)
      val target = File(directory, "${System.nanoTime()}-${safeFileName(displayName)}")
      open().use { input -> target.outputStream().use { output -> input.copyTo(output) } }
      return FileProvider.getUriForFile(context, providerAuthority(context), target)
    }

    private fun pruneSharedCopies(directory: File) {
      val cutoff = System.currentTimeMillis() - SHARED_FILE_MAX_AGE_MS
      directory.listFiles()?.forEach { file ->
        if (file.lastModified() < cutoff) file.delete()
      }
    }

    private fun safeFileName(name: String): String =
      name.replace(Regex("[^A-Za-z0-9._-]+"), "-").take(96).ifEmpty { "file" }
  }
}
