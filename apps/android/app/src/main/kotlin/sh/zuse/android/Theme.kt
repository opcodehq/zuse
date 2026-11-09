package sh.zuse.android

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

private val light =
    lightColorScheme(
        primary = Color(0xFF486900),
        onPrimary = Color.White,
        primaryContainer = Color(0xFFE6EFD3),
        onPrimaryContainer = Color(0xFF314800),
        secondary = Color(0xFF62665B),
        onSecondary = Color.White,
        tertiary = Color(0xFF486900),
        onTertiary = Color.White,
        tertiaryContainer = Color(0xFFE6EFD3),
        onTertiaryContainer = Color(0xFF314800),
        inversePrimary = Color(0xFF89CA21),
        surfaceTint = Color(0xFF486900),
        outline = Color(0xFF74796B),
        secondaryContainer = Color(0xFFE8EDDF),
        onSecondaryContainer = Color(0xFF20251A),
        background = Color(0xFFF7F7F5),
        onBackground = Color(0xFF171716),
        surface = Color(0xFFF7F7F5),
        onSurface = Color(0xFF171716),
        surfaceContainerLowest = Color.White,
        surfaceContainerLow = Color(0xFFF7F7F5),
        surfaceContainer = Color.White,
        surfaceContainerHigh = Color(0xFFF0F0ED),
        surfaceContainerHighest = Color(0xFFECECEA),
        onSurfaceVariant = Color(0xFF686862),
        outlineVariant = Color(0xFFD5D8CE),
    )
private val dark =
    darkColorScheme(
        primary = Color(0xFF89CA21),
        onPrimary = Color(0xFF17220B),
        primaryContainer = Color(0xFF2D3A1B),
        onPrimaryContainer = Color(0xFFCAE9A0),
        secondary = Color(0xFFC2C9B6),
        onSecondary = Color(0xFF20251A),
        onSecondaryContainer = Color(0xFFE8EDDF),
        tertiary = Color(0xFF89CA21),
        onTertiary = Color(0xFF17220B),
        tertiaryContainer = Color(0xFF2D3A1B),
        onTertiaryContainer = Color(0xFFCAE9A0),
        inversePrimary = Color(0xFF486900),
        surfaceTint = Color(0xFF89CA21),
        outline = Color(0xFF909785),
        secondaryContainer = Color(0xFF3E4634),
        background = Color(0xFF0F0F0F),
        onBackground = Color(0xFFEDEDEB),
        surface = Color(0xFF0F0F0F),
        onSurface = Color(0xFFEDEDEB),
        surfaceContainerLowest = Color(0xFF0B0C09),
        surfaceContainerLow = Color(0xFF191B16),
        surfaceContainer = Color(0xFF1F1F1F),
        surfaceContainerHigh = Color(0xFF282B24),
        surfaceContainerHighest = Color(0xFF33372E),
        onSurfaceVariant = Color(0xFFAAAAA4),
        outlineVariant = Color(0xFF454A3E),
    )

val brandFill: Color
    @Composable get() = if (isSystemInDarkTheme()) Color(0xFF89CA21) else Color(0xFF81BF1D)

/** Android typography and surfaces follow Material 3, with generous One UI-style grouping. */
@Composable
fun ZuseTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (isSystemInDarkTheme()) dark else light,
        typography = Typography(),
        shapes =
            Shapes(
                small = RoundedCornerShape(12.dp),
                medium = RoundedCornerShape(12.dp),
                large = RoundedCornerShape(12.dp),
            ),
        content = content,
    )
}
