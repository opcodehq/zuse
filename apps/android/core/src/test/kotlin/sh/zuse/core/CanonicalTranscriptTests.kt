package sh.zuse.core

import kotlin.test.*
import kotlinx.serialization.json.*

class CanonicalTranscriptTests {
    private fun entry(id: String, text: String) =
        obj(
            "id" to id.json(),
            "parts" to
                JsonArray(
                    listOf(obj("id" to "p".json(), "kind" to "text".json(), "text" to text.json()))
                ),
        )

    @Test
    fun `reset ordered upserts removes and UTF8 appends match host contract`() {
        val fold = CanonicalTranscript()
        fold.apply(obj("reset" to JsonArray(listOf(entry("a", "hello"), entry("b", "old")))))
        val rows =
            fold.apply(
                obj(
                    "remove" to JsonArray(listOf("b".json())),
                    "upsert" to
                        JsonArray(listOf(obj("after" to "a".json(), "entry" to entry("c", "new")))),
                    "append" to
                        JsonArray(
                            listOf(
                                obj(
                                    "entry" to "a".json(),
                                    "part" to "p".json(),
                                    "text" to "🌍".json(),
                                    "len" to 9.json(),
                                )
                            )
                        ),
                    "count" to 2.json(),
                )
            )
        assertEquals(listOf("a", "c"), rows.map { it.text("id") })
        assertEquals("hello🌍", rows.first().array("parts").first().text("text"))
    }

    @Test
    fun `invalid delta does not corrupt last accepted state`() {
        val fold = CanonicalTranscript()
        assertFailsWith<IllegalArgumentException> { fold.apply(obj("count" to 0.json())) }
        fold.apply(obj("reset" to JsonArray(listOf(entry("a", "hello")))))
        assertFailsWith<IllegalArgumentException> {
            fold.apply(obj("remove" to JsonArray(listOf("a".json())), "count" to 1.json()))
        }
        assertEquals("a", fold.apply(obj("count" to 1.json())).single().text("id"))
    }

    @Test
    fun `pending questions disappear when resolved and never become text rows`() {
        val fold = CanonicalTranscript()
        val question =
            obj(
                "id" to "q".json(),
                "question" to "Continue?".json(),
                "options" to JsonArray(listOf("Yes".json(), "No".json())),
            )
        fun message(resolved: Boolean) =
            obj(
                "id" to "m".json(),
                "role" to "assistant".json(),
                "parts" to
                    JsonArray(
                        listOf(
                            obj(
                                "id" to "input".json(),
                                "kind" to "input".json(),
                                "requestId" to "request".json(),
                                "resolved" to resolved.json(),
                                "questions" to JsonArray(listOf(question)),
                            )
                        )
                    ),
            )
        fold.apply(obj("reset" to JsonArray(listOf(message(false)))))
        assertEquals("request", fold.interactions().single().text("id"))
        assertTrue(fold.messages("chat").isEmpty())
        fold.apply(
            obj(
                "upsert" to JsonArray(listOf(obj("after" to JsonNull, "entry" to message(true)))),
                "count" to 1.json(),
            )
        )
        assertTrue(fold.interactions().isEmpty())
    }

    @Test
    fun `streaming retains unchanged message objects and never crosses session caches`() {
        val fold = CanonicalTranscript()
        fold.apply(obj("reset" to JsonArray(listOf(entry("old", "history"), entry("live", "a")))))
        val before = fold.messages("chat")
        fold.apply(
            obj(
                "append" to
                    JsonArray(
                        listOf(
                            obj(
                                "entry" to "live".json(),
                                "part" to "p".json(),
                                "text" to "b".json(),
                                "len" to 2.json(),
                            )
                        )
                    ),
                "count" to 2.json(),
            )
        )
        val after = fold.messages("chat")
        assertSame(before[0], after[0])
        assertNotSame(before[1], after[1])
        assertEquals("ab", after[1].text)
        assertTrue(fold.messages("other-chat").all { it.sessionId == "other-chat" })
    }

    @Test
    fun `canonical tool variants retain readable labels inputs and output`() {
        val fold = CanonicalTranscript()
        val tool =
            obj(
                "id" to "read".json(),
                "kind" to "tool".json(),
                "call" to obj("kind" to "readFile".json(), "path" to "src/main.rs".json()),
                "resolved" to true.json(),
                "output" to "file content".json(),
            )
        fold.apply(
            obj(
                "reset" to
                    JsonArray(
                        listOf(
                            obj(
                                "id" to "m".json(),
                                "role" to "assistant".json(),
                                "parts" to JsonArray(listOf(tool)),
                            )
                        )
                    )
            )
        )
        val row = fold.messages("chat").single()
        assertEquals("Read file", row.content.text("tool"))
        assertEquals("src/main.rs\nfile content", row.text)
        assertTrue(row.content.flag("resolved"))
        assertEquals("readFile", row.content.field("input").text("kind"))
    }
}
