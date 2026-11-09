package sh.zuse.core

data class DiffLine(val text: String, val kind: Kind, val oldLine: Int?, val newLine: Int?) {
    enum class Kind {
        CONTEXT,
        ADDITION,
        DELETION,
        HEADER,
    }

    companion object {
        private val hunk = Regex("^@@ -(\\d+)(?:,\\d+)? \\+(\\d+)(?:,\\d+)? @@.*")

        fun parse(patch: String): List<DiffLine> {
            var old = 0
            var new = 0
            var inHunk = false
            return patch
                .lineSequence()
                .map { line ->
                    val match = hunk.matchEntire(line)
                    when {
                        match != null -> {
                            old = match.groupValues[1].toInt()
                            new = match.groupValues[2].toInt()
                            inHunk = true
                            DiffLine(line, Kind.HEADER, null, null)
                        }
                        !inHunk || line.startsWith("\\") -> DiffLine(line, Kind.HEADER, null, null)
                        line.startsWith("+") -> DiffLine(line, Kind.ADDITION, null, new++)
                        line.startsWith("-") -> DiffLine(line, Kind.DELETION, old++, null)
                        line.startsWith(" ") -> DiffLine(line, Kind.CONTEXT, old++, new++)
                        else -> DiffLine(line, Kind.HEADER, null, null)
                    }
                }
                .toList()
        }
    }
}
