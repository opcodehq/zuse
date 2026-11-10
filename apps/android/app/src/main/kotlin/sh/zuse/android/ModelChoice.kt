package sh.zuse.android

import kotlinx.serialization.json.JsonObject
import sh.zuse.core.*

data class ModelChoice(val provider: String, val id: String, val name: String)

fun modelChoices(
    catalog: kotlinx.serialization.json.JsonElement,
    availableOnly: Boolean = false,
    providers: Set<String>? = null,
): List<ModelChoice> =
    (catalog.field("providers") as? JsonObject)
        .orEmpty()
        .filterKeys { providers == null || it in providers }
        .flatMap { (provider, entry) ->
            entry
                .array("models")
                .filter { !availableOnly || it.flag("available") }
                .mapNotNull { model ->
                    model.text("id")?.let { ModelChoice(provider, it, model.text("label") ?: it) }
                }
        }
