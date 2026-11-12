package sh.zuse.core

import java.time.ZoneId
import java.time.ZonedDateTime
import java.util.Locale
import kotlin.test.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.mockwebserver.*

class ChatListTests {
    private val zone = ZoneId.of("UTC")

    private fun at(text: String) = ZonedDateTime.parse(text).toInstant().toEpochMilli()

    private fun row(
        id: String,
        project: String?,
        activity: Long?,
        place: String = "device:laptop",
        status: ChatStatus = ChatStatus.Idle,
        preview: String? = null,
        branch: String? = null,
    ) =
        ChatRow(
            "$place|$id",
            id,
            place,
            if (place == "cloud") "Zuse Cloud" else "Laptop",
            project ?: CanonicalWorkspace.PROJECTLESS,
            project,
            "Chat $id",
            preview,
            status,
            branch,
            activity,
        )

    @Test
    fun `groups chats by project with the newest activity first`() {
        val groups =
            ChatList.groups(
                listOf(
                    row("old", "zuse", 100),
                    row("loose", null, 300),
                    row("new", "zuse", 500),
                    row("site", "website", 200),
                    row("never", "website", null),
                )
            )
        assertEquals(listOf("zuse", "No project", "website"), groups.map { it.title })
        assertEquals(listOf("new", "old"), groups[0].rows.map { it.chatId })
        assertEquals(listOf("site", "never"), groups[2].rows.map { it.chatId })
        assertTrue(groups.all { it.place == null }, "One computer never names itself")
    }

    @Test
    fun `filters by place and search, and names the place only when several are listed`() {
        val rows =
            listOf(
                row("a", "zuse", 3, preview = "Fixed the reconnect loop"),
                row("b", "zuse", 2, place = "cloud"),
                row("c", "docs", 1, branch = "feature/search"),
            )
        val all = ChatList.groups(rows)
        assertEquals(setOf("Zuse Cloud", "Laptop"), all.mapNotNull { it.place }.toSet())
        assertEquals(3, all.size)
        val laptop = ChatList.groups(rows, place = "device:laptop")
        assertEquals(listOf("a", "c"), laptop.flatMap { it.rows }.map { it.chatId })
        assertTrue(laptop.all { it.place == null })
        assertEquals(listOf("a"), ChatList.groups(rows, "reconnect").flatMap { it.rows }.map { it.chatId })
        assertEquals(listOf("c"), ChatList.groups(rows, "feature/").flatMap { it.rows }.map { it.chatId })
        assertEquals(listOf("c"), ChatList.groups(rows, "docs").flatMap { it.rows }.map { it.chatId })
        assertTrue(ChatList.groups(rows, "nothing like this").isEmpty())
    }

    @Test
    fun `the row detail puts status before the latest message`() {
        assertEquals("Working…", ChatList.detail(row("a", null, 1, status = ChatStatus.Working, preview = "hi")))
        assertEquals("Needs input", ChatList.detail(row("a", null, 1, status = ChatStatus.NeedsInput)))
        assertEquals("Failed", ChatList.detail(row("a", null, 1, status = ChatStatus.Failed)))
        assertEquals("Second line wins", ChatList.detail(row("a", null, 1, preview = "\n  Second line wins\nthird")))
        assertNull(ChatList.detail(row("a", null, 1)))
        assertEquals(ChatStatus.NeedsInput, ChatStatus.from("awaitingInput"))
        assertEquals(ChatStatus.Failed, ChatStatus.from("errored"))
        assertEquals(ChatStatus.Working, ChatStatus.from("running"))
        assertEquals(ChatStatus.Idle, ChatStatus.from(null))
    }

    @Test
    fun `relative time reads like a chat list`() {
        val now = at("2026-11-12T15:00:00Z")
        fun format(text: String?) =
            ChatList.relativeTime(text?.let(::at), now, zone, Locale.ENGLISH)
        assertEquals("", format(null))
        assertEquals("now", format("2026-11-12T14:59:30Z"))
        assertEquals("5m", format("2026-11-12T14:55:00Z"))
        assertEquals("3h", format("2026-11-12T12:00:00Z"))
        assertEquals("Yesterday", format("2026-11-11T23:00:00Z"))
        assertEquals("Mon", format("2026-11-09T10:00:00Z"))
        assertEquals("2 Nov", format("2026-11-02T10:00:00Z"))
        assertEquals("12 Nov 2025", format("2025-11-12T10:00:00Z"))
        assertEquals(at("2026-11-12T12:00:00Z"), ChatList.parseTime("2026-11-12T12:00:00Z"))
        assertNull(ChatList.parseTime("not a time"))
    }

    @Test
    fun `new chats without a project only work locally and projects remember their choice`() {
        assertEquals(listOf(CheckoutMode.Local), CheckoutMode.available(null))
        assertEquals(
            listOf(CheckoutMode.Local),
            CheckoutMode.available(CanonicalWorkspace.PROJECTLESS),
        )
        assertEquals(CheckoutMode.entries, CheckoutMode.available("zuse"))
        val remembered =
            mapOf(
                "zuse" to "Worktree",
                CanonicalWorkspace.PROJECTLESS to "Worktree",
                "old" to "Removed",
            )
        assertEquals(CheckoutMode.Local, CheckoutMode.default(null, remembered))
        assertEquals(
            CheckoutMode.Local,
            CheckoutMode.default(CanonicalWorkspace.PROJECTLESS, remembered),
        )
        assertEquals(CheckoutMode.Worktree, CheckoutMode.default("zuse", remembered))
        assertEquals(CheckoutMode.Local, CheckoutMode.default("old", remembered))
        assertEquals(CheckoutMode.Local, CheckoutMode.default("never-used", remembered))
    }

