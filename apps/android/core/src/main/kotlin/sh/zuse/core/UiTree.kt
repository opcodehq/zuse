package sh.zuse.core

import kotlinx.serialization.json.*

/** Native recursion/memory bounds supplement the canonical OpenUI component validator. */
fun validateUiTree(root: JsonElement) {
    var count = 0
    fun visit(node: JsonElement, depth: Int) {
        require(depth < 24 && ++count <= 512) {
            "This visual is too deeply nested or contains too many components."
        }
        require(!node.text("typeName").isNullOrEmpty()) { "Invalid visual component." }
        val props = node.field("props")
        props.array("children").forEach { visit(it, depth + 1) }
        props.array("tabs").forEach { visit(it.field("content"), depth + 1) }
    }
    visit(root, 0)
}

fun collectUiAnswers(root: JsonElement, values: Map<String, JsonElement>): JsonObject {
    val answers = LinkedHashMap<String, JsonElement>()
    fun visit(node: JsonElement) {
        val props = node.field("props")
        props.text("name")?.let { name ->
            val fallback =
                when (node.text("typeName")) {
                    "Checkbox" -> props.flag("checked").json()
                    "Slider" -> props.field("value").takeIf { it != JsonNull } ?: props.field("min")
                    "Select",
                    "RadioGroup" ->
                        props.field("value").takeIf { it != JsonNull }
                            ?: props.array("options").firstOrNull()
                            ?: "".json()
                    else -> props.field("value").takeIf { it != JsonNull } ?: "".json()
                }
            answers[name] = values[name] ?: fallback
        }
        props.array("children").forEach(::visit)
        props.array("tabs").forEach { visit(it.field("content")) }
    }
    validateUiTree(root)
    visit(root)
    return JsonObject(answers)
}

fun JsonElement.numeric(key: String, fallback: Double = 0.0): Double =
    (field(key) as? JsonPrimitive)?.doubleOrNull?.takeIf { it.isFinite() } ?: fallback
