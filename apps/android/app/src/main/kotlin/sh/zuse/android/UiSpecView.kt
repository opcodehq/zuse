@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package sh.zuse.android

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.*
import sh.zuse.core.*

@Composable
fun UiSpecView(
    spec: String,
    responded: Boolean = false,
    respond: (suspend (String) -> Unit)? = null,
    validatedPreview: JsonElement? = null,
    savedDraft: Map<String, JsonElement> = emptyMap(),
    onDraftChange: ((String, JsonElement) -> Unit)? = null,
) {
    val app = LocalContext.current.applicationContext as ZuseApplication
    var root by remember(spec) { mutableStateOf<JsonElement?>(validatedPreview) }
    var error by remember(spec) { mutableStateOf<String?>(null) }
    val localValues =
        remember(spec) { mutableStateMapOf<String, JsonElement>().apply { putAll(savedDraft) } }
    val values =
        remember(localValues, onDraftChange) {
            object : MutableMap<String, JsonElement> by localValues {
                override fun put(key: String, value: JsonElement): JsonElement? {
                    val previous = localValues.put(key, value)
                    onDraftChange?.invoke(key, value)
                    return previous
                }
            }
        }
    var submitting by remember(spec) { mutableStateOf(false) }
    var submitted by remember(spec) { mutableStateOf(false) }
    var showSource by remember(spec) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val enabled = respond != null && !submitting && !submitted && !responded
    fun submit(text: String) {
        if (!enabled) return
        submitting = true
        scope.launch {
            try {
                require(text.length <= 64 * 1024) {
                    "This form response is too large. Shorten the answers and try again."
                }
                respond?.invoke(text)
                submitted = true
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                error = failure.message ?: "Could not queue the response."
            } finally {
                submitting = false
            }
        }
    }
    LaunchedEffect(spec) {
        try {
            root = validatedPreview?.also(::validateUiTree) ?: app.uiParser.parse(spec)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            error =
                failure.message?.take(500)
                    ?: "This visual could not be rendered. Update Android System WebView, or view its source below."
        }
    }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        root?.let { UiNode(it, values, enabled, ::submit) }
            ?: if (error == null)
                Text("Rendering visual…", style = MaterialTheme.typography.bodySmall)
            else Unit
        error?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
        if (submitted || responded)
            Text(
                "Response queued",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.primary,
            )
        TextButton(onClick = { showSource = !showSource }, contentPadding = PaddingValues(0.dp)) {
            Text(
                if (showSource) "Hide visual source" else "View visual source",
                style = MaterialTheme.typography.labelSmall,
            )
        }
        if (showSource)
            SelectionContainer { Text(spec, style = MaterialTheme.typography.bodySmall) }
    }
}

