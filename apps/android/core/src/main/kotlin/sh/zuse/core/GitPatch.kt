package sh.zuse.core

/** Match complete Git headers, including C-quoted UTF-8 paths and renames. */
object GitPatch {
    private fun quote(path: String, quoteUnicode: Boolean): String {
        val bytes = path.toByteArray(Charsets.UTF_8)
        if (
            bytes.none {
                val b = it.toInt() and 255
                b < 32 || b == 127 || b == 34 || b == 92 || (quoteUnicode && b >= 128)
            }
        )
            return path
        if (!quoteUnicode) {
            return "\"" +
                path
                    .map { character ->
                        if (character.code >= 128) character.toString()
                        else {
                            val escaped = quote(character.toString(), true)
                            if (escaped.startsWith("\"")) escaped.substring(1, escaped.length - 1)
                            else escaped
                        }
                    }
                    .joinToString("") +
                "\""
        }
        return buildString {
            append('"')
            bytes.forEach {
                val b = it.toInt() and 255
                when (b) {
                    34 -> append("\\\"")
                    92 -> append("\\\\")
                    9 -> append("\\t")
                    10 -> append("\\n")
                    13 -> append("\\r")
                    7 -> append("\\a")
                    8 -> append("\\b")
                    11 -> append("\\v")
                    12 -> append("\\f")
                    else ->
                        if (b < 32 || b == 127 || b >= 128)
                            append("\\" + b.toString(8).padStart(3, '0'))
                        else append(b.toChar())
                }
            }
            append('"')
        }
    }

    fun file(patch: String, path: String, oldPath: String? = null): String? {
        val source = oldPath ?: path
        val headers =
            listOf(true, false)
                .map { unicode ->
                    "diff --git ${quote("a/$source", unicode)} ${quote("b/$path", unicode)}"
                }
                .toSet()
        return patch.split(Regex("(?m)(?=^diff --git )")).firstOrNull {
            it.lineSequence().firstOrNull() in headers
        }
    }
}
