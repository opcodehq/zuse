use std::{collections::BTreeMap, path::Path};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::vscode::{DetectedThemeSource, SourceCompilation};
use crate::{
    AccentRoles, Appearance, Color, ThemeFamily, ThemeRegistry, ThemeSource, ThemeVariant,
};

pub(super) fn compile(value: &Value, path: &Path, id: &str) -> Result<Option<SourceCompilation>> {
    if value.get("version").is_none() || value.get("appearance").is_none() {
        return Ok(None);
    }
    if value["version"].as_u64() != Some(1) {
        bail!("unsupported T3 Code theme version; expected version 1");
    }
    let name = value["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .context("a T3 Code theme needs a name")?;
    let appearance = scheme(&value["appearance"])?;
    let mut variants = vec![variant(
        id,
        name,
        appearance,
        &value["colors"],
        Some(value),
    )?];
    if let Some(other) = value.get("variants").and_then(Value::as_object) {
        for (label, colors) in other {
            let other_appearance = scheme(&Value::String(label.clone()))?;
            if other_appearance != appearance {
                // Unset roles stay on the opposite scheme's defaults, except
                // the accent: that is the theme's identity, so it carries over.
                let inherited = value["colors"]
                    .get("accent")
                    .map(|accent| serde_json::json!({ "accent": accent }));
                variants.push(variant(
                    id,
                    name,
                    other_appearance,
                    colors,
                    inherited.as_ref(),
                )?);
            }
        }
    }
    Ok(Some(SourceCompilation {
        path: path.into(),
        source_kind: DetectedThemeSource::File,
        family: ThemeFamily {
            id: id.into(),
            name: name.into(),
            variants,
        },
        reports: BTreeMap::new(),
        failures: Vec::new(),
    }))
}

fn scheme(value: &Value) -> Result<Appearance> {
    match value.as_str() {
        Some("light") => Ok(Appearance::Light),
        Some("dark") => Ok(Appearance::Dark),
        _ => bail!("T3 Code theme appearance must be light or dark"),
    }
}

fn variant(
    id: &str,
    name: &str,
    appearance: Appearance,
    overrides: &Value,
    seed: Option<&Value>,
) -> Result<ThemeVariant> {
    let suffix = if appearance.is_dark() {
        "dark"
    } else {
        "light"
    };
    let mut variant = ThemeRegistry::builtin()
        .variant(&format!("zuse-{suffix}"))
        .unwrap()
        .clone();
    variant.id = format!("{id}-{suffix}");
    variant.family_id = id.into();
    variant.name = format!(
        "{name} {}",
        if appearance.is_dark() {
            "Dark"
        } else {
            "Light"
        }
    );
    let tone = |role: &str| -> Result<Option<Color>> {
        let Some(value) = overrides
            .get(role)
            .or_else(|| seed.and_then(|seed| seed.get(role)))
        else {
            return Ok(None);
        };
        let input = value
            .as_str()
            .with_context(|| format!("{role} must be a color string"))?;
        let parsed = color::parse_color(input.trim())
            .map_err(|_| anyhow::anyhow!("invalid {role} color: {input}"))?;
        let rgba = parsed.to_alpha_color::<color::Srgb>().to_rgba8();
        Ok(Some(
            Color::rgb(rgba.r, rgba.g, rgba.b).with_alpha(f32::from(rgba.a) / 255.0),
        ))
    };
    let palette = &mut variant.colors;
    let canvas = tone("canvas")?;
    if let Some(canvas) = canvas {
        palette.background = canvas.blend_over(palette.background);
    }
    // Text first: the derived surfaces below are tinted from it.
    if let Some(text) = tone("text")? {
        palette.text = text.blend_over(palette.background);
    }
    if canvas.is_some() {
        palette.shell = palette.background;
        palette.card = palette.background.mix(palette.text, 0.045);
        palette.raised = palette.background.mix(palette.text, 0.075);
        palette.dialog = palette.raised;
        palette.overlay = palette.raised;
        palette.input = palette.card;
        variant.terminal.background = palette.background;
    }
    let background = palette.background;
    for (color, role) in [
        (&mut palette.shell, "sidebar"),
        (&mut palette.card, "surface"),
        (&mut palette.raised, "surfaceRaised"),
        (&mut palette.overlay, "surfaceOverlay"),
        (&mut palette.dialog, "surfaceOverlay"),
        (&mut palette.input, "input"),
        (&mut palette.border, "border"),
        (&mut palette.text_muted, "textMuted"),
        (&mut palette.text_faint, "placeholder"),
        (&mut palette.danger, "error"),
        (&mut palette.warning, "warning"),
        (&mut variant.terminal.background, "terminalBackground"),
        (&mut variant.terminal.foreground, "terminalForeground"),
        (&mut variant.terminal.selection, "terminalSelection"),
    ] {
        if let Some(value) = tone(role)? {
            *color = value.blend_over(background);
        }
    }
    palette.text = palette.text.ensure_contrast(background, 4.5);
    palette.text_muted = palette.text_muted.ensure_contrast(background, 4.5);
    palette.text_strong = crate::ThemeColors::derive_text_strong(palette.text, background);
    palette.selected = crate::ThemeColors::derive_selected(background);
    palette.diff_delete = palette.danger;
    variant.terminal.foreground = variant
        .terminal
        .foreground
        .ensure_contrast(variant.terminal.background, 4.5);
    variant.accent = AccentRoles::derive(
        tone("accent")?
            .unwrap_or(variant.accent.primary)
            .blend_over(background),
        appearance,
        background,
    );
    palette.active = variant.accent.primary.with_alpha(0.14);
    variant.source = ThemeSource {
        format: "t3-code".into(),
        url: "https://t3.codes".into(),
        revision: "theme-v1".into(),
        license: "User supplied".into(),
        asset_hash: String::new(),
    };
    use sha2::{Digest, Sha256};
    variant.source.asset_hash =
        format!("sha256:{:x}", Sha256::digest(serde_json::to_vec(&variant)?));
    Ok(variant)
}
