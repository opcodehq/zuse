package sh.zuse.core

data class MarkdownBlock(val diagram: Boolean, val content: String, val language: String? = null) {
    companion object {
        /**
         * Complete top-level fences become native code/diagram surfaces; partial streams stay text.
         */
        fun parse(markdown: String): List<MarkdownBlock> {
            val blocks = ArrayList<MarkdownBlock>()
            val text = ArrayList<String>()
            val fence = ArrayList<String>()
            var character: Char? = null
            var length = 0
            var language = ""
            fun flushText() {
                val content = text.joinToString("\n")
                if (content.isNotBlank()) blocks.add(MarkdownBlock(false, content))
                text.clear()
            }
            for (line in markdown.split('\n')) {
                val trimmed = line.trimStart(' ')
                val indent = line.length - trimmed.length
                val first = trimmed.firstOrNull()
                val count = trimmed.takeWhile { it == first }.length
                if (character != null) {
                    if (
                        indent <= 3 &&
                            first == character &&
                            count >= length &&
                            trimmed.drop(count).isBlank()
                    ) {
                        flushText()
                        blocks.add(
                            MarkdownBlock(
                                language == "mermaid",
                                fence.drop(1).joinToString("\n"),
                                language.ifEmpty { "code" },
                            )
                        )
                        fence.clear()
                        character = null
                    } else fence.add(line)
                } else if (indent <= 3 && first in listOf('`', '~') && count >= 3) {
                    character = first
                    length = count
                    language =
                        trimmed.drop(count).trim().takeWhile { !it.isWhitespace() }.lowercase()
                    fence.add(line)
                } else text.add(line)
            }
            if (fence.isNotEmpty()) text.addAll(fence)
            flushText()
            return blocks
        }
    }
}