    @Test
    fun `only a request that may have reached the host is uncertain`() {
        assertFalse(RequestOutcome.uncertain(RpcRequestFailure("Invalid", "Rejected by host")))
        assertFalse(RequestOutcome.uncertain(IllegalArgumentException("Choose a project.")))
        assertTrue(RequestOutcome.uncertain(ClientException("Connection lost. Reconnect to synchronize.")))
        assertTrue(RequestOutcome.uncertain(java.io.IOException("reset")))
    }

    @Test
    fun `an unreadable reply after sending is uncertain`() = runBlocking {
        val server = MockWebServer()
        server.enqueue(
            MockResponse()
                .withWebSocketUpgrade(
                    object : WebSocketListener() {
                        override fun onMessage(socket: WebSocket, text: String) {
                            val frame = WireJson.parseToJsonElement(text)
                            val id = frame.field("id")
                            if (frame.text("method") == "QueueCommand")
                                socket.send("{\"id\":$id,\"ok\":")
                            else socket.send(obj("id" to id, "ok" to obj()).toString())
                        }
                    }
                )
        )
        server.start()
        val client = RpcClient(server.url("/api/rpc").toString().replace("http:", "ws:"))
        try {
            client.connect()
            val failure = assertFails { client.call("QueueCommand") }
            assertIs<ClientException>(failure)
            assertTrue(RequestOutcome.uncertain(failure))
        } finally {
            client.close()
            server.shutdown()
        }
    }

    @Test
    fun `canonical chats carry list details and refuse impossible requests before sending`() =
        runBlocking {
            val sent = java.util.concurrent.LinkedBlockingQueue<String>()
            val now = java.time.Instant.now().toString()
            val chats =
                JsonArray(
                    listOf(
                        obj(
                            "id" to "chat".json(),
                            "title" to "Fix reconnect".json(),
                            "branch" to "fix/reconnect".json(),
                            "lastMessagePreview" to "Done, tests pass".json(),
                            "lastMessageAt" to "2026-11-12T12:00:00Z".json(),
                            "createdAt" to "2026-11-10T12:00:00Z".json(),
                            "config" to obj("harness" to "claude".json(), "model" to "m".json()),
                        ),
                        obj(
                            "id" to "unresolved".json(),
                            "createdAt" to "2026-11-10T12:00:00Z".json(),
                            "config" to obj("harness" to "claude".json(), "model" to "m".json()),
                        ),
                    )
                )
            val server = MockWebServer()
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onMessage(socket: WebSocket, text: String) {
                                val frame = WireJson.parseToJsonElement(text)
                                val id = frame.field("id")
                                val method = frame.text("method") ?: return
                                sent.add(method)
                                val reply =
                                    when (method) {
                                        "WatchChats" -> obj("id" to id, "item" to chats)
                                        "WatchSpaces" ->
                                            obj("id" to id, "item" to JsonArray(emptyList()))
                                        "WatchSessions" ->
                                            obj(
                                                "id" to id,
                                                "item" to
                                                    JsonArray(
                                                        listOf(
                                                            obj(
                                                                "chatId" to "chat".json(),
                                                                "status" to "working".json(),
                                                                "updatedAt" to now.json(),
                                                            )
                                                        )
                                                    ),
                                            )
                                        "Mutate" ->
                                            obj("id" to id, "err" to "Rejected by host".json())
                                        else -> obj("id" to id, "ok" to obj())
                                    }
                                socket.send(reply.toString())
                            }
                        }
                    )
            )
            server.start()
            val client = RpcClient(server.url("/api/rpc").toString().replace("http:", "ws:"))
            try {
                client.connect()
                val workspace = CanonicalWorkspace(client)
                val chat = workspace.allSessions().first { it.id == "chat" }
                assertEquals("Done, tests pass", chat.preview)
                assertEquals("2026-11-12T12:00:00Z", chat.activityAt)
                assertEquals("fix/reconnect", chat.branch)
                assertEquals(
                    "2026-11-10T12:00:00Z",
                    workspace.allSessions().first { it.id == "unresolved" }.activityAt,
                )
                assertEquals(mapOf("chat" to "working"), workspace.runtimeStatuses())
                assertEquals(ChatList.NO_PROJECT, workspace.projects().last().name)
                assertTrue(workspace.exists("chat"))
                assertFalse(workspace.exists("missing"))

                val isolated =
                    assertFailsWith<IllegalArgumentException> {
                        workspace.create(
                            obj(
                                "operationId" to "op".json(),
                                "projectId" to CanonicalWorkspace.PROJECTLESS.json(),
                            ),
                            null,
                            fresh = true,
                        )
                    }
                assertFalse(RequestOutcome.uncertain(isolated))

                val rejected =
                    assertFailsWith<RpcRequestFailure> {
                        workspace.create(
                            obj(
                                "operationId" to "op".json(),
                                "projectId" to CanonicalWorkspace.PROJECTLESS.json(),
                                "providerId" to "claude".json(),
                                "model" to "m".json(),
                            ),
                            null,
                            fresh = false,
                        )
                    }
                assertFalse(RequestOutcome.uncertain(rejected))

                val unsent =
                    assertFailsWith<IllegalArgumentException> {
                        workspace.send(PendingMessage("p", "m", "unresolved", "hello"))
                    }
                assertFalse(RequestOutcome.uncertain(unsent))
                assertFalse("QueueCommand" in sent, "A refused message never reaches the host")

                client.close()
                val dropped = assertFails { client.call("EngineInfo") }
                assertTrue(RequestOutcome.uncertain(dropped))
            } finally {
                client.close()
                server.shutdown()
            }
        }
}