@Composable
private fun UiNode(
    node: JsonElement,
    values: MutableMap<String, JsonElement>,
    enabled: Boolean,
    submit: (String) -> Unit,
) {
    val props = node.field("props")
    val type = node.text("typeName").orEmpty()
    val children = props.array("children")
    val name = props.text("name").orEmpty()
    val label = props.text("label") ?: name
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        when (type) {
            "Card",
            "Form" -> {
                props.text("title")?.let { Text(it, style = MaterialTheme.typography.titleSmall) }
                children.forEach { UiNode(it, values, enabled, submit) }
                if (type == "Form")
                    CompactAction(props.text("submitLabel") ?: "Submit", enabled) {
                        submit("Form response:\n" + collectUiAnswers(node, values))
                    }
            }
            "Grid" ->
                FlowRow(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    maxItemsInEachRow = props.long("columns")?.toInt()?.coerceIn(1, 4) ?: 2,
                ) {
                    children.forEach { child ->
                        Box(Modifier.weight(1f).widthIn(min = 120.dp)) {
                            UiNode(child, values, enabled, submit)
                        }
                    }
                }
            "Text" ->
                SelectionContainer {
                    Text(props.text("text").orEmpty(), style = MaterialTheme.typography.bodyMedium)
                }
            "Stat" -> {
                Text(
                    label,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(props.text("value").orEmpty(), style = MaterialTheme.typography.headlineSmall)
                props.text("hint")?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            "KeyValue" ->
                props.array("pairs").forEach { pair ->
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(pair.text("key").orEmpty(), style = MaterialTheme.typography.bodySmall)
                        Text(
                            pair.text("value").orEmpty(),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            "Table" ->
                Column(
                    Modifier.horizontalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                        props.array("columns").forEach { cell ->
                            Text(
                                (cell as? JsonPrimitive)?.content.orEmpty(),
                                Modifier.width(140.dp),
                                style = MaterialTheme.typography.labelMedium,
                            )
                        }
                    }
                    props.array("rows").forEach { row ->
                        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                            (row as? JsonArray).orEmpty().forEach { cell ->
                                SelectionContainer {
                                    Text(
                                        (cell as? JsonPrimitive)?.content.orEmpty(),
                                        Modifier.width(140.dp),
                                        style = MaterialTheme.typography.bodySmall,
                                    )
                                }
                            }
                        }
                    }
                }
            "Progress" -> {
                Text(label, style = MaterialTheme.typography.bodySmall)
                LinearProgressIndicator(
                    progress = { (props.numeric("percent") / 100).toFloat().coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            "Badge",
            "Callout" -> {
                props.text("title")?.let { Text(it, style = MaterialTheme.typography.labelLarge) }
                val color =
                    when (props.text("tone")) {
                        "good" -> MaterialTheme.colorScheme.primary
                        "warn" -> Color(0xFFAD7016)
                        "bad" -> MaterialTheme.colorScheme.error
                        else -> MaterialTheme.colorScheme.onSurface
                    }
                Text(
                    props.text("text") ?: props.text("label").orEmpty(),
                    color = color,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            "List" ->
                props.array("items").forEach { item ->
                    Text(
                        "• ${(item as? JsonPrimitive)?.content.orEmpty()}",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            "Steps",
            "Flow" -> {
                (if (type == "Steps") props.array("items") else props.array("nodes"))
                    .forEachIndexed { index, item ->
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(
                                when (item.text("status")) {
                                    "done" -> "✓"
                                    "failed" -> "×"
                                    else -> "${index + 1}"
                                },
                                color = MaterialTheme.colorScheme.primary,
                            )
                            Column {
                                Text(
                                    item.text("label") ?: "Step ${index + 1}",
                                    style = MaterialTheme.typography.labelLarge,
                                )
                                item.text("detail")?.let {
                                    Text(
                                        it,
                                        style = MaterialTheme.typography.bodySmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                            }
                        }
                    }
                props.text("caption")?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            }
            "Tabs" ->
                props.array("tabs").forEach { tab ->
                    var expanded by remember(tab) { mutableStateOf(false) }
                    TextButton(
                        onClick = { expanded = !expanded },
                        contentPadding = PaddingValues(0.dp),
                    ) {
                        Text("${if (expanded) "▾" else "▸"} ${tab.text("label") ?: "Details"}")
                    }
                    if (expanded) UiNode(tab.field("content"), values, enabled, submit)
                }
            "BarChart",
            "LineChart",
            "DonutChart" -> UiChart(type, props)
            "Input",
            "TextArea" -> {
                val value =
                    (values[name] as? JsonPrimitive)?.contentOrNull ?: props.text("value").orEmpty()
                if (type == "Input")
                    CompactField(
                        label,
                        value,
                        { values[name] = it.take(2000).json() },
                        enabled = enabled,
                        placeholder = props.text("placeholder"),
                    )
                else {
                    Text(label, style = MaterialTheme.typography.labelSmall)
                    OutlinedTextField(
                        value,
                        { values[name] = it.take(8000).json() },
                        modifier = Modifier.fillMaxWidth().heightIn(max = 180.dp),
                        minLines = 2,
                        maxLines = 6,
                        enabled = enabled,
                        placeholder = { Text(props.text("placeholder").orEmpty()) },
                    )
                }
            }
            "Select",
            "RadioGroup" -> {
                val options =
                    props.array("options").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
                val value =
                    (values[name] as? JsonPrimitive)?.contentOrNull
                        ?: props.text("value")
                        ?: options.firstOrNull().orEmpty()
                Text(label, style = MaterialTheme.typography.labelSmall)
                if (type == "RadioGroup")
                    options.forEach { option ->
                        Row(
                            Modifier.fillMaxWidth().clickable(enabled = enabled) {
                                values[name] = option.json()
                            }
                        ) {
                            RadioButton(
                                value == option,
                                { values[name] = option.json() },
                                enabled = enabled,
                            )
                            Text(
                                option,
                                Modifier.padding(top = 14.dp),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                else {
                    var expanded by remember { mutableStateOf(false) }
                    Box {
                        TextButton(onClick = { expanded = true }, enabled = enabled) { Text(value) }
                        DropdownMenu(expanded, { expanded = false }) {
                            options.forEach { option ->
                                DropdownMenuItem(
                                    text = { Text(option) },
                                    onClick = {
                                        values[name] = option.json()
                                        expanded = false
                                    },
                                )
                            }
                        }
                    }
                }
            }
            "Checkbox" ->
                Row {
                    Checkbox(
                        (values[name] as? JsonPrimitive)?.booleanOrNull ?: props.flag("checked"),
                        { values[name] = it.json() },
                        enabled = enabled,
                    )
                    Text(
                        label,
                        Modifier.padding(top = 14.dp),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            "Slider" -> {
                val minimum = props.numeric("min")
                val maximum = props.numeric("max", minimum + 1)
                val value =
                    ((values[name] as? JsonPrimitive)?.doubleOrNull
                            ?: props.numeric("value", minimum))
                        .coerceIn(minimum, maximum)
                Text("$label · $value", style = MaterialTheme.typography.labelSmall)
                // Normalize to a finite unit interval so extreme valid values cannot overflow
                // Compose's Float range.
                val scale = max(abs(minimum), abs(maximum)).takeIf { it > 0 } ?: 1.0
                val low = minimum / scale
                val range = maximum / scale - low
                if (range > 0 && range.isFinite())
                    Slider(
                        value = ((value / scale - low) / range).toFloat().coerceIn(0f, 1f),
                        enabled = enabled,
                        onValueChange = { fraction ->
                            val raw = (low + fraction * range) * scale
                            val step = props.numeric("step", 1.0)
                            val snapped =
                                if (step > 0 && ((raw - minimum) / step).isFinite())
                                    minimum + kotlin.math.round((raw - minimum) / step) * step
                                else raw
                            values[name] = snapped.coerceIn(minimum, maximum).json()
                        },
                    )
            }
            "FollowUps" ->
                props.array("items").forEach { item ->
                    TextButton(
                        onClick = { submit(item.text("prompt").orEmpty()) },
                        enabled = enabled,
                    ) {
                        Text(item.text("label") ?: "Continue")
                    }
                }
            else ->
                Text(
                    "Unsupported visual component: $type",
                    style = MaterialTheme.typography.bodySmall,
                )
        }
    }
}

@Composable
private fun UiChart(type: String, props: JsonElement) {
    val points = props.array("points")
    val primary = MaterialTheme.colorScheme.primary
    val colors =
        listOf(
            primary,
            Color(0xFF5886B7),
            Color(0xFFD4A848),
            Color(0xFFAA77B4),
            Color(0xFFD98069),
            Color(0xFF63A99B),
        )
    val values = points.map { it.numeric("value") }
    val scale = values.maxOfOrNull { abs(it) }?.takeIf { it > 0 } ?: 1.0
    val normalized = values.map { it / scale }
    val low = min(0.0, normalized.minOrNull() ?: 0.0)
    val high = max(0.0, normalized.maxOrNull() ?: 1.0)
    val range = (high - low).takeIf { it > 0 } ?: 1.0
    props.text("title")?.let { Text(it, style = MaterialTheme.typography.titleSmall) }
    val labelColor = MaterialTheme.colorScheme.onSurface.toArgb()
    val labelSize = with(LocalDensity.current) { 11.sp.toPx() }
    val paint =
        remember(labelColor, labelSize) {
            android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
                color = labelColor
                textSize = labelSize
            }
        }
    val chartHeight =
        if (type == "BarChart") (points.size * 42 + 12).coerceIn(96, 220).dp else 220.dp
    Canvas(
        Modifier.fillMaxWidth().height(chartHeight).semantics {
            contentDescription =
                "${props.text("title").orEmpty()} $type, ${points.size} values. Expand chart values for details."
        }
    ) {
        val inset = 12.dp.toPx()
        val width = size.width - inset * 2
        val height = size.height - inset * 2
        when (type) {
            "BarChart" ->
                normalized.forEachIndexed { index, value ->
                    val row = height / max(1, points.size)
                    val labelled = points.size <= 8
                    val labelHeight = if (labelled) labelSize + 6.dp.toPx() else 0f
                    val zero = ((-low) / range).toFloat() * width + inset
                    val end = ((value - low) / range).toFloat() * width + inset
                    if (labelled) {
                        val label = points[index].text("label").orEmpty()
                        paint.textAlign = android.graphics.Paint.Align.LEFT
                        drawContext.canvas.nativeCanvas.drawText(
                            label.take(30) + if (label.length > 30) "…" else "",
                            inset,
                            inset + row * index + labelSize,
                            paint,
                        )
                        paint.textAlign = android.graphics.Paint.Align.RIGHT
                        drawContext.canvas.nativeCanvas.drawText(
                            points[index].text("value").orEmpty(),
                            size.width - inset,
                            inset + row * index + labelSize,
                            paint,
                        )
                    }
                    drawRect(
                        primary,
                        Offset(min(zero, end), inset + row * index + labelHeight),
                        Size(max(1f, abs(end - zero)), max(1f, min(8.dp.toPx(), row * .35f))),
                    )
                }
            "LineChart" -> {
                val path = Path()
                normalized.forEachIndexed { index, value ->
                    val x = inset + width * index / max(1, normalized.lastIndex)
                    val y = inset + height * (1 - (value - low) / range).toFloat()
                    if (index == 0) path.moveTo(x, y) else path.lineTo(x, y)
                    drawCircle(primary, 3.dp.toPx(), Offset(x, y))
                }
                drawPath(path, primary, style = Stroke(2.dp.toPx()))
            }
            else -> {
                val total = normalized.sum()
                val stroke = min(width, height) * .16f
                val diameter = min(width, height) - stroke
                val origin = Offset((size.width - diameter) / 2, (size.height - diameter) / 2)
                if (total <= 0)
                    drawArc(
                        primary.copy(alpha = .15f),
                        0f,
                        360f,
                        false,
                        origin,
                        Size(diameter, diameter),
                        style = Stroke(stroke),
                    )
                else {
                    var angle = -90f
                    normalized.forEachIndexed { index, value ->
                        val sweep = (value / total * 360).toFloat().coerceIn(0f, 360f)
                        drawArc(
                            colors[index % colors.size],
                            angle,
                            sweep,
                            false,
                            origin,
                            Size(diameter, diameter),
                            style = Stroke(stroke),
                        )
                        angle += sweep
                    }
                }
            }
        }
    }
    var expanded by remember(props) { mutableStateOf(type == "DonutChart") }
    TextButton(onClick = { expanded = !expanded }, contentPadding = PaddingValues(0.dp)) {
        Text(if (expanded) "Hide chart values" else "Chart values")
    }
    if (expanded)
        points.forEach { point ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(point.text("label").orEmpty(), style = MaterialTheme.typography.bodySmall)
                Text(
                    "${point.text("value").orEmpty()} ${props.text("unit").orEmpty()}",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
}
